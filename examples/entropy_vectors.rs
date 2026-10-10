// SPDX-License-Identifier: MIT
//! Emit native entropy packets using the event sequences in the reference fixture.
use snacka::entropy::{Encoder, Model};
use std::io::{self, Write};

fn main() -> io::Result<()> {
    let fixture = include_bytes!("../tests/fixtures/entropy-libopus-1.6.1.bin");
    let cases = u32::from_le_bytes(fixture[8..12].try_into().unwrap());
    let mut cursor = 12;
    let mut output = io::BufWriter::new(io::stdout().lock());
    output.write_all(&fixture[..12])?;
    let model = Model::new(&[0, 27, 200, 256]).unwrap();
    for _ in 0..cases {
        let old_length =
            u16::from_le_bytes(fixture[cursor..cursor + 2].try_into().unwrap()) as usize;
        let events = u16::from_le_bytes(fixture[cursor + 2..cursor + 4].try_into().unwrap());
        cursor += 4 + old_length;
        let events_start = cursor;
        let mut packet = [0; 4_096];
        let mut encoder = Encoder::new(&mut packet);
        for _ in 0..events {
            let kind = fixture[cursor];
            let parameter = u32::from_le_bytes(fixture[cursor + 1..cursor + 5].try_into().unwrap());
            let value = u32::from_le_bytes(fixture[cursor + 5..cursor + 9].try_into().unwrap());
            match kind {
                0 => encoder.symbol(value as usize, model).unwrap(),
                1 => encoder.bit(value != 0, parameter).unwrap(),
                2 => encoder.raw(value, parameter).unwrap(),
                3 => encoder.uniform(value, parameter).unwrap(),
                _ => panic!("invalid fixture event"),
            }
            cursor += 21;
        }
        let length = encoder.finish().unwrap();
        output.write_all(&(length as u16).to_le_bytes())?;
        output.write_all(&events.to_le_bytes())?;
        output.write_all(&packet[..length])?;
        output.write_all(&fixture[events_start..cursor])?;
    }
    output.flush()
}
