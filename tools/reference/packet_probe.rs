// SPDX-License-Identifier: MIT
//! Standalone packet oracle probe, compiled by check_opus_packets.py.

#![forbid(unsafe_code)]

#[allow(dead_code)]
#[path = "../../src/opus/packet.rs"]
mod packet;

use std::hint::black_box;
use std::io::{self, Write};
use std::time::Instant;

fn packets(path: &str) -> io::Result<Vec<Vec<u8>>> {
    let bytes = std::fs::read(path)?;
    let mut remaining = bytes.as_slice();
    let mut packets = Vec::new();
    while !remaining.is_empty() {
        if remaining.len() < 4 {
            return Err(io::Error::other("truncated packet record length"));
        }
        let length = u32::from_le_bytes(remaining[..4].try_into().unwrap()) as usize;
        remaining = &remaining[4..];
        if length > remaining.len() {
            return Err(io::Error::other("truncated packet record data"));
        }
        packets.push(remaining[..length].to_vec());
        remaining = &remaining[length..];
    }
    Ok(packets)
}

fn bandwidth(value: packet::Bandwidth) -> u8 {
    match value {
        packet::Bandwidth::Narrowband => 0,
        packet::Bandwidth::Mediumband => 1,
        packet::Bandwidth::Wideband => 2,
        packet::Bandwidth::Superwideband => 3,
        packet::Bandwidth::Fullband => 4,
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let args = std::env::args().skip(1).collect::<Vec<_>>();
    if !(1..=2).contains(&args.len()) {
        return Err("usage: packet_probe INPUT [BENCHMARK_REPETITIONS]".into());
    }
    let packets = packets(&args[0])?;
    if let Some(repetitions) = args.get(1) {
        let repetitions: usize = repetitions.parse()?;
        let mut checksum = 0u64;
        let start = Instant::now();
        for _ in 0..repetitions {
            for bytes in &packets {
                match packet::Packet::parse(black_box(bytes)) {
                    Ok(packet) => {
                        checksum += packet.frame_count() as u64;
                        checksum += packet.frames().map(|frame| frame.len() as u64).sum::<u64>();
                    }
                    Err(_) => checksum += 1,
                }
            }
        }
        let elapsed = start.elapsed().as_secs_f64();
        println!("{elapsed:.9} {checksum}");
        return Ok(());
    }

    let stdout = io::stdout();
    let mut output = io::BufWriter::new(stdout.lock());
    for bytes in packets {
        let Ok(packet) = packet::Packet::parse(&bytes) else {
            output.write_all(&[0])?;
            continue;
        };
        let toc = packet.toc();
        output.write_all(&[
            1,
            toc.byte(),
            packet.frame_count() as u8,
            toc.channels(),
            bandwidth(toc.bandwidth()),
            u8::from(packet.is_vbr()),
        ])?;
        for rate in [8000, 12000, 16000, 24000, 48000] {
            output.write_all(&(packet.frame_samples(rate)? as u16).to_le_bytes())?;
            output.write_all(&(packet.samples(rate)? as u16).to_le_bytes())?;
        }
        output.write_all(&(packet.padding_len() as u32).to_le_bytes())?;
        for frame in packet.frames() {
            let offset = frame.as_ptr() as usize - bytes.as_ptr() as usize;
            output.write_all(&(frame.len() as u16).to_le_bytes())?;
            output.write_all(&(offset as u32).to_le_bytes())?;
        }
    }
    output.flush()?;
    Ok(())
}
