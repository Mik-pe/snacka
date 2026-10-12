// SPDX-License-Identifier: MIT
//! Two-party Snacka calls, directly over IPv6/IPv4 or through a UDP relay.
#[path = "../monitor/audio.rs"]
mod audio;
mod connection;
mod editor;
mod jitter;
mod processing;
mod profile;
mod rate;
#[path = "../monitor/resample.rs"]
mod resample;
#[path = "../call_common/transport.rs"]
mod transport;
mod ui;

use connection::Connection;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use editor::{EditAction, Editor};
use processing::{BLOCK, Processing};
use profile::{Family, Profile};
use rate::RateControl;
use snacka::{Config, Encoder, FRAME_SAMPLES, MAX_PACKET_BYTES, SAMPLE_RATE};
use std::{
    error::Error,
    io,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const HELP: &str = "Snacka / Voice Link — encrypted two-person call with a live TUI

cargo run --release --features internet-call --example call -- [OPTIONS]

Start without connection options to choose your destination in the TUI.
Or choose one connection mode:
  --peer ADDRESS     Direct UDP to peer, e.g. [2001:db8::2]:5000 (no relay)
  --relay ADDRESS    Outbound UDP to a call_relay server, e.g. host:5000
  --bind ADDRESS     Local address; direct default [::]:5000 or 0.0.0.0:5000;
                     relay default ephemeral port in the remote address family
  --role 1|2         Different roles on both clients; TUI defaults to role 1
  --ipv4             Resolve and connect using IPv4 only
  --ipv6             Resolve and connect using IPv6 only (default: automatic)

Set the same SNACKA_CALL_KEY (64 hex digits) on both clients and any relay.
The TUI can also accept the room key. IPv4 internet needs port forwarding or a
relay; direct IPv6 needs global addresses and inbound UDP firewall rules.
  --generate-key     Generate a random room key and exit
  --bitrate BPS      Initial codec bitrate 12000..32000 (default 24000)
  --max-bitrate BPS  Adaptive ceiling 12000..32000 (default 32000)
  --fixed-bitrate    Disable automatic network adaptation; fixed 20 ms frames
  --buffer-ms MS     Jitter prebuffer 20..200 (default 60)
  --gain GAIN        Playback volume 0..1 (default 0.5)
  --no-aec           Disable echo cancellation (AEC3)
  --no-ns            Disable noise suppression
  --no-agc           Disable adaptive digital gain control
  --input INDEX      Microphone device
  --output INDEX     Speaker/headphone device
  --list-devices     Print audio devices and exit
  --test-tone        Headless synthetic audio, opens no audio devices
  --seconds SECONDS  Stop automatically
  --no-tui           Plain status output
  --help             Show this help

Keys: M mute; arrows volume/bitrate; A echo cancellation; N noise suppression;
      C connection settings (address, IP family, mode, role, key, hang up);
      G auto gain; R adaptive bitrate; Q, Esc or Ctrl-C hang up. No audio is saved.";

struct Options {
    remote: String,
    direct: bool,
    family: Family,
    bind: Option<String>,
    role: u8,
    bitrate: u32,
    max_bitrate: u32,
    adaptive: bool,
    buffer_ms: u64,
    gain: f32,
    aec: bool,
    ns: bool,
    agc: bool,
    input: Option<usize>,
    output: Option<usize>,
    tone: bool,
    seconds: Option<f64>,
    tui: bool,
}

fn options(args: impl IntoIterator<Item = String>) -> Result<Option<Options>, Box<dyn Error>> {
    let mut options = Options {
        remote: String::new(),
        direct: true,
        family: Family::Auto,
        bind: None,
        role: 0,
        bitrate: 24000,
        max_bitrate: 32000,
        adaptive: true,
        buffer_ms: 60,
        gain: 0.5,
        aec: true,
        ns: true,
        agc: true,
        input: None,
        output: None,
        tone: false,
        seconds: None,
        tui: true,
    };
    let mut args = args.into_iter();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--help" | "-h" => {
                println!("{HELP}");
                return Ok(None);
            }
            "--generate-key" => {
                println!("{}", transport::generate_key()?);
                return Ok(None);
            }
            "--list-devices" => {
                audio::list_devices()?;
                return Ok(None);
            }
            "--no-aec" => options.aec = false,
            "--no-ns" => options.ns = false,
            "--no-agc" => options.agc = false,
            "--fixed-bitrate" => options.adaptive = false,
            "--no-tui" => options.tui = false,
            "--test-tone" => options.tone = true,
            "--ipv4" | "--ipv6" => {
                if options.family != Family::Auto {
                    return Err(io::Error::other("Choose only one of --ipv4 and --ipv6").into());
                }
                options.family = if arg == "--ipv4" {
                    Family::Ipv4
                } else {
                    Family::Ipv6
                };
            }
            "--peer" | "--relay" | "--bind" | "--role" | "--bitrate" | "--max-bitrate"
            | "--buffer-ms" | "--gain" | "--input" | "--output" | "--seconds" => {
                let value = args
                    .next()
                    .ok_or_else(|| io::Error::other(format!("Missing value for {arg}")))?;
                match arg.as_str() {
                    "--peer" | "--relay" => {
                        if !options.remote.is_empty() {
                            return Err(io::Error::other(
                                "Choose exactly one of --peer and --relay",
                            )
                            .into());
                        }
                        options.remote = value;
                        options.direct = arg == "--peer";
                    }
                    "--bind" => options.bind = Some(value),
                    "--role" => options.role = value.parse()?,
                    "--bitrate" => options.bitrate = value.parse()?,
                    "--max-bitrate" => options.max_bitrate = value.parse()?,
                    "--buffer-ms" => options.buffer_ms = value.parse()?,
                    "--gain" => options.gain = value.parse()?,
                    "--input" => options.input = Some(value.parse()?),
                    "--output" => options.output = Some(value.parse()?),
                    "--seconds" => options.seconds = Some(value.parse()?),
                    _ => unreachable!(),
                }
            }
            _ => return Err(io::Error::other(format!("Unknown option {arg}; use --help")).into()),
        }
    }
    if (options.role != 0 && !(1..=2).contains(&options.role))
        || (!options.tui && (options.remote.is_empty() || options.role == 0))
    {
        return Err(io::Error::other(
            "Specify --peer ADDRESS or --relay ADDRESS, and --role 1 or 2; use --help",
        )
        .into());
    }
    Config::new(options.bitrate)?;
    Config::new(options.max_bitrate)?;
    if options.bitrate > options.max_bitrate {
        return Err(io::Error::other("--bitrate must not exceed --max-bitrate").into());
    }
    if !(20..=200).contains(&options.buffer_ms) {
        return Err(io::Error::other("--buffer-ms must be 20..200").into());
    }
    if !options.gain.is_finite() || !(0.0..=1.0).contains(&options.gain) {
        return Err(io::Error::other("--gain must be finite and 0..1").into());
    }
    if options.seconds.is_some_and(|s| !s.is_finite() || s <= 0.0) {
        return Err(io::Error::other("--seconds must be finite and positive").into());
    }
    Ok(Some(options))
}

