// SPDX-License-Identifier: MIT
use snacka::{
    ALGORITHMIC_DELAY, Config, Decoder, Encoder, Error, FRAME_SAMPLES, FrameDuration,
    MAX_PACKET_BYTES, Packet,
};

fn tone(frame: usize) -> [f32; FRAME_SAMPLES] {
    std::array::from_fn(|index| {
        let time = (frame * FRAME_SAMPLES + index) as f32 / 48_000.0;
        0.5 * (std::f32::consts::TAU * 440.0 * time).sin()
            + 0.1 * (std::f32::consts::TAU * 880.0 * time).sin()
    })
}

#[test]
fn native_codec_round_trips_both_frame_durations_at_every_target_bitrate() {
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let samples = duration.samples();
        for bitrate in [12_000, 12_345, 16_000, 20_000, 24_000, 32_000] {
            let config = Config::new(bitrate).unwrap().with_frame_duration(duration);
            let mut encoder = Encoder::new(config);
            let mut decoder = Decoder::with_frame_duration(duration);
            let mut packet = [0; MAX_PACKET_BYTES];
            let mut output = vec![0.0; samples];
            let mut original = Vec::new();
            let mut recovered = Vec::new();
            let mut bytes = 0;
            for frame in 0..300 {
                let input: Vec<_> = (0..samples)
                    .map(|i| {
                        let time = (frame * samples + i) as f32 / 48_000.0;
                        0.5 * (std::f32::consts::TAU * 440.0 * time).sin()
                            + 0.1 * (std::f32::consts::TAU * 880.0 * time).sin()
                    })
                    .collect();
                original.extend_from_slice(&input);
                let length = encoder.encode(&input, &mut packet).unwrap();
                assert!(length <= config.maximum_packet_bytes());
                bytes += length;
                let parsed = Packet::parse(&packet[..length]).unwrap();
                assert_eq!(parsed.sequence(), frame as u8);
                assert_eq!(parsed.frame_duration(), duration);
                assert!(!parsed.is_final());
                let info = decoder.decode(&packet[..length], &mut output).unwrap();
                assert_eq!(info.sequence, frame as u8);
                assert_eq!(info.valid_samples, samples);
                assert!(!info.concealed);
                assert!(
                    output
                        .iter()
                        .all(|value| value.is_finite() && value.abs() <= 1.0)
                );
                recovered.extend_from_slice(&output);
            }
            assert_eq!(
                bytes,
                (bitrate * 300 / (8_000 / duration.milliseconds())) as usize
            );
            let length = encoder.finish(&mut packet).unwrap();
            let info = decoder.decode(&packet[..length], &mut output).unwrap();
            assert!(info.finished);
            assert_eq!(info.valid_samples, ALGORITHMIC_DELAY);
            recovered.extend_from_slice(&output[..info.valid_samples]);
            let recovered = &recovered[ALGORITHMIC_DELAY..];
            assert_eq!(recovered.len(), original.len());
            let energy: f64 = original.iter().map(|&x| f64::from(x).powi(2)).sum();
            let error: f64 = original
                .iter()
                .zip(recovered)
                .map(|(&x, &y)| f64::from(x - y).powi(2))
                .sum();
            let snr = 10.0 * (energy / error).log10();
            eprintln!("{duration:?} bitrate {bitrate}: synthetic tone SNR {snr:.2} dB");
            assert!(snr > 8.0, "synthetic tone at {bitrate}: {snr}");
            assert_eq!(
                encoder.encode(&original[..samples], &mut packet),
                Err(Error::Finished)
            );
            assert_eq!(
                decoder.decode(&packet[..length], &mut output),
                Err(Error::Finished)
            );
            assert_eq!(decoder.conceal(&mut output), Err(Error::Finished));
        }
    }
}

#[test]
fn invalid_input_and_insufficient_capacity_preserve_encoder_history() {
    let mut encoder = Encoder::new(Config::default());
    let mut control = Encoder::new(Config::default());
    let mut packet = [0xa5; MAX_PACKET_BYTES];
    assert_eq!(encoder.encode(&[0.0; 479], &mut packet), Err(Error::Input));
    for value in [f32::NAN, f32::INFINITY, 1.01, -1.01] {
        let mut input = tone(0);
        input[301] = value;
        assert_eq!(encoder.encode(&input, &mut packet), Err(Error::Input));
        assert_eq!(packet, [0xa5; MAX_PACKET_BYTES]);
    }
    assert_eq!(
        encoder.encode(&tone(0), &mut packet[..59]),
        Err(Error::Output)
    );
    assert_eq!(packet, [0xa5; MAX_PACKET_BYTES]);
    let length = encoder.encode(&tone(0), &mut packet).unwrap();
    let mut expected = [0; MAX_PACKET_BYTES];
    assert_eq!(control.encode(&tone(0), &mut expected), Ok(length));
    assert_eq!(&packet[..length], &expected[..length]);
    assert_eq!(&packet[length..], &[0xa5; MAX_PACKET_BYTES][length..]);
}

