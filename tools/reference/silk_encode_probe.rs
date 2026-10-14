// SPDX-License-Identifier: MIT
//! Owned native-rate SILK packets for optional independent oracle verification.

#[path = "../../src/entropy.rs"]
mod entropy;
#[path = "../../src/opus/silk/mod.rs"]
pub mod silk;

use std::io::{self, Write};

use silk::{Bandwidth, Frame, FrameDuration, PacketDuration, SilkEncoder};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let packets: u32 = std::env::args()
        .nth(1)
        .map(|value| value.parse())
        .transpose()?
        .unwrap_or(50);
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    stdout.write_all(b"SNKSEC01")?;
    stdout.write_all(&(24 * packets).to_le_bytes())?;
    let mut stream = 0u16;
    for (band, bandwidth, rate) in [
        (0, Bandwidth::Narrow, 8000),
        (1, Bandwidth::Medium, 12000),
        (2, Bandwidth::Wide, 16000),
    ] {
        for (index, duration, milliseconds) in [
            (0, PacketDuration::Ms10, 10),
            (1, PacketDuration::Ms20, 20),
            (2, PacketDuration::Ms40, 40),
            (3, PacketDuration::Ms60, 60),
        ] {
            for channels in 1..=2 {
                let frame = Frame {
                    bandwidth,
                    duration: if milliseconds == 10 {
                        FrameDuration::Ms10
                    } else {
                        FrameDuration::Ms20
                    },
                };
                let samples = rate * milliseconds / 1000;
                let mut encoder = SilkEncoder::new();
                let mut pcm = [0.0; 1920];
                for packet in 0..packets as usize {
                    for sample in 0..samples {
                        let time = (packet * samples + sample) as f32 / rate as f32;
                        for channel in 0..channels as usize {
                            let envelope =
                                0.75 + 0.25 * (time * 2.0 * std::f32::consts::PI * 1.7).sin();
                            pcm[sample * channels as usize + channel] = 0.18
                                * envelope
                                * ((time
                                    * 2.0
                                    * std::f32::consts::PI
                                    * (180.0 + channel as f32 * 30.0))
                                    .sin()
                                    + 0.25 * (time * 2.0 * std::f32::consts::PI * 540.0).sin());
                        }
                    }
                    let mut bytes = [0; 4096];
                    bytes[0] = ((band * 4 + index) << 3) | ((channels - 1) << 2);
                    let mut range = entropy::Encoder::new(&mut bytes[1..]);
                    encoder.encode_layer(&mut range, frame, duration, channels,
                        &pcm[..samples * channels as usize], (24 * milliseconds * channels as usize) as u32)
                        .map_err(|error| format!("stream {stream} packet {packet} {rate}Hz/{milliseconds}ms/{channels}ch: {error}"))?;
                    let final_range = range.range();
                    let length = range.finish()? + 1;
                    stdout.write_all(&stream.to_le_bytes())?;
                    stdout.write_all(&[band, milliseconds as u8, channels])?;
                    stdout.write_all(&(length as u16).to_le_bytes())?;
                    stdout.write_all(&(samples as u16).to_le_bytes())?;
                    stdout.write_all(&final_range.to_le_bytes())?;
                    stdout.write_all(&bytes[..length])?;
                    for sample in &pcm[..samples * channels as usize] {
                        stdout.write_all(&sample.to_le_bytes())?;
                    }
                }
                stream += 1;
            }
        }
    }
    stdout.flush()?;
    Ok(())
}