impl Options {
    fn profile(&self) -> Profile {
        Profile {
            remote: self.remote.clone(),
            bind: self.bind.clone().unwrap_or_default(),
            direct: self.direct,
            family: self.family,
            role: if self.role == 0 { 1 } else { self.role },
            key: std::env::var("SNACKA_CALL_KEY").unwrap_or_default(),
        }
    }
}

fn level(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()))
}

enum Action {
    None,
    Quit,
    Connect(Profile),
    Disconnect,
}

fn controls(
    state: &mut ui::Dashboard,
    encoder: &mut Encoder,
    rate: &mut RateControl,
    processing: &mut Processing,
    editor: &mut Option<Editor>,
    profile: &Profile,
) -> io::Result<Action> {
    let mut rebuild = false;
    while event::poll(Duration::ZERO)? {
        let event = event::read()?;
        if let Event::Paste(text) = &event
            && let Some(editor) = editor
        {
            editor.paste(text);
            continue;
        }
        if let Event::Key(key) = event
            && key.kind != KeyEventKind::Release
        {
            if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
                return Ok(Action::Quit);
            }
            if let Some(form) = editor {
                match form.key(key) {
                    EditAction::Cancel => *editor = None,
                    EditAction::Connect => return Ok(Action::Connect(form.draft.clone())),
                    EditAction::Disconnect => return Ok(Action::Disconnect),
                    EditAction::None => {}
                }
                continue;
            }
            match key.code {
                KeyCode::Esc | KeyCode::Char('q' | 'Q') => return Ok(Action::Quit),
                KeyCode::Char('c' | 'C') => *editor = Some(Editor::new(profile)),
                KeyCode::Char('m' | 'M') => {
                    state.muted = !state.muted;
                    state.log(if state.muted {
                        "Microphone muted"
                    } else {
                        "Microphone live"
                    });
                }
                KeyCode::Up => state.gain = (state.gain + 0.05).min(1.0),
                KeyCode::Down => state.gain = (state.gain - 0.05).max(0.0),
                KeyCode::Left | KeyCode::Right => {
                    let current = if rate.automatic {
                        rate.maximum
                    } else {
                        rate.bitrate()
                    };
                    let target = if key.code == KeyCode::Left {
                        current.saturating_sub(1000).max(12000)
                    } else {
                        (current + 1000).min(32000)
                    };
                    if rate.automatic {
                        rate.set_maximum(target);
                    } else {
                        rate.set_fixed(target, Instant::now());
                    }
                    state.bitrate = rate.bitrate();
                    encoder
                        .set_bitrate(state.bitrate)
                        .map_err(io::Error::other)?;
                    state.log(format!(
                        "{}: {} kb/s",
                        if rate.automatic {
                            "Bitrate ceiling"
                        } else {
                            "Encoder"
                        },
                        target / 1000
                    ));
                }
                KeyCode::Char('r' | 'R') => {
                    if rate.automatic {
                        rate.set_fixed(rate.bitrate(), Instant::now());
                    } else {
                        rate.enable(Instant::now());
                    }
                    state.log(if rate.automatic {
                        "Adaptive bitrate enabled"
                    } else {
                        "Fixed bitrate enabled"
                    });
                }
                KeyCode::Char('a' | 'A') => {
                    state.aec = !state.aec;
                    rebuild = true;
                }
                KeyCode::Char('n' | 'N') => {
                    state.ns = !state.ns;
                    rebuild = true;
                }
                KeyCode::Char('g' | 'G') => {
                    state.agc = !state.agc;
                    rebuild = true;
                }
                _ => {}
            }
        }
    }
    if rebuild {
        *processing = Processing::new(state.aec, state.ns, state.agc);
        state.log(format!(
            "Voice engine restarted: AEC {} / NS {} / AGC {}",
            state.aec, state.ns, state.agc
        ));
    }
    Ok(Action::None)
}

