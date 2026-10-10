// SPDX-License-Identifier: MIT
//! Offline fixed-frame CPU workloads and exact packet/PCM traces.
use snacka::{Config, Decoder, Encoder, FrameDuration, MAX_PACKET_BYTES};
use std::{error::Error, fs, hint::black_box, io};

fn main() -> Result<(), Box<dyn Error>> {
    let arguments: Vec<_> = std::env::args().collect();
    if arguments.len() != 6 {
        return Err(io::Error::other(
            "usage: cpu_probe encode|decode|trace input.f32 frames|output.trace bitrate frame_ms",
        )
        .into());
    }
    let duration = match arguments[5].as_str() {
        "10" => FrameDuration::Ms10,
        "20" => FrameDuration::Ms20,
        _ => return Err(io::Error::other("frame_ms must be 10 or 20").into()),
    };
    let config = Config::new(arguments[4].parse()?)?.with_frame_duration(duration);
    let bytes = fs::read(&arguments[2])?;
    let (chunks, trailing) = bytes.as_chunks::<4>();
    if !trailing.is_empty() || chunks.is_empty() {
        return Err(io::Error::other("input must contain nonempty complete f32 samples").into());
    }
    let input: Vec<_> = chunks.iter().map(|&x| f32::from_le_bytes(x)).collect();
    let samples = config.frame_samples();
    let mut encoder = Encoder::new(config);
    let mut decoder = Decoder::with_frame_duration(duration);
    let mut packet = [0; MAX_PACKET_BYTES];
    let mut output = vec![0.0; samples];
    let mut frame = vec![0.0; samples];
    if arguments[1] == "trace" {
        let mut trace = Vec::new();
        for chunk in input.chunks(samples) {
            frame.fill(0.0);
            frame[..chunk.len()].copy_from_slice(chunk);
            let length = encoder.encode(&frame, &mut packet)?;
            trace.extend_from_slice(&(length as u16).to_le_bytes());
            trace.extend_from_slice(&packet[..length]);
            let info = decoder.decode(&packet[..length], &mut output)?;
            trace.extend(
                output[..info.valid_samples]
                    .iter()
                    .flat_map(|x| x.to_le_bytes()),
            );
        }
        let length = encoder.finish(&mut packet)?;
        trace.extend_from_slice(&(length as u16).to_le_bytes());
        trace.extend_from_slice(&packet[..length]);
        let info = decoder.decode(&packet[..length], &mut output)?;
        trace.extend(
            output[..info.valid_samples]
                .iter()
                .flat_map(|x| x.to_le_bytes()),
        );
        fs::write(&arguments[3], trace)?;
    } else {
        let count: usize = arguments[3].parse()?;
        if count == 0 {
            return Err(io::Error::other("frames must be positive").into());
        }
        let inputs: Vec<Vec<f32>> = (0..256)
            .map(|index| {
                let mut frame = vec![0.0; samples];
                let start = (index * samples) % input.len();
                let chunk = &input[start..(start + samples).min(input.len())];
                frame[..chunk.len()].copy_from_slice(chunk);
                frame
            })
            .collect();
        match arguments[1].as_str() {
            "encode" => {
                for index in 0..count {
                    let length = encoder.encode(black_box(&inputs[index % 256]), &mut packet)?;
                    black_box(&packet[..length]);
                }
            }
            "decode" => {
                let packets: Vec<_> = inputs
                    .iter()
                    .map(|input| {
                        let length = encoder.encode(input, &mut packet)?;
                        Ok(packet[..length].to_vec())
                    })
                    .collect::<Result<_, snacka::Error>>()?;
                for index in 0..count {
                    decoder.decode(black_box(&packets[index % 256]), &mut output)?;
                    black_box(&output);
                }
            }
            _ => return Err(io::Error::other("mode must be encode, decode or trace").into()),
        }
        println!("frames={count}");
    }
    Ok(())
}
