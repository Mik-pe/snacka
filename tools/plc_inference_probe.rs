// SPDX-License-Identifier: MIT
//! Batched wall timing of the actual quantized16/17-head native inference.
use std::{env, fs, hint::black_box, time::Instant};
fn main() {
    let raw = fs::read(env::args().nth(1).expect("parity fixture")).unwrap();
    assert!(raw.len() == 32 * 384 || raw.len() == 32 * 388);
    let stride = raw.len() / 32;
    let input: [[f32; 80]; 32] = std::array::from_fn(|row| {
        std::array::from_fn(|column| {
            let at = row * stride + column * 4;
            f32::from_le_bytes(raw[at..at + 4].try_into().unwrap())
        })
    });
    for i in 0..16384 {
        black_box(snacka::diagnostic_controls(black_box(&input[i & 31])));
    }
    println!("repetition,calls,nanoseconds_per_call");
    for repetition in 0..20 {
        let calls = 65536;
        let start = Instant::now();
        for i in 0..calls {
            black_box(snacka::diagnostic_controls(black_box(&input[i & 31])));
        }
        println!("{repetition},{calls},{:.6}", start.elapsed().as_nanos() as f64 / calls as f64);
    }
}
