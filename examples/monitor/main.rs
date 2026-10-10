// SPDX-License-Identifier: MIT
//! Microphone -> Snacka packets -> decoded headphone audio, with live A/B controls.
mod audio;
mod processor;
mod resample;
mod ui;

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use processor::Processor;
use snacka::{Config, FRAME_SAMPLES, FrameDuration, SAMPLE_RATE};
use std::{
    error::Error,
    io::{self, IsTerminal},
    sync::atomic::Ordering,
    time::{Duration, Instant},
};

const HELP: &str = "Snacka microphone monitor (use headphones at low volume)

cargo run --release --features live-audio --example monitor -- [OPTIONS]

  --bitrate BPS       12000..32000 (default 24000)
  --frame-ms MS       10 or 20 (default 20)
  --buffer-ms MS      Playback prebuffer, 20..200 (default 40)
  --gain GAIN         Playback gain, 0..2 (default 0.5)
  --raw               Start with delayed unencoded audio for comparison
  --list-devices      Print input/output device indices and exit
  --input INDEX       Capture device (default: system default)
  --output INDEX      Playback device (default: system default)
  --seconds SECONDS  Stop automatically after this duration
  --no-tui           Run without interactive terminal controls
  --help             Print this help

Keys: Left/Right or -/+ bitrate; Up/Down volume; Space A/B; F frame size;
      B buffer; M mute; ? help; Q, Esc or Ctrl-C quit. Changes apply to subsequent frames.
The frame-size key restarts the codec stream. No audio is saved to disk.";

struct Options {
    config: Config,
    buffer_ms: usize,
    gain: f32,
    raw: bool,
    input: Option<usize>,
    output: Option<usize>,
    seconds: Option<f64>,
    tui: bool,
}

fn options() -> Result<Option<Options>, Box<dyn Error>> {
    let mut options = Options {
        config: Config::new(24_000)?,
        buffer_ms: 40,
        gain: 0.5,
        raw: false,
        input: None,
        output: None,
        seconds: None,
        tui: true,
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--help" | "-h" => {
                println!("{HELP}");
                return Ok(None);
            }
            "--list-devices" => {
                audio::list_devices()?;
                return Ok(None);
            }
            "--raw" => options.raw = true,
            "--no-tui" => options.tui = false,
            "--bitrate" | "--frame-ms" | "--buffer-ms" | "--gain" | "--input" | "--output"
            | "--seconds" => {
                let value = arguments
                    .next()
                    .ok_or_else(|| io::Error::other(format!("Missing value for {argument}")))?;
                match argument.as_str() {
                    "--bitrate" => {
                        options.config = Config::new(value.parse()?)?
                            .with_frame_duration(options.config.frame_duration())
                    }
                    "--frame-ms" => {
                        options.config = options.config.with_frame_duration(match value.as_str() {
                            "10" => FrameDuration::Ms10,
                            "20" => FrameDuration::Ms20,
                            _ => return Err(io::Error::other("--frame-ms must be 10 or 20").into()),
                        })
                    }
                    "--buffer-ms" => options.buffer_ms = value.parse()?,
                    "--gain" => options.gain = value.parse()?,
                    "--input" => options.input = Some(value.parse()?),
                    "--output" => options.output = Some(value.parse()?),
                    "--seconds" => options.seconds = Some(value.parse()?),
                    _ => unreachable!(),
                }
            }
            _ => {
                return Err(
                    io::Error::other(format!("Unknown option {argument}; use --help")).into(),
                );
            }
        }
    }
    if !(20..=200).contains(&options.buffer_ms) {
        return Err(io::Error::other("--buffer-ms must be between 20 and 200").into());
    }
    if !options.gain.is_finite() || !(0.0..=2.0).contains(&options.gain) {
        return Err(io::Error::other("--gain must be finite and between 0 and 2").into());
    }
    if options.seconds.is_some_and(|s| !s.is_finite() || s <= 0.0) {
        return Err(io::Error::other("--seconds must be finite and positive").into());
    }
    Ok(Some(options))
}

struct Terminal(ratatui::DefaultTerminal);

impl Terminal {
    fn enter() -> io::Result<Self> {
        if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
            return Err(io::Error::other(
                "The TUI requires a terminal; use --no-tui --seconds 10 for a timed run",
            ));
        }
        match ratatui::try_init() {
            Ok(terminal) => Ok(Self(terminal)),
            Err(error) => {
                ratatui::restore();
                Err(error)
            }
        }
    }
}

impl Drop for Terminal {
    fn drop(&mut self) {
        ratatui::restore();
    }
}