#[test]
fn every_single_bit_flip_is_rejected_without_changing_decoder_history() {
    let mut encoder = Encoder::new(Config::default());
    let mut decoder = Decoder::new();
    let mut control = Decoder::new();
    let mut packet = [0; MAX_PACKET_BYTES];
    let length = encoder.encode(&tone(0), &mut packet).unwrap();
    for bit in 0..length * 8 {
        let mut corrupted = packet;
        corrupted[bit / 8] ^= 1 << (bit % 8);
        let mut output = [9.0; FRAME_SAMPLES];
        assert!(decoder.decode(&corrupted[..length], &mut output).is_err());
        assert_eq!(output, [9.0; FRAME_SAMPLES]);
    }
    for length in 0..15 {
        assert!(
            decoder
                .decode(&packet[..length], &mut [9.0; FRAME_SAMPLES])
                .is_err()
        );
    }
    let mut actual = [0.0; FRAME_SAMPLES];
    let mut expected = [0.0; FRAME_SAMPLES];
    decoder.decode(&packet[..length], &mut actual).unwrap();
    control.decode(&packet[..length], &mut expected).unwrap();
    assert_eq!(actual, expected);
}

#[test]
fn missing_frames_require_explicit_concealment_and_recover_on_the_next_packet() {
    let mut encoder = Encoder::new(Config::default());
    let mut decoder = Decoder::new();
    let mut packet = [0; MAX_PACKET_BYTES];
    let mut output = [0.0; FRAME_SAMPLES];
    for frame in 0..4 {
        let length = encoder.encode(&tone(frame), &mut packet).unwrap();
        decoder.decode(&packet[..length], &mut output).unwrap();
    }
    encoder.encode(&tone(4), &mut packet).unwrap();
    let length = encoder.encode(&tone(5), &mut packet).unwrap();
    output.fill(9.0);
    assert_eq!(
        decoder.decode(&packet[..length], &mut output),
        Err(Error::Sequence {
            expected: 4,
            actual: 5
        })
    );
    assert_eq!(output, [9.0; FRAME_SAMPLES]);
    let info = decoder.conceal(&mut output).unwrap();
    assert_eq!(info.sequence, 4);
    assert!(info.concealed);
    decoder.decode(&packet[..length], &mut output).unwrap();
    assert!(
        output
            .iter()
            .all(|value| value.is_finite() && value.abs() <= 1.0)
    );
    for _ in 0..40 {
        decoder.conceal(&mut output).unwrap();
    }
    assert_eq!(output, [0.0; FRAME_SAMPLES]);
    encoder.reset();
    decoder.reset();
    let length = encoder.encode(&tone(0), &mut packet).unwrap();
    assert_eq!(
        decoder
            .decode(&packet[..length], &mut output)
            .unwrap()
            .sequence,
        0
    );
}

#[test]
fn silence_is_exact_and_a_finished_stream_can_be_reset_without_allocating() {
    let mut encoder = Encoder::new(Config::new(12_000).unwrap());
    let mut decoder = Decoder::new();
    let mut packet = [0; MAX_PACKET_BYTES];
    let mut output = [9.0; FRAME_SAMPLES];
    for _ in 0..10 {
        let length = encoder.encode(&[0.0; FRAME_SAMPLES], &mut packet).unwrap();
        decoder.decode(&packet[..length], &mut output).unwrap();
        assert_eq!(output, [0.0; FRAME_SAMPLES]);
    }
    let length = encoder.finish(&mut packet).unwrap();
    assert!(
        decoder
            .decode(&packet[..length], &mut output)
            .unwrap()
            .finished
    );
    encoder.reset();
    decoder.reset();
    let length = encoder.encode(&[0.0; FRAME_SAMPLES], &mut packet).unwrap();
    assert_eq!(
        decoder
            .decode(&packet[..length], &mut output)
            .unwrap()
            .sequence,
        0
    );
}

