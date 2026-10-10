// SPDX-License-Identifier: MIT
#![no_main]
use libfuzzer_sys::fuzz_target;
use snacka::entropy::{Decoder, Encoder, Model};

fuzz_target!(|bytes: &[u8]| {
    let model = Model::new(&[0, 1, 2, 256]).unwrap();
    let mut arbitrary = Decoder::new(bytes);
    for &control in bytes.iter().take(128) {
        match control % 4 {
            0 => {
                assert!(arbitrary.symbol(model) < 3);
            }
            1 => {
                let _ = arbitrary.bit(u32::from(control >> 2));
            }
            2 => {
                let _ = arbitrary.raw(u32::from(control >> 2));
            }
            _ => {
                let _ = arbitrary.uniform(u32::from(control) * 65_537);
            }
        }
        assert!(arbitrary.range() > 1 << 23);
        assert!(arbitrary.tell_eighths() >= arbitrary.tell().saturating_sub(1) * 8);
    }

    let mut packet = [0; 1_024];
    let mut encoder = Encoder::new(&mut packet);
    for event in bytes.chunks(5).take(128) {
        let kind = event[0] % 4;
        let mut value_bytes = [0; 4];
        value_bytes[..event.len() - 1].copy_from_slice(&event[1..]);
        let value = u32::from_le_bytes(value_bytes);
        match kind {
            0 => encoder.symbol(value as usize % 3, model).unwrap(),
            1 => encoder.bit(value & 1 != 0, 1 + (value >> 1) % 16).unwrap(),
            2 => encoder.raw(value, 32).unwrap(),
            _ => encoder.uniform(value % u32::MAX, u32::MAX).unwrap(),
        }
    }
    let length = if bytes.first().copied().unwrap_or(0) & 1 == 0 { encoder.finish().unwrap() } else { encoder.finish_padded().unwrap() };
    let mut decoder = Decoder::new(&packet[..length]);
    for event in bytes.chunks(5).take(128) {
        let mut value_bytes = [0; 4];
        value_bytes[..event.len() - 1].copy_from_slice(&event[1..]);
        let value = u32::from_le_bytes(value_bytes);
        match event[0] % 4 {
            0 => assert_eq!(decoder.symbol(model), value as usize % 3),
            1 => assert_eq!(decoder.bit(1 + (value >> 1) % 16), Ok(value & 1 != 0)),
            2 => assert_eq!(decoder.raw(32), Ok(value)),
            _ => assert_eq!(decoder.uniform(u32::MAX), Ok(value % u32::MAX)),
        }
    }

    let capacity = bytes.first().copied().unwrap_or(0) as usize % 65;
    let mut short_packet = [0; 64];
    let mut short_encoder = Encoder::new(&mut short_packet[..capacity]);
    for &value in bytes.iter().take(128) {
        if short_encoder.uniform(u32::from(value), 256).is_err() {
            break;
        }
    }
    let _ = short_encoder.finish();
});