fn run(mut options: Options) -> Result<(), Box<dyn Error>> {
    // Construct plans before opening capture or playback.
    let mut processor = Processor::new(options.config, options.raw);
    let mut terminal = if options.tui {
        Some(Terminal::enter()?)
    } else {
        None
    };
    let target = options.buffer_ms * SAMPLE_RATE as usize / 1000;
    let mut audio = audio::start(options.input, options.output, target)?;
    if !options.tui {
        println!(
            "{} -> Snacka {} b/s, {} ms -> {}. Use headphones. Ctrl-C to stop.",
            audio.input_name,
            options.config.bitrate(),
            options.config.frame_duration().milliseconds(),
            audio.output_name
        );
    }
    let start = Instant::now();
    let mut last_draw = start - Duration::from_secs(1);
    let mut frame = [0.0; FRAME_SAMPLES];
    let mut decoded = [0.0; FRAME_SAMPLES];
    let mut ui = ui::Ui::default();
    loop {
        if options
            .seconds
            .is_some_and(|s| start.elapsed().as_secs_f64() >= s)
        {
            break;
        }
        if let Ok(error) = audio.errors.try_recv() {
            match error.kind() {
                cpal::ErrorKind::Xrun
                | cpal::ErrorKind::RealtimeDenied
                | cpal::ErrorKind::DeviceChanged => {
                    ui.notice = error.to_string();
                    if !options.tui {
                        eprintln!("{error}");
                    }
                }
                _ => return Err(error.into()),
            }
        }
        if options.tui && event::poll(Duration::ZERO)? {
            match event::read()? {
                Event::Key(key) if key.kind != KeyEventKind::Release => match key.code {
                    KeyCode::Esc if ui.help => ui.help = false,
                    KeyCode::Char('q' | 'Q') | KeyCode::Esc => break,
                    KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => break,
                    KeyCode::Right | KeyCode::Char('+' | '=') => {
                        processor.set_bitrate((processor.config.bitrate() + 1000).min(32_000))?
                    }
                    KeyCode::Left | KeyCode::Char('-') => processor
                        .set_bitrate(processor.config.bitrate().saturating_sub(1000).max(12_000))?,
                    KeyCode::Up => options.gain = (options.gain + 0.05).min(2.0),
                    KeyCode::Down => options.gain = (options.gain - 0.05).max(0.0),
                    KeyCode::Char(' ') => {
                        processor.raw = !processor.raw;
                        ui.notice = if processor.raw {
                            "Listening to raw microphone audio with matching transform delay."
                        } else {
                            "Listening through the Snacka encoder and decoder."
                        }
                        .into();
                    }
                    KeyCode::Char('f' | 'F') => {
                        processor.toggle_duration();
                        ui.notice =
                            "Frame profile changed; encoder and decoder restarted together.".into();
                    }
                    KeyCode::Char('b' | 'B') => {
                        options.buffer_ms = match options.buffer_ms {
                            0..=19 => 20,
                            20..=39 => 40,
                            40..=79 => 80,
                            80..=119 => 120,
                            120..=199 => 200,
                            _ => 20,
                        };
                        audio.target.store(
                            options.buffer_ms * SAMPLE_RATE as usize / 1000,
                            Ordering::Relaxed,
                        );
                        ui.notice = format!(
                            "Playback buffer changed to {} ms; briefly rebuffering.",
                            options.buffer_ms
                        );
                    }
                    KeyCode::Char('?') => ui.help = !ui.help,
                    KeyCode::Char('m' | 'M') => {
                        ui.muted = !ui.muted;
                        ui.notice = if ui.muted {
                            "Playback muted. Microphone capture continues."
                        } else {
                            "Playback unmuted."
                        }
                        .into();
                    }
                    _ => {}
                },
                Event::Resize(..) => last_draw = start - Duration::from_secs(1),
                _ => {}
            }
        }
        let samples = processor.config.frame_samples();
        if audio.captured.slots() >= samples {
            for sample in &mut frame[..samples] {
                *sample = audio
                    .captured
                    .pop()
                    .expect("slots checked; this is the only consumer");
            }
            let frame_peak = frame[..samples]
                .iter()
                .map(|s| s.abs())
                .fold(0.0_f32, f32::max);
            ui.peak = frame_peak.max(ui.peak);
            processor.process(&frame[..samples], &mut decoded[..samples])?;
            let queued = audio.playback.buffer().capacity() - audio.playback.slots();
            if queued + samples <= audio.target.load(Ordering::Relaxed) * 2 {
                for &sample in &decoded[..samples] {
                    let sample = if ui.muted {
                        0.0
                    } else {
                        (sample * options.gain).clamp(-1.0, 1.0)
                    };
                    audio
                        .playback
                        .push(sample)
                        .expect("slots checked; this is the only producer");
                }
            } else {
                // Drop a complete decoded frame rather than grow the latency indefinitely.
                audio
                    .stats
                    .playback_drops
                    .fetch_add(samples, Ordering::Relaxed);
            }
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
        if let Some(terminal) = terminal.as_mut()
            && last_draw.elapsed() >= Duration::from_millis(100)
        {
            let queued = audio.playback.buffer().capacity() - audio.playback.slots();
            let view = ui::View {
                config: processor.config,
                raw: processor.raw,
                gain: options.gain,
                buffer_ms: options.buffer_ms,
                input: &audio.input_name,
                output: &audio.output_name,
                queued_ms: queued as f64 / f64::from(SAMPLE_RATE) * 1000.0,
                capture_drops: audio.stats.capture_drops.load(Ordering::Relaxed),
                playback_gaps: audio.stats.playback_gaps.load(Ordering::Relaxed),
                playback_drops: audio.stats.playback_drops.load(Ordering::Relaxed),
                clipped: audio.stats.clipped_input.load(Ordering::Relaxed),
                elapsed: start.elapsed().as_secs_f64(),
                encoded_bytes: processor.bytes,
                samples: processor.samples,
            };
            terminal.0.draw(|frame| ui.render(frame, &view))?;
            ui.tick();
            last_draw = Instant::now();
        }
    }
    let playback_drops = audio.stats.playback_drops.load(Ordering::Relaxed);
    let capture_drops = audio.stats.capture_drops.load(Ordering::Relaxed);
    let playback_gaps = audio.stats.playback_gaps.load(Ordering::Relaxed);
    drop(audio);
    drop(terminal);
    println!(
        "Stopped after {:.1}s; processed {} samples, {} encoded bytes; capture drops: {}, playback gaps: {}, output drops: {} samples.",
        start.elapsed().as_secs_f64(),
        processor.samples,
        processor.bytes,
        capture_drops,
        playback_gaps,
        playback_drops
    );
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    if let Some(options) = options()? {
        run(options)?;
    }
    Ok(())
}