fn spectral_packet(gain: u32, amplitude: u32) -> Vec<u8> {
    let mut source = Encoder::new(Config::default());
    let mut packet = vec![0; 60];
    source.encode(&[0.0; FRAME_SAMPLES], &mut packet).unwrap();
    // Raw suffix: gain is first, followed by six-bit amplitudes, LSB first.
    for bit in 0..8 {
        let byte = 57 - bit / 8;
        packet[byte] =
            (packet[byte] & !(1 << (bit % 8))) | ((((gain >> bit) & 1) as u8) << (bit % 8));
    }
    for bit in 0..6 {
        let position = 8 + bit;
        let byte = 57 - position / 8;
        packet[byte] = (packet[byte] & !(1 << (position % 8)))
            | ((((amplitude >> bit) & 1) as u8) << (position % 8));
    }
    let mut crc = 0xffffu16;
    for &byte in &packet[..58] {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    packet[58..].copy_from_slice(&crc.to_le_bytes());
    packet
}

#[test]
fn invalid_spectral_parameters_with_valid_checksums_preserve_state() {
    let mut decoder = Decoder::new();
    for packet in [
        spectral_packet(137, 31),
        spectral_packet(128, 63),
        spectral_packet(0, 32),
    ] {
        assert!(Packet::parse(&packet).is_ok());
        let mut output = [9.0; FRAME_SAMPLES];
        assert!(decoder.decode(&packet, &mut output).is_err());
        assert_eq!(output, [9.0; FRAME_SAMPLES]);
    }
    decoder
        .decode(&spectral_packet(0, 31), &mut [0.0; FRAME_SAMPLES])
        .unwrap();
}

#[test]
fn independent_tracks_and_wrong_duration_do_not_share_or_consume_history() {
    let mut short_encoder =
        Encoder::new(Config::default().with_frame_duration(FrameDuration::Ms10));
    let mut long_encoder = Encoder::new(Config::default());
    let mut short_decoder = Decoder::with_frame_duration(FrameDuration::Ms10);
    let mut long_decoder = Decoder::new();
    let mut short_packet = [0; MAX_PACKET_BYTES];
    let mut long_packet = [0; MAX_PACKET_BYTES];
    for frame in 0..8 {
        let short_length = short_encoder
            .encode(&tone(frame)[..480], &mut short_packet)
            .unwrap();
        let long_length = long_encoder.encode(&tone(frame), &mut long_packet).unwrap();
        let mut output = [9.0; FRAME_SAMPLES];
        assert_eq!(
            long_decoder.decode(&short_packet[..short_length], &mut output),
            Err(Error::Profile)
        );
        assert_eq!(output, [9.0; FRAME_SAMPLES]);
        short_decoder
            .decode(&short_packet[..short_length], &mut output[..480])
            .unwrap();
        long_decoder
            .decode(&long_packet[..long_length], &mut output)
            .unwrap();
    }
}

#[test]
fn all_packet_budgets_encode_dense_noise_transients_and_high_frequencies() {
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let samples = duration.samples();
        let divisor = 8_000 / duration.milliseconds();
        for bytes in 12_000 / divisor..=32_000 / divisor {
            let config = Config::new(bytes * divisor)
                .unwrap()
                .with_frame_duration(duration);
            let mut encoder = Encoder::new(config);
            let mut decoder = Decoder::with_frame_duration(duration);
            let mut state = 0x17abc32eu32;
            let mut packet = [0; MAX_PACKET_BYTES];
            let mut output = vec![0.0; samples];
            for kind in 0..5 {
                let input: Vec<_> = (0..samples)
                    .map(|i| {
                        state ^= state << 13;
                        state ^= state >> 17;
                        state ^= state << 5;
                        match kind {
                            0 => (state as f64 / u32::MAX as f64 * 2.0 - 1.0) as f32,
                            1 => {
                                if i % 173 == 0 {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            2 => {
                                if i & 1 == 0 {
                                    1.0
                                } else {
                                    -1.0
                                }
                            }
                            3 => 1.0,
                            _ => (i as f32 * i as f32 * 0.001).sin(),
                        }
                    })
                    .collect();
                let length = encoder.encode(&input, &mut packet).unwrap();
                assert_eq!(length, bytes as usize);
                decoder.decode(&packet[..length], &mut output).unwrap();
                assert!(output.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
            }
        }
    }
}

#[test]
fn bitrate_changes_keep_sequence_overlap_and_fractional_budget() {
    let mut encoder = Encoder::new(Config::new(12_345).unwrap());
    let mut decoder = Decoder::new();
    let mut packet = [0; MAX_PACKET_BYTES];
    let mut output = [0.0; FRAME_SAMPLES];
    let mut bits = 0;
    let mut bytes = 0;
    for (frame, bitrate) in [12_345, 32_000, 12_000, 24_013, 16_003]
        .into_iter()
        .cycle()
        .take(300)
        .enumerate()
    {
        assert_eq!(encoder.set_bitrate(32_001), Err(Error::Config));
        encoder.set_bitrate(bitrate).unwrap();
        let length = encoder.encode(&tone(frame), &mut packet).unwrap();
        bits += bitrate;
        bytes += length;
        let info = decoder.decode(&packet[..length], &mut output).unwrap();
        assert_eq!(info.sequence, frame as u8);
        assert_eq!(bytes, (bits / 400) as usize);
    }
}
