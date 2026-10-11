// SPDX-License-Identifier: MIT
use super::resample::Converter;
use cpal::{
    Device, FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig,
    SupportedStreamConfigRange,
    traits::{DeviceTrait, HostTrait, StreamTrait},
};
use rtrb::{Consumer, Producer, RingBuffer};
use snacka::{FRAME_SAMPLES, SAMPLE_RATE};
use std::{
    error::Error,
    io,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
};

#[derive(Default)]
pub struct Stats {
    pub capture_drops: AtomicUsize,
    pub playback_gaps: AtomicUsize,
    pub clipped_input: AtomicUsize,
    pub playback_drops: AtomicUsize,
    pub reference_drops: AtomicUsize,
}

pub struct Audio {
    pub captured: Consumer<f32>,
    pub playback: Producer<f32>,
    // Used by the call example; the monitor does not enable the reference tap.
    #[allow(dead_code)]
    pub rendered: Option<Consumer<f32>>,
    pub stats: Arc<Stats>,
    pub errors: mpsc::Receiver<cpal::Error>,
    pub input_name: String,
    pub output_name: String,
    pub target: Arc<AtomicUsize>,
    // Streams must outlive the loop and are stopped by dropping Audio.
    _input: Stream,
    _output: Stream,
}

fn name(device: &Device) -> String {
    device
        .description()
        .map(|d| d.name().to_owned())
        .unwrap_or_else(|_| device.to_string())
}

pub fn list_devices() -> Result<(), Box<dyn Error>> {
    let host = cpal::default_host();
    println!("Input devices (--input INDEX):");
    for (index, device) in host.input_devices()?.enumerate() {
        print_device(index, &device, true);
    }
    println!("Output devices (--output INDEX):");
    for (index, device) in host.output_devices()?.enumerate() {
        print_device(index, &device, false);
    }
    Ok(())
}

fn print_device(index: usize, device: &Device, input: bool) {
    match select_config(device, input) {
        Ok(config) => println!(
            "  {index}: {} ({} Hz, {} channels, {})",
            name(device),
            config.sample_rate(),
            config.channels(),
            config.sample_format()
        ),
        Err(error) => println!("  {index}: {} ({error})", name(device)),
    }
}

fn best_config(
    ranges: impl IntoIterator<Item = SupportedStreamConfigRange>,
) -> Option<SupportedStreamConfig> {
    ranges
        .into_iter()
        .filter(|r| {
            r.channels() > 0 && r.min_sample_rate() > 0 && supported_format(r.sample_format())
        })
        .map(|r| {
            let rate = SAMPLE_RATE.clamp(r.min_sample_rate(), r.max_sample_rate());
            r.with_sample_rate(rate)
        })
        .min_by_key(|c| {
            (
                c.sample_rate().abs_diff(SAMPLE_RATE),
                c.sample_format() != SampleFormat::F32,
                c.channels(),
            )
        })
}

fn select_config(device: &Device, input: bool) -> Result<SupportedStreamConfig, Box<dyn Error>> {
    // Keep the device's current/default format, especially for Bluetooth devices
    // whose capture and playback profiles share a hardware clock.
    let default = if input {
        device.default_input_config()
    } else {
        device.default_output_config()
    };
    if let Ok(config) = default
        && config.channels() > 0
        && config.sample_rate() > 0
        && supported_format(config.sample_format())
    {
        return Ok(config);
    }
    let ranges: Vec<_> = if input {
        device.supported_input_configs()?.collect()
    } else {
        device.supported_output_configs()?.collect()
    };
    // Prefer 48 kHz; otherwise convert from/to the nearest supported rate.
    best_config(ranges).ok_or_else(|| {
        io::Error::other(format!(
            "{} has no supported PCM {} configuration. Select another device with --list-devices.",
            name(device),
            if input { "input" } else { "output" }
        ))
        .into()
    })
}

fn supported_format(format: SampleFormat) -> bool {
    matches!(
        format,
        SampleFormat::F32
            | SampleFormat::F64
            | SampleFormat::I8
            | SampleFormat::I16
            | SampleFormat::I32
            | SampleFormat::I64
            | SampleFormat::U8
            | SampleFormat::U16
            | SampleFormat::U32
            | SampleFormat::U64
    )
}

