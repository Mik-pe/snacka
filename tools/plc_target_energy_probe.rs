// SPDX-License-Identifier: MIT
//! Synthetic packet/PCM/feature parity, paired call timing, and gain fixtures.
use snacka::{Config, Decoder, Encoder, FRAME_SAMPLES, FrameDuration, MAX_PACKET_BYTES};
use std::{
    error::Error,
    fs,
    hint::black_box,
    io::{self, Write},
    time::Instant,
};

fn floats(out: &mut impl Write, values: &[f32]) -> io::Result<()> {
    for value in values {
        out.write_all(&value.to_le_bytes())?;
    }
    Ok(())
}

fn lost(frame: usize) -> bool {
    matches!(frame % 40, 8 | 18 | 19 | 28..=33)
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() != 3 {
        return Err(io::Error::other("trace|time|parity output").into());
    }
    let mut out = io::BufWriter::new(fs::File::create(&args[2])?);
    if args[1] == "parity" {
        out.write_all(b"STEG\x01\0\0\0")?;
        for case in 0..32 {
            let n = if case % 2 == 0 { 480 } else { 960 };
            let candidate: Vec<_> = (0..n)
                .map(|i| {
                    if case % 7 == 0 {
                        0.0
                    } else {
                        ((i + case * 13) as f32 * 0.037).sin() * 0.1f32.powi((case % 6) as i32)
                    }
                })
                .collect();
            let last = std::array::from_fn(|i| -((i + case) as f32) * 0.7);
            let delta = std::array::from_fn(|i| match case {
                0 => 1e20,
                1 => -1e20,
                _ => ((i * 13 + case) as f32 * 0.27).sin() * 8.0,
            });
            let gains = snacka::diagnostic_target_energy(&candidate, &last, &delta);
            out.write_all(&(n as u32).to_le_bytes())?;
            floats(&mut out, &candidate)?;
            floats(&mut out, &last)?;
            floats(&mut out, &delta)?;
            floats(&mut out, &gains)?;
        }
        return Ok(());
    }
    if args[1] == "time" {
        writeln!(
            out,
            "signal,bitrate,frame_ms,harmonic,frame,operation,elapsed_ns"
        )?;
    }
    for rate in [12000, 24000, 32000] {
        for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
            let n = duration.samples();
            for harmonic in [false, true] {
                for signal in 0..3 {
                    let config = Config::new(rate)?
                        .with_frame_duration(duration)
                        .with_harmonic_speech(harmonic);
                    let mut encoder = Encoder::new(config);
                    let mut decoder = Decoder::with_frame_duration(duration);
                    let mut reference = Decoder::with_frame_duration(duration);
                    let mut packet = [0; MAX_PACKET_BYTES];
                    let mut input = [0.0; FRAME_SAMPLES];
                    let mut output = [0.0; FRAME_SAMPLES];
                    let mut good = [0.0; FRAME_SAMPLES];
                    let mut seed = 6716u32;
                    let mut timings = Vec::with_capacity(96);
                    for frame in 0..96 {
                        for (i, value) in input[..n].iter_mut().enumerate() {
                            seed ^= seed << 13;
                            seed ^= seed >> 17;
                            seed ^= seed << 5;
                            let noise = seed as f32 / u32::MAX as f32 - 0.5;
                            let t = (frame * n + i) as f32 / 48000.0;
                            *value = match signal {
                                0 => 0.0,
                                1 => {
                                    0.24 * (std::f32::consts::TAU * (173.0 * t + 0.1 * t * t)).sin()
                                        + noise * 0.12
                                }
                                _ => noise * 0.4,
                            };
                        }
                        let length = encoder.encode(&input[..n], &mut packet)?;
                        if args[1] == "trace" {
                            reference.decode(&packet[..length], &mut good[..n])?;
                            out.write_all(&(length as u32).to_le_bytes())?;
                            out.write_all(&packet[..length])?;
                            floats(&mut out, &good[..n])?;
                            let mut features = [0.0; 80];
                            let mut coefficients = [0.0; FRAME_SAMPLES];
                            decoder.plc_training_prediction(1, &mut features, &mut coefficients)?;
                            floats(&mut out, &features)?;
                        }
                        let start = Instant::now();
                        if lost(frame) {
                            decoder.conceal(black_box(&mut output[..n]))?;
                        } else {
                            decoder.decode(
                                black_box(&packet[..length]),
                                black_box(&mut output[..n]),
                            )?;
                        }
                        let elapsed = start.elapsed().as_nanos();
                        if args[1] == "trace" {
                            floats(&mut out, &output[..n])?;
                        } else {
                            timings.push((frame, elapsed));
                        }
                    }
                    decoder.reset();
                    if args[1] == "trace" {
                        let mut features = [0.0; 80];
                        let mut coefficients = [0.0; FRAME_SAMPLES];
                        decoder.plc_training_prediction(1, &mut features, &mut coefficients)?;
                        floats(&mut out, &features)?;
                    }
                    for (frame, elapsed) in timings {
                        let operation = if lost(frame) {
                            if frame > 0 && lost(frame - 1) {
                                "continued_loss"
                            } else {
                                "first_loss"
                            }
                        } else if frame > 0 && lost(frame - 1) {
                            "recovery"
                        } else {
                            "received"
                        };
                        writeln!(
                            out,
                            "{signal},{rate},{},{},{frame},{operation},{elapsed}",
                            n / 48,
                            u8::from(harmonic)
                        )?;
                    }
                }
            }
        }
    }
    Ok(())
}
