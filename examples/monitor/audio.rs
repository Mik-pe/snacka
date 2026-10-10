// SPDX-License-Identifier: MIT
use cpal::{
    Device, FromSample, Sample, SampleFormat, SizedSample, Stream, SupportedStreamConfig,
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
}

pub struct Audio {
    pub captured: Consumer<f32>,
    pub playback: Producer<f32>,
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
        println!("  {index}: {}", name(&device));
    }
    println!("Output devices (--output INDEX):");
    for (index, device) in host.output_devices()?.enumerate() {
        println!("  {index}: {}", name(&device));
    }
    Ok(())
}

fn select_config(device: &Device, input: bool) -> Result<SupportedStreamConfig, Box<dyn Error>> {
    let ranges: Vec<_> = if input {
        device.supported_input_configs()?.collect()
    } else {
        device.supported_output_configs()?.collect()
    };
    // Prefer float and few channels; input and output need not share a format.
    ranges.into_iter()
        .filter(|r| r.channels() > 0 && supported_format(r.sample_format()))
        .filter_map(|r| r.try_with_sample_rate(SAMPLE_RATE))
        .min_by_key(|c| (c.sample_format() != SampleFormat::F32, c.channels()))
        .ok_or_else(|| io::Error::other(format!(
            "{} has no supported 48 kHz PCM {} configuration. Select another device with --list-devices; this example does not resample.",
            name(device), if input { "input" } else { "output" }
        )).into())
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
                dropped += usize::from(producer.push(sample).is_err());
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

fn play<T: SizedSample + FromSample<f32>>(
    device: &Device,
    config: SupportedStreamConfig,
    mut consumer: Consumer<f32>,
    stats: Arc<Stats>,
    errors: mpsc::Sender<cpal::Error>,
    target: Arc<AtomicUsize>,
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
                    return;
                }
                primed = true;
            }
            let mut gaps = 0;
            for frame in data.chunks_exact_mut(channels) {
                let sample = consumer.pop().unwrap_or_else(|_| {
                    primed = false;
                    gaps += 1;
                    0.0
                });
                frame.fill(T::from_sample(sample));
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
    let (capture_producer, captured) = RingBuffer::new(FRAME_SAMPLES * 4);
    // Allocate once for the largest live buffer choice (200 ms, with headroom).
    let (playback, playback_consumer) = RingBuffer::new(SAMPLE_RATE as usize * 400 / 1000);
    let target = Arc::new(AtomicUsize::new(target));
    let stats = Arc::new(Stats::default());
    let (error_tx, errors) = mpsc::channel();
    let input_stream = stream!(
        capture,
        &input,
        input_config,
        capture_producer,
        stats.clone(),
        error_tx.clone()
    )?;
    let output_stream = stream!(
        play,
        &output,
        output_config,
        playback_consumer,
        stats.clone(),
        error_tx,
        target.clone()
    )?;
    output_stream.play()?;
    input_stream.play()?;
    Ok(Audio {
        captured,
        playback,
        stats,
        errors,
        input_name: name(&input),
        output_name: name(&output),
        target,
        _input: input_stream,
        _output: output_stream,
    })
}
