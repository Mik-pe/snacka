// SPDX-License-Identifier: MIT
#![cfg(feature = "opus")]

use snacka::opus::{Bandwidth, CeltConfig, CeltEncoder, CeltFrameDuration, Decoder};

#[test]
fn duplex_codec_construction_and_processing_fit_a_one_mebibyte_stack() {
    std::thread::Builder::new()
        .name("opus-small-stack".into())
        .stack_size(1024 * 1024)
        .spawn(|| {
            for channels in 1..=2 {
                let config = CeltConfig::new(
                    32_000,
                    CeltFrameDuration::Ms20,
                    channels,
                    Bandwidth::Fullband,
                )
                .unwrap();
                let mut encoder = CeltEncoder::new(config);
                let mut decoder = Decoder::new(channels).unwrap();
                let input = [0.0; 1920];
                let mut packet = [0; 1276];
                let mut output = [0.0; 1920];
                for _ in 0..4 {
                    let length = encoder
                        .encode(&input[..960 * channels], &mut packet)
                        .unwrap();
                    assert_eq!(decoder.decode(&packet[..length], &mut output).unwrap(), 960);
                    assert!(
                        output[..960 * channels]
                            .iter()
                            .all(|value| value.is_finite())
                    );
                }
                encoder.reset();
                decoder.reset();
            }
        })
        .unwrap()
        .join()
        .unwrap();
}