fn run(options: Options) -> Result<(), Box<dyn Error>> {
    let running = Arc::new(AtomicBool::new(true));
    let signal = running.clone();
    ctrlc::set_handler(move || signal.store(false, Ordering::Relaxed))?;
    let mut terminal = if options.tui {
        ui::Terminal::enter()?
    } else {
        None
    };
    let mut profile = options.profile();
    let mut pending = if !profile.remote.is_empty() && options.role != 0 {
        Some(profile.clone())
    } else {
        None
    };
    if terminal.is_none() && pending.is_none() {
        return Err(io::Error::other("No interactive terminal. Specify --peer/--relay and --role, or start in a terminal to use the connection panel.").into());
    }
    let mut editor = pending.is_none().then(|| Editor::new(&profile));
    let mut connection: Option<Connection> = None;
    let mut state = ui::Dashboard::new("NO LINK", "Not bound".into(), "Press C to connect".into());
    state.bitrate = options.bitrate;
    state.adaptive = options.adaptive;
    state.bitrate_limit = options.max_bitrate;
    state.buffer_ms = options.buffer_ms;
    state.gain = options.gain;
    state.aec = options.aec;
    state.ns = options.ns;
    state.agc = options.agc;
    state.log("C opens the connection panel • direct IPv4 / IPv6 or relay");
    let mut encoder = Encoder::new(Config::new(options.bitrate)?);
    let mut processing = Processing::new(options.aec, options.ns, options.agc);
    let mut jitter = jitter::Jitter::new(options.buffer_ms);
    // Open devices only after an authenticated handshake. The dashboard is
    // already visible while the second client is being started.
    let mut audio: Option<audio::Audio> = None;
    if options.tone {
        state.input = "Synthetic tone / no microphone".into();
        state.output = "Decode check / no speakers".into();
    }
    let start = Instant::now();
    let mut rate = RateControl::new(
        options.bitrate,
        options.max_bitrate,
        options.adaptive,
        false,
        start,
    );
    let mut next_hello = start;
    let mut next_tone = start;
    let mut next_ui = start;
    let mut next_status = start;
    let mut tx_index = 0u64;
    let mut tone_sample = 0u64;
    let mut capture = [0.0; BLOCK];
    let mut reference = [0.0; BLOCK];
    let mut reference_filled = 0;
    let mut encoded_frame = [0.0; FRAME_SAMPLES];
    let mut capture_filled = 0;
    let mut output = [0.0; FRAME_SAMPLES];
    let mut packet = [0; MAX_PACKET_BYTES];
    let mut datagram = [0; transport::MAX_DATAGRAM + 1];

    let session_result = (|| -> Result<(), Box<dyn Error>> {
        while running.load(Ordering::Relaxed)
            && options
                .seconds
                .is_none_or(|s| start.elapsed().as_secs_f64() < s)
        {
            let now = Instant::now();
            let action = if terminal.is_some() {
                controls(
                    &mut state,
                    &mut encoder,
                    &mut rate,
                    &mut processing,
                    &mut editor,
                    &profile,
                )?
            } else {
                Action::None
            };
            match action {
                Action::Quit => break,
                Action::Connect(draft) => pending = Some(draft),
                Action::Disconnect => {
                    if let Some(mut old) = connection.take() {
                        let _ = old.send(transport::BYE, &[]);
                    }
                    audio = None;
                    jitter.reset();
                    state.active = false;
                    state.connected = false;
                    state.registered = false;
                    state.local_level = 0.0;
                    state.remote_level = 0.0;
                    state.queue_ms = 0;
                    state.log("Disconnected • C to choose another destination");
                    editor = None;
                }
                Action::None => {}
            }
            if let Some(draft) = pending.take() {
                // Validate syntax, address family and key before hanging up.
                // Release the old socket before binding the same local port.
                let connect_result = (|| -> io::Result<()> {
                    let key = draft.key_bytes()?;
                    let (local, remote) = draft.addresses()?;
                    if let Some(mut old) = connection.take() {
                        let _ = old.send(transport::BYE, &[]);
                    }
                    audio = None;
                    jitter = jitter::Jitter::new(options.buffer_ms);
                    encoder.reset();
                    processing = Processing::new(state.aec, state.ns, state.agc);
                    reference_filled = 0;
                    capture_filled = 0;
                    tx_index = 0;
                    tone_sample = 0;
                    next_hello = now;
                    next_tone = now;
                    state.reset_link();
                    rate.reset_link(remote.is_ipv6(), now);
                    encoder
                        .set_bitrate(rate.bitrate())
                        .map_err(io::Error::other)?;
                    let fresh = Connection::new(&key, draft.role, draft.direct, local, remote)?;
                    state.mode = draft.mode(remote).into();
                    state.local = fresh.socket.local_addr()?.to_string();
                    state.remote = remote.to_string();
                    state.active = true;
                    state.log(format!(
                        "Role {} • connecting to {} ({})",
                        draft.role, remote, state.mode
                    ));
                    connection = Some(fresh);
                    profile = draft.clone();
                    Ok(())
                })();
                match connect_result {
                    Ok(()) => editor = None,
                    Err(error) => {
                        if terminal.is_none() {
                            return Err(error.into());
                        }
                        let mut form = Editor::new(&draft);
                        form.error = error.to_string();
                        state.log(format!("Connection: {error}"));
                        editor = Some(form);
                    }
                }
                next_ui = now;
            }
            let tick_result = (|| -> Result<(), Box<dyn Error>> {
                let Some(connection) = &mut connection else {
                    return Ok(());
                };
                if now >= next_hello {
                    connection.send(transport::HELLO, &[])?;
                    next_hello = now + Duration::from_secs(1);
                }
                // Bound network work per tick so a busy socket cannot starve audio.
                for _ in 0..128 {
                    match connection.socket.recv(&mut datagram) {
                        Ok(length) => {
                            let received = connection.handle(&datagram[..length], now)?;
                            if connection.changed {
                                jitter.reset();
                                rate.reset_link(connection.socket.peer_addr()?.is_ipv6(), now);
                                state.log(if connection.peer.is_some() {
                                    "Peer authenticated — voice link established"
                                } else {
                                    "Peer disconnected — waiting"
                                });
                                connection.changed = false;
                            }
                            if let Some((index, bytes)) = received {
                                jitter.insert(index, &bytes, now);
                            }
                        }
                        Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
                        // An ICMP port-unreachable can mean the peer has not
                        // started yet. Winsock reports this as ConnectionReset:
                        // https://learn.microsoft.com/en-us/windows/win32/api/winsock/nf-winsock-recvfrom
                        Err(error)
                            if matches!(
                                error.kind(),
                                io::ErrorKind::ConnectionRefused | io::ErrorKind::ConnectionReset
                            ) =>
                        {
                            state.log("Peer port not open yet — retrying handshake");
                            break;
                        }
                        Err(error) => return Err(error.into()),
                    }
                }
                if connection.timed_out(now) {
                    return Err(io::Error::other("No authenticated response for 10 seconds. Check both addresses, distinct roles, matching keys and inbound UDP firewall rules.").into());
                }
                if connection.peer.is_some() {
                    if let Some((report, received_at)) = connection.feedback.take() {
                        rate.receive(report, received_at);
                    }
                    if let Some(token) = connection.probe.take()
                        && let Some(body) = rate.reply(
                            connection.peer.expect("checked peer"),
                            token,
                            (jitter.decoded + jitter.concealed) as u64,
                            jitter.concealed as u64,
                            now,
                        )
                    {
                        connection.send(transport::FEEDBACK, &body)?;
                    }
                    if let Some(body) = rate.probe(now) {
                        connection.send(transport::PROBE, &body)?;
                    }
                    rate.tick(now);
                    if rate.bitrate() != state.bitrate {
                        encoder.set_bitrate(rate.bitrate())?;
                        state.log(format!(
                            "Bitrate {} b/s • {:?}",
                            rate.bitrate(),
                            rate.reason()
                        ));
                    }
                }
                state.bitrate = rate.bitrate();
                state.adaptive = rate.automatic;
                state.bitrate_limit = rate.maximum;
                state.wire_bitrate = rate.wire_bitrate();
                state.rtt_ms = rate.rtt.map(|rtt| rtt.as_secs_f64() * 1_000.0);
                state.remote_loss = rate.loss_percent;
                state.rate_reason = rate.reason();
                if connection.registered && audio.is_none() && !options.tone {
                    // Jitter buffering is performed above the audio queue; only one
                    // frame of device prebuffer is needed here.
                    let device = audio::start(options.input, options.output, FRAME_SAMPLES, true)?;
                    state.input = device.input_name.clone();
                    state.output = device.output_name.clone();
                    state.log(format!(
                        "Device prebuffer: {} ms",
                        device.target.load(Ordering::Relaxed) * 1000 / SAMPLE_RATE as usize
                    ));
                    state.log(
                    "Capture and playback devices opened; AEC uses the playback callback reference",
                );
                    audio = Some(device);
                }
                if let Some(audio) = &mut audio {
                    if let Ok(error) = audio.errors.try_recv() {
                        return Err(error.into());
                    }
                    let rendered = audio.rendered.as_mut().expect("reference tap enabled");
                    while let Ok(sample) = rendered.pop() {
                        reference[reference_filled] = sample;
                        reference_filled += 1;
                        if reference_filled == BLOCK {
                            processing.render(&reference)?;
                            reference_filled = 0;
                        }
                    }
                }
                // Each 10 ms capture block is processed independently, then two
                // blocks form the codec's fixed 20 ms frame.
                for _ in 0..8 {
                    let available = if options.tone {
                        connection.registered && Instant::now() >= next_tone
                    } else {
                        audio.as_ref().is_some_and(|a| a.captured.slots() >= BLOCK)
                    };
                    if !available {
                        break;
                    }
                    if options.tone {
                        if Instant::now().duration_since(next_tone) > Duration::from_millis(100) {
                            next_tone = Instant::now();
                        }
                        for sample in &mut capture {
                            let frequency = if connection.role == 1 { 440.0 } else { 660.0 };
                            *sample = ((tone_sample as f32 * frequency / SAMPLE_RATE as f32)
                                * std::f32::consts::TAU)
                                .sin()
                                * 0.15;
                            tone_sample += 1;
                        }
                        // Keep an absolute sample clock; sleeping for 10 ms after
                        // every block would accumulate scheduler overhead as drift.
                        next_tone += Duration::from_millis(10);
                    } else {
                        let audio = audio.as_mut().expect("capture available");
                        for sample in &mut capture {
                            *sample = audio.captured.pop().expect("checked slots");
                        }
                    }
                    state.local_level = level(&capture);
                    processing.capture(
                        &capture,
                        &mut encoded_frame[capture_filled..capture_filled + BLOCK],
                    )?;
                    if state.muted {
                        encoded_frame[capture_filled..capture_filled + BLOCK].fill(0.0);
                    }
                    capture_filled += BLOCK;
                    if capture_filled == FRAME_SAMPLES {
                        let length = encoder.encode(&encoded_frame, &mut packet)?;
                        let mut body = Vec::from(tx_index.to_be_bytes());
                        body.extend_from_slice(&packet[..length]);
                        connection.send(transport::AUDIO, &body)?;
                        tx_index = tx_index
                            .checked_add(1)
                            .ok_or_else(|| io::Error::other("Frame counter exhausted"))?;
                        capture_filled = 0;
                    }
                }
                if jitter.poll(now, &mut output) {
                    for sample in &mut output {
                        *sample = (*sample * state.gain).clamp(-1.0, 1.0);
                    }
                    state.remote_level = level(&output);
                    if let Some(audio) = &mut audio {
                        // A large queue means device/worker clock drift or a stall.
                        // Drop whole frames instead of accumulating latency.
                        if audio.playback.slots() >= FRAME_SAMPLES
                            && audio.playback.buffer().capacity() - audio.playback.slots()
                                <= FRAME_SAMPLES * 4
                        {
                            for &sample in &output {
                                audio.playback.push(sample).expect("checked slots");
                            }
                        } else {
                            audio
                                .stats
                                .playback_drops
                                .fetch_add(FRAME_SAMPLES, Ordering::Relaxed);
                        }
                    } else if options.tone {
                        for block in output.as_chunks::<BLOCK>().0 {
                            processing.render(block)?;
                        }
                    }
                }
                state.connected = connection.peer.is_some();
                state.registered = connection.registered;
                state.seconds = start.elapsed().as_secs_f64();
                state.tx = connection.sent;
                state.rx = connection.received;
                state.decoded = jitter.decoded;
                state.concealed = jitter.concealed;
                state.rejected = connection.rejected + jitter.dropped;
                if let Some(audio) = &audio {
                    state.queue_ms = (audio.playback.buffer().capacity() - audio.playback.slots())
                        * 1000
                        / SAMPLE_RATE as usize;
                    state.gaps = audio.stats.playback_gaps.load(Ordering::Relaxed);
                    state.drops = audio.stats.capture_drops.load(Ordering::Relaxed)
                        + audio.stats.playback_drops.load(Ordering::Relaxed)
                        + audio.stats.reference_drops.load(Ordering::Relaxed);
                }
                Ok(())
            })();
            if let Err(error) = tick_result {
                if terminal.is_none() {
                    return Err(error);
                }
                if let Some(mut old) = connection.take() {
                    let _ = old.send(transport::BYE, &[]);
                }
                audio = None;
                jitter.reset();
                state.active = false;
                state.connected = false;
                state.registered = false;
                state.local_level = 0.0;
                state.remote_level = 0.0;
                state.queue_ms = 0;
                state.log(format!("Call stopped: {error}"));
                editor.get_or_insert_with(|| Editor::new(&profile)).error = error.to_string();
            }
            state.seconds = start.elapsed().as_secs_f64();
            if now >= next_ui {
                state.sample();
                if let Some(terminal) = &mut terminal {
                    terminal.draw(&state, editor.as_ref())?;
                }
                state.local_level *= 0.8;
                state.remote_level *= 0.8;
                next_ui = now + Duration::from_millis(80);
            }
            if terminal.is_none() && now >= next_status {
                println!(
                    "{} {} TX={} RX={} decoded={} PLC={} rejected={} peer_peak={:.3} bitrate={} rate={} wire_bps={} rtt_ms={} remote_loss={:.1}% reason={:?}",
                    state.mode,
                    if state.connected {
                        "connected"
                    } else {
                        "waiting"
                    },
                    state.tx,
                    state.rx,
                    state.decoded,
                    state.concealed,
                    state.rejected,
                    state.remote_level,
                    state.bitrate,
                    if state.adaptive { "auto" } else { "fixed" },
                    state.wire_bitrate,
                    state
                        .rtt_ms
                        .map_or_else(|| "unknown".into(), |rtt| format!("{rtt:.1}")),
                    state.remote_loss,
                    state.rate_reason
                );
                next_status = now + Duration::from_secs(1);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    })();
    // Best-effort leave even after audio/network errors, then restore the terminal.
    if let Some(mut connection) = connection {
        let _ = connection.send(transport::BYE, &[]);
    }
    drop(audio);
    drop(terminal);
    println!(
        "Call ended: TX={} RX={} decoded={} PLC={} rejected={}",
        state.tx, state.rx, state.decoded, state.concealed, state.rejected
    );
    session_result
}

fn main() -> Result<(), Box<dyn Error>> {
    if let Some(options) = options(std::env::args().skip(1))? {
        run(options)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(args: &[&str]) -> Result<Option<Options>, Box<dyn Error>> {
        options(args.iter().map(|s| (*s).to_owned()))
    }
    #[test]
    fn validates_connection_modes_and_audio_parameters_before_opening_devices() {
        let valid = ["--peer", "[::1]:5001", "--role", "1"];
        let direct = parse(&valid).unwrap().unwrap();
        assert!(direct.direct);
        assert!(direct.adaptive);
        assert_eq!(direct.max_bitrate, 32_000);
        assert!(!parse(&["--fixed-bitrate"]).unwrap().unwrap().adaptive);
        assert_eq!(
            direct.profile().addresses().unwrap().0,
            "[::]:5000".parse().unwrap()
        );
        let relay = parse(&["--relay", "127.0.0.1:5000", "--role", "2"])
            .unwrap()
            .unwrap();
        assert!(!relay.direct);
        assert_eq!(relay.profile().addresses().unwrap().0.port(), 0);
        assert!(parse(&[]).unwrap().unwrap().remote.is_empty());
        let ipv4 = parse(&["--peer", "localhost:5000", "--role", "1", "--ipv4"])
            .unwrap()
            .unwrap();
        assert!(ipv4.profile().addresses().unwrap().1.is_ipv4());
        for args in [
            vec!["--no-tui"],
            vec!["--peer", "[::1]:5001", "--no-tui"],
            vec!["--ipv4", "--ipv6"],
            vec!["--max-bitrate", "11999"],
            vec!["--max-bitrate", "33000"],
            vec!["--bitrate", "24000", "--max-bitrate", "16000"],
            vec![
                "--peer",
                "[::1]:5001",
                "--relay",
                "localhost:5000",
                "--role",
                "1",
            ],
            vec!["--peer", "[::1]:5001", "--role", "3"],
            vec!["--peer", "[::1]:5001", "--role", "1", "--gain", "NaN"],
            vec!["--peer", "[::1]:5001", "--role", "1", "--buffer-ms", "1000"],
            vec!["--peer", "[::1]:5001", "--role", "1", "--seconds", "inf"],
        ] {
            assert!(parse(&args).is_err());
        }
    }
}