fn capture<T: SizedSample>(
    device: &Device,
    config: SupportedStreamConfig,
    mut producer: Producer<f32>,
    stats: Arc<Stats>,
    errors: mpsc::Sender<cpal::Error>,
    mut converter: Converter,
) -> Result<Stream, cpal::Error>
where
    f32: FromSample<T>,
{
    let channels = usize::from(config.channels());
    device.build_input_stream(
        config.config(),
        move |data: &[T], _| {
            let mut dropped = 0;
            let mut clipped = 0;
            for frame in data.chunks_exact(channels) {
                let mono =
                    frame.iter().map(|&s| f32::from_sample(s)).sum::<f32>() / channels as f32;
                let sample = if mono.is_finite() {
                    mono.clamp(-1.0, 1.0)
                } else {
                    0.0
                };
                clipped += usize::from(!mono.is_finite() || mono.abs() >= 1.0);
                for &sample in converter.push(sample) {
                    dropped += usize::from(producer.push(sample.clamp(-1.0, 1.0)).is_err());
                }
            }
            stats.capture_drops.fetch_add(dropped, Ordering::Relaxed);
            stats.clipped_input.fetch_add(clipped, Ordering::Relaxed);
        },
        move |error| {
            let _ = errors.send(error);
        },
        None,
    )
}

struct RenderReference {
    producer: Producer<f32>,
    converter: Converter,
}

impl RenderReference {
    fn push(&mut self, sample: f32, stats: &Stats) {
        let mut dropped = 0;
        for &sample in self.converter.push(sample) {
            dropped += usize::from(self.producer.push(sample).is_err());
        }
        stats.reference_drops.fetch_add(dropped, Ordering::Relaxed);
    }
}

#[allow(clippy::too_many_arguments)] // Callback state is constructed before opening audio.
fn play<T: SizedSample + FromSample<f32>>(
    device: &Device,
    config: SupportedStreamConfig,
    mut consumer: Consumer<f32>,
    stats: Arc<Stats>,
    errors: mpsc::Sender<cpal::Error>,
    target: Arc<AtomicUsize>,
    mut converter: Converter,
    mut reference: Option<RenderReference>,
) -> Result<Stream, cpal::Error> {
    let channels = usize::from(config.channels());
    let mut primed = false;
    let mut previous_target = target.load(Ordering::Relaxed);
    device.build_output_stream(
        config.config(),
        move |data: &mut [T], _| {
            let target = target.load(Ordering::Relaxed);
            if target != previous_target {
                // Re-prime on increases; discard stale queued audio on decreases.
                primed = false;
                converter.reset();
                previous_target = target;
                let discard = consumer.slots().saturating_sub(target);
                for _ in 0..discard {
                    let _ = consumer.pop();
                }
                stats.playback_drops.fetch_add(discard, Ordering::Relaxed);
            }
            if !primed {
                if consumer.slots() < target {
                    data.fill(T::EQUILIBRIUM);
                    if let Some(reference) = &mut reference {
                        for _ in data.chunks_exact(channels) {
                            reference.push(0.0, &stats);
                        }
                    }
                    return;
                }
                primed = true;
            }
            let mut gaps = 0;
            for frame in data.chunks_exact_mut(channels) {
                let sample = converter.next(|| {
                    consumer.pop().unwrap_or_else(|_| {
                        primed = false;
                        gaps += 1;
                        0.0
                    })
                });
                frame.fill(T::from_sample(sample));
                if let Some(reference) = &mut reference {
                    // Tap the device-rate signal actually handed to playback,
                    // including gain, resampling, priming silence and underruns.
                    reference.push(sample, &stats);
                }
            }
            if !primed {
                // Re-priming must not retain a partially played conversion block.
                converter.reset();
            }
            stats.playback_gaps.fetch_add(gaps, Ordering::Relaxed);
        },
        move |error| {
            let _ = errors.send(error);
        },
        None,
    )
}

// Dispatch each device independently, including common integer PCM devices.
macro_rules! stream {
    ($function:ident, $device:expr, $config:expr, $($argument:expr),*) => {
        match $config.sample_format() {
            SampleFormat::F32 => $function::<f32>($device, $config, $($argument),*),
            SampleFormat::F64 => $function::<f64>($device, $config, $($argument),*),
            SampleFormat::I8 => $function::<i8>($device, $config, $($argument),*),
            SampleFormat::I16 => $function::<i16>($device, $config, $($argument),*),
            SampleFormat::I32 => $function::<i32>($device, $config, $($argument),*),
            SampleFormat::I64 => $function::<i64>($device, $config, $($argument),*),
            SampleFormat::U8 => $function::<u8>($device, $config, $($argument),*),
            SampleFormat::U16 => $function::<u16>($device, $config, $($argument),*),
            SampleFormat::U32 => $function::<u32>($device, $config, $($argument),*),
            SampleFormat::U64 => $function::<u64>($device, $config, $($argument),*),
            _ => unreachable!("select_config filters unsupported formats"),
        }
    };
}

