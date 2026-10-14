// SPDX-License-Identifier: MIT
#![cfg(feature = "opus")]

use snacka::opus::multistream::{ChannelMapping, MultistreamDecoder};
use snacka::opus::{
    Application, Bandwidth, CeltConfig, CeltEncoder, CeltFrameDuration, Decoder, Encoder,
    EncoderConfig, FrameDuration, Mode,
};

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

#[test]
fn public_opus_construction_and_reset_fit_a_one_mib_thread() {
    std::thread::Builder::new()
        .name("opus-one-mib-stack".to_owned())
        .stack_size(1024 * 1024)
        .spawn(|| {
            for channels in [1, 2] {
                for rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
                    let mut decoder = Decoder::with_sample_rate(channels, rate).unwrap();
                    decoder.reset();
                    std::hint::black_box(decoder);
                }
                for (mode, bandwidth) in [
                    (Mode::Silk, Bandwidth::Wideband),
                    (Mode::Hybrid, Bandwidth::Fullband),
                    (Mode::Celt, Bandwidth::Fullband),
                ] {
                    let config = EncoderConfig::new(
                        64_000,
                        channels,
                        FrameDuration::Ms60,
                        bandwidth,
                        Application::Voice,
                    )
                    .unwrap()
                    .with_mode(mode)
                    .unwrap();
                    let mut encoder = Encoder::new(config).unwrap();
                    encoder.reset();
                    std::hint::black_box(encoder);
                }
            }
            // The maximum public stream/output counts also exercise the
            // constructor's temporary values and reset path on this stack.
            let mapping = ChannelMapping::new(255, 0, &[0; 255]).unwrap();
            let mut mapped = MultistreamDecoder::new(mapping).unwrap();
            mapped.reset();
            std::hint::black_box(mapped);
        })
        .unwrap()
        .join()
        .unwrap();
}
