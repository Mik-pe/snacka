// SPDX-License-Identifier: MIT
#![no_main]
use libfuzzer_sys::fuzz_target;
use snacka::{Config, Decoder, Encoder, FrameDuration, MAX_PACKET_BYTES, Packet};

fn repair_crc(packet: &mut [u8]) {
    let end = packet.len() - 2;
    let mut crc = 0xffffu16;
    for &byte in &packet[..end] {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 { (crc << 1) ^ 0x1021 } else { crc << 1 };
        }
    }
    packet[end..].copy_from_slice(&crc.to_le_bytes());
}

fuzz_target!(|bytes: &[u8]| {
    let _ = Packet::parse(bytes);
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let samples = duration.samples();
        let bitrate = 12_000 + u32::from(bytes.first().copied().unwrap_or(0)) * 20_000 / 255;
        let config = Config::new(bitrate).unwrap().with_frame_duration(duration);
        let mut encoder = Encoder::new(config);
        let mut decoder = Decoder::with_frame_duration(duration);
        let input: Vec<_> = (0..samples).map(|i| {
            (f32::from(bytes.get(i % bytes.len().max(1)).copied().unwrap_or(128)) - 128.0) / 128.0
        }).collect();
        let mut packet = [0; MAX_PACKET_BYTES];
        let mut output = vec![7.0; samples];
        for _ in 0..4 {
            let length = encoder.encode(&input, &mut packet).unwrap();
            decoder.decode(&packet[..length], &mut output).unwrap();
        }
        assert!(output.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        let length = encoder.encode(&input, &mut packet).unwrap();
        packet[2] = (packet[2] & !6) | (2 * (bytes.get(1).copied().unwrap_or(0) % 3));
        for (destination, &source) in packet[4..length - 2].iter_mut().zip(bytes) {
            *destination ^= source;
        }
        repair_crc(&mut packet[..length]);
        output.fill(7.0);
        if decoder.decode(&packet[..length], &mut output).is_ok() {
            assert!(output.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        } else {
            assert!(output.iter().all(|&x| x == 7.0));
        }
        for _ in 0..12 {
            decoder.conceal(&mut output).unwrap();
            assert!(output.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        }
        assert!(output.iter().all(|&x| x == 0.0));
    }
});
