// SPDX-License-Identifier: MIT
//! Two-party Snacka calls, directly over IPv6/IPv4 or through a UDP relay.
#[path = "../monitor/audio.rs"]
mod audio;
mod connection;
mod jitter;
mod processing;
#[path = "../monitor/resample.rs"]
mod resample;
#[path = "../call_common/transport.rs"]
mod transport;
mod ui;

use connection::Connection;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use processing::{BLOCK, Processing};
use snacka::{Config, Encoder, FRAME_SAMPLES, MAX_PACKET_BYTES, SAMPLE_RATE};
use std::{
    error::Error,
    io,
    net::{SocketAddr, ToSocketAddrs},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const HELP: &str = "Snacka / Voice Link — encrypted two-person call with a live TUI

cargo run --release --features internet-call --example call -- [OPTIONS]

Choose one connection mode:
  --peer ADDRESS     Direct UDP to peer, e.g. [2001:db8::2]:5000 (no relay)
  --relay ADDRESS    Outbound UDP to a call_relay server, e.g. host:5000
  --bind ADDRESS     Local address; direct default [::]:5000 or 0.0.0.0:5000;
                     relay default ephemeral port in the remote address family
  --role 1|2         Use different roles on the two clients (required)

Set the same SNACKA_CALL_KEY (64 hex digits) on both clients and any relay.
Direct internet IPv6 needs global addresses and inbound UDP firewall rules.
  --generate-key     Generate a random room key and exit
  --bitrate BPS      12000..32000 (default 24000), fixed 20 ms frames
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
      G auto gain; Q, Esc or Ctrl-C hang up. No audio is saved.";

struct Options {
    remote: String,
    direct: bool,
    bind: Option<String>,
    role: u8,
    bitrate: u32,
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
        bind: None,
        role: 0,
        bitrate: 24000,
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
            "--no-tui" => options.tui = false,
            "--test-tone" => options.tone = true,
            "--peer" | "--relay" | "--bind" | "--role" | "--bitrate" | "--buffer-ms" | "--gain"
            | "--input" | "--output" | "--seconds" => {
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
    if options.remote.is_empty() || !(1..=2).contains(&options.role) {
        return Err(io::Error::other(
            "Specify --peer ADDRESS or --relay ADDRESS, and --role 1 or 2; use --help",
        )
        .into());
    }
    Config::new(options.bitrate)?;
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

fn addresses(options: &Options) -> io::Result<(SocketAddr, SocketAddr)> {
    let explicit = options
        .bind
        .as_ref()
        .map(|s| s.parse::<SocketAddr>())
        .transpose()
        .map_err(io::Error::other)?;
    let remote = options
        .remote
        .to_socket_addrs()?
        .find(|r| explicit.is_none_or(|l| r.is_ipv6() == l.is_ipv6()))
        .ok_or_else(|| io::Error::other("No remote address matches --bind address family"))?;
    let local = explicit.unwrap_or_else(|| {
        let port = if options.direct { 5000 } else { 0 };
        if remote.is_ipv6() {
            SocketAddr::from(([0u16; 8], port))
        } else {
            SocketAddr::from(([0u8; 4], port))
        }
    });
    Ok((local, remote))
}

fn level(samples: &[f32]) -> f32 {
    samples.iter().fold(0.0f32, |peak, s| peak.max(s.abs()))
}

fn controls(
    state: &mut ui::Dashboard,
    encoder: &mut Encoder,
    processing: &mut Processing,
) -> io::Result<bool> {
    let mut rebuild = false;
    while event::poll(Duration::ZERO)? {
        if let Event::Key(key) = event::read()?
            && key.kind != KeyEventKind::Release
        {
            match key.code {
                KeyCode::Esc | KeyCode::Char('q' | 'Q') => return Ok(false),
                KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                    return Ok(false);
                }
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
                    state.bitrate = if key.code == KeyCode::Left {
                        state.bitrate.saturating_sub(1000).max(12000)
                    } else {
                        (state.bitrate + 1000).min(32000)
                    };
                    encoder
                        .set_bitrate(state.bitrate)
                        .map_err(io::Error::other)?;
                    state.log(format!("Encoder: {} kb/s", state.bitrate / 1000));
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
    Ok(true)
}

fn run(options: Options) -> Result<(), Box<dyn Error>> {
    let (local, remote) = addresses(&options)?;
    let mut connection = Connection::new(
        &transport::key_from_env()?,
        options.role,
        options.direct,
        local,
        remote,
    )?;
    let running = Arc::new(AtomicBool::new(true));
    let signal = running.clone();
    ctrlc::set_handler(move || signal.store(false, Ordering::Relaxed))?;
    let mut terminal = if options.tui {
        ui::Terminal::enter()?
    } else {
        None
    };
    let mode = if options.direct {
        if remote.is_ipv6() {
            "DIRECT IPv6"
        } else {
            "DIRECT IPv4"
        }
    } else {
        "UDP RELAY"
    };
    let mut state = ui::Dashboard::new(
        mode,
        connection.socket.local_addr()?.to_string(),
        remote.to_string(),
    );
    state.bitrate = options.bitrate;
    state.buffer_ms = options.buffer_ms;
    state.gain = options.gain;
    state.aec = options.aec;
    state.ns = options.ns;
    state.agc = options.agc;
    state.log(format!("Role {} • authenticating peer", options.role));
    state.log("Shared room key • authenticated, encrypted UDP");
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
                    // Some OSes deliver a prior UDP port-unreachable here.
                    Err(error) if error.kind() == io::ErrorKind::ConnectionRefused => {
                        state.log("Peer port not open yet — retrying handshake");
                        break;
                    }
                    Err(error) => return Err(error.into()),
                }
            }
            if connection.timed_out(now) {
                return Err(io::Error::other("No authenticated response for 10 seconds. Check both addresses, distinct roles, matching keys and inbound UDP firewall rules.").into());
            }
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
                        let frequency = if options.role == 1 { 440.0 } else { 660.0 };
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
            if terminal.is_some() && !controls(&mut state, &mut encoder, &mut processing)? {
                break;
            }
            if now >= next_ui {
                state.sample();
                if let Some(terminal) = &mut terminal {
                    terminal.draw(&state)?;
                }
                state.local_level *= 0.8;
                state.remote_level *= 0.8;
                next_ui = now + Duration::from_millis(80);
            }
            if terminal.is_none() && now >= next_status {
                println!(
                    "{mode} {} TX={} RX={} decoded={} PLC={} rejected={} peer_peak={:.3}",
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
                    state.remote_level
                );
                next_status = now + Duration::from_secs(1);
            }
            std::thread::sleep(Duration::from_millis(1));
        }
        Ok(())
    })();
    // Best-effort leave even after audio/network errors, then restore the terminal.
    let _ = connection.send(transport::BYE, &[]);
    drop(audio);
    drop(terminal);
    println!(
        "Call ended: TX={} RX={} decoded={} PLC={} rejected={}",
        connection.sent,
        connection.received,
        jitter.decoded,
        jitter.concealed,
        connection.rejected + jitter.dropped
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
        assert_eq!(addresses(&direct).unwrap().0, "[::]:5000".parse().unwrap());
        let relay = parse(&["--relay", "127.0.0.1:5000", "--role", "2"])
            .unwrap()
            .unwrap();
        assert!(!relay.direct);
        assert_eq!(addresses(&relay).unwrap().0.port(), 0);
        for args in [
            vec![],
            vec!["--peer", "[::1]:5001"],
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
