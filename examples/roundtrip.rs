// SPDX-License-Identifier: MIT
//! Offline validation of little-endian mono f32 PCM at 48 kHz.
use snacka::{ALGORITHMIC_DELAY, Config, Decoder, Encoder, FrameDuration, MAX_PACKET_BYTES};
use std::{error::Error, fs, io};

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments.len() != 5 {
        return Err(
            io::Error::other("usage: roundtrip input.f32 output.f32 bitrate frame_ms").into(),
        );
    }
    let duration = match arguments[4].as_str() {
        "10" => FrameDuration::Ms10,
        "20" => FrameDuration::Ms20,
        _ => return Err(io::Error::other("frame_ms must be 10 or 20").into()),
    };
    let config = Config::new(arguments[3].parse()?)?.with_frame_duration(duration);
    let bytes = fs::read(&arguments[1])?;
    let (chunks, trailing) = bytes.as_chunks::<4>();
    if !trailing.is_empty() {
        return Err(io::Error::other("input must contain complete f32 samples").into());
    }
    let input: Vec<_> = chunks.iter().map(|&x| f32::from_le_bytes(x)).collect();
    let samples = config.frame_samples();
    let mut encoder = Encoder::new(config);
    let mut decoder = Decoder::with_frame_duration(duration);
    let mut packet = [0; MAX_PACKET_BYTES];
    let mut frame = vec![0.0; samples];
    let mut decoded = vec![0.0; samples];
    let mut output = Vec::new();
    let mut encoded_bytes = 0;
    for chunk in input.chunks(samples) {
        frame.fill(0.0);
        frame[..chunk.len()].copy_from_slice(chunk);
        let length = encoder.encode(&frame, &mut packet)?;
        encoded_bytes += length;
        let info = decoder.decode(&packet[..length], &mut decoded)?;
        output.extend_from_slice(&decoded[..info.valid_samples]);
    }
    let length = encoder.finish(&mut packet)?;
    let info = decoder.decode(&packet[..length], &mut decoded)?;
    output.extend_from_slice(&decoded[..info.valid_samples]);
    let output = &output[ALGORITHMIC_DELAY..ALGORITHMIC_DELAY + input.len()];
    let pcm: Vec<_> = output.iter().flat_map(|x| x.to_le_bytes()).collect();
    fs::write(&arguments[2], pcm)?;
    let signal: f64 = input.iter().map(|&x| f64::from(x).powi(2)).sum();
    let error: f64 = input
        .iter()
        .zip(output)
        .map(|(&x, &y)| f64::from(x - y).powi(2))
        .sum();
    println!(
        "samples={},frame_ms={},packet_bytes={},drain_bytes={},aligned_snr_db={:.3}",
        input.len(),
        duration.milliseconds(),
        encoded_bytes,
        length,
        10.0 * (signal / error.max(1e-30)).log10()
    );
    Ok(())
}
