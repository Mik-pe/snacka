// SPDX-License-Identifier: MIT
#![cfg(feature = "opus")]

use snacka::opus::{
    Application, Bandwidth, Decoder, Encoder, EncoderConfig, FrameDuration,
    multistream::{ChannelMapping, MultistreamDecoder, MultistreamEncoder},
    pcm,
    projection::{ProjectionDecoder, ProjectionEncoder, ProjectionMapping},
};

fn input(channels: usize, samples: usize) -> (Vec<f32>, Vec<f32>) {
    let integer16: Vec<i16> = (0..channels * samples)
        .map(|index| ((index as f32 * 0.057).sin() * 20_000.0) as i16)
        .collect();
    let integer24: Vec<_> = integer16
        .iter()
        .map(|&value| i32::from(value) * 256)
        .collect();
    let mut from16 = vec![0.0; integer16.len()];
    let mut from24 = vec![0.0; integer24.len()];
    pcm::i16_to_f32(&integer16, &mut from16).unwrap();
    pcm::i24_to_f32(&integer24, &mut from24).unwrap();
    assert_eq!(from16, from24);
    (from16, from24)
}

fn config(rate: u32, channels: usize) -> EncoderConfig {
    EncoderConfig::new(
        32_000 * channels as u32,
        channels,
        FrameDuration::Ms20,
        Bandwidth::Wideband,
        Application::Voice,
    )
    .unwrap()
    .with_sample_rate(rate)
    .unwrap()
}

fn output(floating: &[f32]) {
    let mut integer16 = vec![7; floating.len() + 3];
    let mut integer24 = vec![7; floating.len() + 3];
    pcm::f32_to_i16(floating, &mut integer16).unwrap();
    pcm::f32_to_i24(floating, &mut integer24).unwrap();
    assert_eq!(&integer16[floating.len()..], &[7; 3]);
    assert_eq!(&integer24[floating.len()..], &[7; 3]);
}

#[test]
fn formats_compose_with_ordinary_mapped_and_projection_at_every_rate() {
    for rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
        let cfg = config(rate, 1);
        let samples = cfg.input_samples();
        let (input16, input24) = input(1, samples);
        let mut enc16 = Encoder::new(cfg).unwrap();
        let mut enc24 = Encoder::new(cfg).unwrap();
        let mut packet16 = vec![0; cfg.max_packet_bytes()];
        let mut packet24 = packet16.clone();
        let mut decoder = Decoder::with_sample_rate(1, rate).unwrap();
        let mut floating = vec![0.0; samples];
        let mut packet_length = 0;
        for _ in 0..3 {
            let length16 = enc16.encode(&input16, &mut packet16).unwrap();
            packet_length = length16;
            let length24 = enc24.encode(&input24, &mut packet24).unwrap();
            assert_eq!(&packet16[..length16], &packet24[..length24]);
            assert_eq!(
                decoder
                    .decode(&packet16[..length16], &mut floating)
                    .unwrap(),
                samples
            );
            output(&floating);
        }
        decoder.conceal(samples, &mut floating).unwrap();
        output(&floating);
        decoder
            .recover(&packet16[..packet_length], samples, &mut floating)
            .unwrap();
        output(&floating);

        let mapping = ChannelMapping::new(2, 1, &[0, 1, 2, 255, 0]).unwrap();
        let configs = [config(rate, 2), cfg];
        let (input16, input24) = input(5, samples);
        let mut enc16 = MultistreamEncoder::new(mapping, &configs).unwrap();
        let mut enc24 = MultistreamEncoder::new(mapping, &configs).unwrap();
        let mut packet16 = vec![0; enc16.max_packet_bytes()];
        let mut packet24 = packet16.clone();
        let mut decoder = MultistreamDecoder::with_sample_rate(mapping, rate).unwrap();
        let mut floating = vec![0.0; samples * 5];
        let length16 = enc16.encode(&input16, &mut packet16).unwrap();
        let length24 = enc24.encode(&input24, &mut packet24).unwrap();
        assert_eq!(&packet16[..length16], &packet24[..length24]);
        decoder
            .decode(&packet16[..length16], &mut floating)
            .unwrap();
        output(&floating);
        decoder.conceal(samples, &mut floating).unwrap();
        output(&floating);
        decoder
            .recover(&packet16[..length16], samples, &mut floating)
            .unwrap();
        output(&floating);

        let mapping = ProjectionMapping::new(1, 0, 4, vec![32767, -16384, 8192, 0]).unwrap();
        let (input16, input24) = input(4, samples);
        let mixing = vec![0.5, 0.125, 0.125, 0.125];
        let mut enc16 = ProjectionEncoder::new(mapping.clone(), mixing.clone(), &[cfg]).unwrap();
        let mut enc24 = ProjectionEncoder::new(mapping.clone(), mixing, &[cfg]).unwrap();
        let mut packet16 = vec![0; enc16.max_packet_bytes()];
        let mut packet24 = packet16.clone();
        let length16 = enc16.encode(&input16, &mut packet16).unwrap();
        let length24 = enc24.encode(&input24, &mut packet24).unwrap();
        assert_eq!(&packet16[..length16], &packet24[..length24]);
        let mut decoder = ProjectionDecoder::with_sample_rate(mapping, rate).unwrap();
        let mut floating = vec![0.0; samples * 4];
        decoder
            .decode(&packet16[..length16], &mut floating)
            .unwrap();
        output(&floating);
        decoder.conceal(samples, &mut floating).unwrap();
        output(&floating);
        decoder
            .recover(&packet16[..length16], samples, &mut floating)
            .unwrap();
        output(&floating);
    }
}

#[test]
fn extended_i24_conversion_does_not_silently_clip_encoder_input() {
    let mut encoder = Encoder::new(EncoderConfig::default()).unwrap();
    let input = vec![8_388_609; encoder.config().input_samples()];
    let mut floating = vec![0.0; input.len()];
    pcm::i24_to_f32(&input, &mut floating).unwrap();
    assert!(floating.iter().all(|&sample| sample > 1.0));
    let mut packet = [77; 1276];
    assert_eq!(
        encoder.encode(&floating, &mut packet),
        Err(snacka::opus::EncodeError::Input)
    );
    assert!(packet.iter().all(|&byte| byte == 77));
    assert_eq!(encoder.final_range(), 0);
}