pub fn start(
    input: Option<usize>,
    output: Option<usize>,
    target: usize,
    with_reference: bool,
) -> Result<Audio, Box<dyn Error>> {
    let host = cpal::default_host();
    let input = match input {
        Some(index) => host.input_devices()?.nth(index),
        None => host.default_input_device(),
    }
    .ok_or_else(|| io::Error::other("Input device not found; use --list-devices"))?;
    let output = match output {
        Some(index) => host.output_devices()?.nth(index),
        None => host.default_output_device(),
    }
    .ok_or_else(|| io::Error::other("Output device not found; use --list-devices"))?;
    let input_config = select_config(&input, true)?;
    let output_config = select_config(&output, false)?;
    let input_name = format!("{} ({} Hz)", name(&input), input_config.sample_rate());
    let output_name = format!("{} ({} Hz)", name(&output), output_config.sample_rate());
    let capture_converter = Converter::new(input_config.sample_rate(), SAMPLE_RATE)?;
    let playback_converter = Converter::new(SAMPLE_RATE, output_config.sample_rate())?;
    let (capture_producer, captured) = RingBuffer::new(FRAME_SAMPLES * 4);
    // Allocate once for the largest live buffer choice (200 ms, with headroom).
    let (playback, playback_consumer) = RingBuffer::new(SAMPLE_RATE as usize * 400 / 1000);
    let (reference, rendered) = if with_reference {
        let (producer, consumer) = RingBuffer::new(SAMPLE_RATE as usize * 400 / 1000);
        (
            Some(RenderReference {
                producer,
                converter: Converter::new(output_config.sample_rate(), SAMPLE_RATE)?,
            }),
            Some(consumer),
        )
    } else {
        (None, None)
    };
    let target = Arc::new(AtomicUsize::new(target));
    let stats = Arc::new(Stats::default());
    let (error_tx, errors) = mpsc::channel();
    let input_stream = stream!(
        capture,
        &input,
        input_config,
        capture_producer,
        stats.clone(),
        error_tx.clone(),
        capture_converter
    )
    .map_err(|error| io::Error::other(format!("Could not open input {input_name}: {error}")))?;
    let output_stream = stream!(
        play,
        &output,
        output_config,
        playback_consumer,
        stats.clone(),
        error_tx,
        target.clone(),
        playback_converter,
        reference
    )
    .map_err(|error| io::Error::other(format!("Could not open output {output_name}: {error}")))?;
    output_stream.play()?;
    input_stream.play()?;
    Ok(Audio {
        captured,
        playback,
        rendered,
        stats,
        errors,
        input_name,
        output_name,
        target,
        _input: input_stream,
        _output: output_stream,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cpal::SupportedBufferSize;

    fn range(rate: u32, channels: u16, format: SampleFormat) -> SupportedStreamConfigRange {
        SupportedStreamConfigRange::new(channels, rate, rate, SupportedBufferSize::Unknown, format)
    }

    #[test]
    fn native_48khz_takes_precedence_over_float_and_mono() {
        let config = best_config([
            range(16_000, 1, SampleFormat::F32),
            range(48_000, 2, SampleFormat::I16),
        ])
        .unwrap();
        assert_eq!(config.sample_rate(), 48_000);
        assert_eq!(config.sample_format(), SampleFormat::I16);
    }

    #[test]
    fn headset_pcm_and_continuous_rate_ranges_are_supported() {
        let config = best_config([range(16_000, 1, SampleFormat::I16)]).unwrap();
        assert_eq!(config.sample_rate(), 16_000);
        let config = best_config([SupportedStreamConfigRange::new(
            1,
            8_000,
            96_000,
            SupportedBufferSize::Unknown,
            SampleFormat::F32,
        )])
        .unwrap();
        assert_eq!(config.sample_rate(), 48_000);
        assert!(best_config([range(48_000, 0, SampleFormat::F32)]).is_none());
        assert!(best_config([]).is_none());
    }
}
