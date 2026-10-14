// SPDX-License-Identifier: MIT
//! Causal owned SILK LBRR for an optional independent public-API decoder.

#[path = "../../src/entropy.rs"]
mod entropy;
#[path = "../../src/opus/silk/mod.rs"]
pub mod silk;

use silk::{Bandwidth, Frame, FrameDuration, PacketDuration, SilkDecoder, SilkEncoder};
use std::io::{self, Write};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut stdout = io::BufWriter::new(io::stdout().lock());
    stdout.write_all(b"SNKFEC01")?;
    stdout.write_all(&96u32.to_le_bytes())?;
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
                encoder.set_fec(true);
                let mut decoder = SilkDecoder::new();
                let mut pcm = [0.0; 1920];
                for packet in 0..4 {
                    for sample in 0..samples {
                        for channel in 0..channels as usize {
                            let frequency = 160.0 + 70.0 * packet as f32 + 20.0 * channel as f32;
                            pcm[sample * channels as usize + channel] = 0.16
                                * (sample as f32 * 2.0 * std::f32::consts::PI * frequency
                                    / rate as f32)
                                    .sin();
                        }
                    }
                    let mut bytes = [0; 4096];
                    bytes[0] = ((band * 4 + index) << 3) | ((channels - 1) << 2);
                    let mut range = entropy::Encoder::new(&mut bytes[1..]);
                    let statistics = encoder.encode_layer(
                        &mut range,
                        frame,
                        duration,
                        channels,
                        &pcm[..samples * channels as usize],
                        (48 * milliseconds * channels as usize) as u32,
                    )?;
                    if statistics.redundancy != (packet != 0) {
                        return Err("missing owned LBRR".into());
                    }
                    let final_range = range.range();
                    let length = range.finish()? + 1;
                    let mut fec_pcm = [0.0; 1920];
                    let mut normal_pcm = [0.0; 1920];
                    if packet % 2 == 1 {
                        let mut fec = entropy::Decoder::new(&bytes[1..length]);
                        if decoder.decode_fec(&mut fec, frame, duration, channels, &mut fec_pcm)?
                            != Some(samples)
                        {
                            return Err("owned decoder did not recover actual LBRR".into());
                        }
                        let mut regular = entropy::Decoder::new(&bytes[1..length]);
                        if decoder.decode(
                            &mut regular,
                            frame,
                            duration,
                            channels,
                            &mut normal_pcm,
                        )? != samples
                            || regular.range() != final_range
                        {
                            return Err("following owned packet range mismatch".into());
                        }
                    }
                    stdout.write_all(&stream.to_le_bytes())?;
                    stdout.write_all(&[band, milliseconds as u8, channels, packet])?;
                    stdout.write_all(&(length as u16).to_le_bytes())?;
                    stdout.write_all(&(samples as u16).to_le_bytes())?;
                    stdout.write_all(&final_range.to_le_bytes())?;
                    stdout.write_all(&bytes[..length])?;
                    for block in [&pcm, &fec_pcm, &normal_pcm] {
                        for sample in &block[..samples * channels as usize] {
                            stdout.write_all(&sample.to_le_bytes())?;
                        }
                    }
                }
                stream += 1;
            }
        }
    }
    stdout.flush()?;
    Ok(())
}
