// SPDX-License-Identifier: MIT
//! Offline owned Opus quality probe. Foreign codecs are never linked here.
//! Build against a release library with rustc --edition=2024 -O --extern
//! snacka=target/release/libsnacka.rlib -L dependency=target/release/deps.

use snacka::opus::{
    Application, Bandwidth, Decoder, Encoder, EncoderConfig, FrameDuration, Mode, Packet,
    RateControl,
};
use std::error::Error;
use std::fs;
use std::io::{self, Write};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    if args.len() != 9 {
        return Err("usage: opus_quality_probe INPUT.f32 OUTPUT.f32 PACKETS.bin MODE BANDWIDTH FRAME_MS CHANNELS BITRATE LOSS_CSV (use - for no losses)".into());
    }
    let mode = match args[3].as_str() {
        "silk" => Mode::Silk,
        "hybrid" => Mode::Hybrid,
        "celt" => Mode::Celt,
        _ => return Err("mode must be silk, hybrid or celt".into()),
    };
    let bandwidth = match args[4].as_str() {
        "NB" => Bandwidth::Narrowband,
        "MB" => Bandwidth::Mediumband,
        "WB" => Bandwidth::Wideband,
        "SWB" => Bandwidth::Superwideband,
        "FB" => Bandwidth::Fullband,
        _ => return Err("invalid bandwidth".into()),
    };
    let duration = match args[5].as_str() {
        "2.5" => FrameDuration::Ms2_5,
        "5" => FrameDuration::Ms5,
        "10" => FrameDuration::Ms10,
        "20" => FrameDuration::Ms20,
        "40" => FrameDuration::Ms40,
        "60" => FrameDuration::Ms60,
        "80" => FrameDuration::Ms80,
        "100" => FrameDuration::Ms100,
        "120" => FrameDuration::Ms120,
        _ => return Err("invalid frame duration".into()),
    };
    let channels: usize = args[6].parse()?;
    let bitrate: u32 = args[7].parse()?;
    let losses: Vec<usize> = if args[8] == "-" {
        Vec::new()
    } else {
        args[8]
            .split(',')
            .map(str::parse)
            .collect::<Result<_, _>>()?
    };
    let bytes = fs::read(&args[0])?;
    let (chunks, trailing) = bytes.as_chunks::<4>();
    if !trailing.is_empty() {
        return Err("incomplete f32 input".into());
    }
    let input: Vec<f32> = chunks
        .iter()
        .map(|value| f32::from_le_bytes(*value))
        .collect();
    let application = if mode == Mode::Celt {
        Application::LowDelay
    } else {
        Application::Voice
    };
    let config = EncoderConfig::new(bitrate, channels, duration, bandwidth, application)?
        .with_mode(mode)?
        .with_rate_control(RateControl::Constant);
    if !input.len().is_multiple_of(channels)
        || input.is_empty()
        || input
            .iter()
            .any(|v| !v.is_finite() || !(-1.0..=1.0).contains(v))
    {
        return Err("invalid normalized interleaved PCM".into());
    }
    let samples = duration.samples();
    let budget_numerator = u64::from(bitrate) * samples as u64;
    if !budget_numerator.is_multiple_of(384_000) {
        return Err("quality comparison requires integral complete packet budgets".into());
    }
    let budget = (budget_numerator / 384_000) as usize;
    let mut encoder = Encoder::new(config)?;
    let mut decoder = Decoder::new(channels)?;
    let delay = encoder.algorithmic_delay_samples();
    let ordinary = (input.len() / channels).div_ceil(samples);
    // Keep one complete drain frame on both sides of matched offline tests.
    // 10/20 ms frames also cover the reference VOIP encoder's longer lookahead.
    let packets = ordinary + delay.div_ceil(samples);
    if losses.iter().any(|&index| index >= packets) {
        return Err("loss index exceeds packet stream".into());
    }
    let mut frame = vec![0.0; samples * channels];
    let mut output = vec![0.0; samples * channels];
    let mut decoded = Vec::with_capacity(packets * frame.len());
    let mut packet = [0; 8000];
    let mut archive = io::BufWriter::new(fs::File::create(&args[2])?);
    archive.write_all(b"SNKQ0001")?;
    let mut packet_bytes = 0;
    let mut drain_bytes = 0;
    for index in 0..packets {
        frame.fill(0.0);
        let start = index * frame.len();
        if start < input.len() {
            let count = frame.len().min(input.len() - start);
            frame[..count].copy_from_slice(&input[start..start + count]);
        }
        let length = encoder.encode(&frame, &mut packet)?;
        let parsed = Packet::parse(&packet[..length])?;
        if length != budget
            || parsed.toc().mode() != mode
            || parsed.toc().bandwidth() != bandwidth
            || usize::from(parsed.toc().channels()) != channels
            || parsed.samples(48_000)? != samples
        {
            return Err("packet budget or wire geometry mismatch".into());
        }
        archive.write_all(&(length as u32).to_le_bytes())?;
        archive.write_all(&encoder.final_range().to_le_bytes())?;
        archive.write_all(&packet[..length])?;
        if losses.contains(&index) {
            decoder.conceal(samples, &mut output)?;
        } else {
            decoder.decode(&packet[..length], &mut output)?;
        }
        if output.iter().any(|v| !v.is_finite()) {
            return Err("non-finite decoded PCM".into());
        }
        decoded.extend_from_slice(&output);
        if index < ordinary {
            packet_bytes += length;
        } else {
            drain_bytes += length;
        }
    }
    archive.flush()?;
    let aligned = &decoded[delay * channels..delay * channels + input.len()];
    let mut sink = io::BufWriter::new(fs::File::create(&args[1])?);
    for value in aligned {
        sink.write_all(&value.to_le_bytes())?;
    }
    sink.flush()?;
    println!(
        "{{\"samples\":{},\"packets\":{packets},\"packet_bytes\":{packet_bytes},\"drain_bytes\":{drain_bytes},\"algorithmic_delay_samples\":{delay},\"channels\":{channels}}}",
        input.len() / channels
    );
    Ok(())
}
