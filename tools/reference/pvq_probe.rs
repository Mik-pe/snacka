// SPDX-License-Identifier: MIT
//! Standalone native verifier for the full black-box CWRS corpus.
//!
//! rustc --edition=2024 -O -C overflow-checks=yes tools/reference/pvq_probe.rs -o /tmp/pvq-probe
//! /tmp/pvq-probe /tmp/pvq-full.bin
#![allow(dead_code)]

#[path = "../../src/entropy.rs"]
mod entropy;
#[path = "../../src/opus/pvq.rs"]
mod pvq;

fn main() {
    let path = std::env::args().nth(1).expect("PVQ fixture path");
    let data = std::fs::read(path).expect("read PVQ fixture");
    assert_eq!(&data[..8], b"SNKPVQ01");
    let count = u32::from_le_bytes(data[8..12].try_into().unwrap());
    let mut cursor = 12;
    let mut expected = [0i16; pvq::MAX_DIMENSIONS];
    let mut output = [0i16; pvq::MAX_DIMENSIONS];
    for case in 0..count {
        let n = usize::from(u16::from_le_bytes(
            data[cursor..cursor + 2].try_into().unwrap(),
        ));
        let k = usize::from(u16::from_le_bytes(
            data[cursor + 2..cursor + 4].try_into().unwrap(),
        ));
        cursor += 4;
        let numbers: [u32; 5] = std::array::from_fn(|j| {
            u32::from_le_bytes(data[cursor + j * 4..cursor + j * 4 + 4].try_into().unwrap())
        });
        cursor += 20;
        let packet = &data[cursor..cursor + 8];
        cursor += 8;
        for value in &mut expected[..n] {
            *value = i16::from_le_bytes(data[cursor..cursor + 2].try_into().unwrap());
            cursor += 2;
        }
        let book = pvq::Codebook::new(n, k).unwrap();
        assert_eq!(book.total(), numbers[0], "case {case}: N={n}, K={k}");
        assert_eq!(book.index_of(&expected[..n]).unwrap(), numbers[1]);
        book.vector_at(numbers[1], &mut output[..n]).unwrap();
        assert_eq!(output[..n], expected[..n]);
        let mut decoder = entropy::Decoder::new(packet);
        let index = decoder.uniform(book.total()).unwrap();
        book.vector_at(index, &mut output[..n]).unwrap();
        assert_eq!(output[..n], expected[..n]);
        assert_eq!(decoder.range(), numbers[2]);
        assert_eq!(decoder.tell(), u64::from(numbers[3]));
        assert_eq!(decoder.tell_eighths(), u64::from(numbers[4]));
        let mut bytes = [0; 8];
        let mut encoder = entropy::Encoder::new(&mut bytes);
        encoder
            .uniform(book.index_of(&expected[..n]).unwrap(), book.total())
            .unwrap();
        assert_eq!(encoder.range(), numbers[2]);
        assert_eq!(encoder.tell(), u64::from(numbers[3]));
        assert_eq!(encoder.tell_eighths(), u64::from(numbers[4]));
        encoder.finish_padded().unwrap();
        assert_eq!(bytes.as_slice(), packet);
    }
    assert_eq!(cursor, data.len());
    println!(
        "{count} exact native/reference count, rank, unrank, encode/decode, interval, bit-usage and byte agreements"
    );
}
