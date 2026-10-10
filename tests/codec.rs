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

fn spectral_packet(gain: u32, bits: u32, amplitudes: &[u32]) -> Vec<u8> {
    let mut packet = vec![0; 60];
    packet[..4].copy_from_slice(&[b'S', b'N', 64, 0]);
    let mut count = 0;
    let mut log_combinations = 0.0f64;
    for candidate in 1..100 {
        log_combinations += ((961 - candidate) as f64 / candidate as f64).log2();
        if log_combinations.ceil() as u32 + bits * candidate + 12 > 54 * 8 {
            break;
        }
        count = candidate;
    }
    let mut writer = snacka::entropy::Encoder::new(&mut packet[4..58]);
    writer.raw(gain, 8).unwrap();
    writer.raw(u32::from(bits == 6), 1).unwrap();
    for index in 0..count {
        let available = 960 - index;
        let remaining = count - index;
        let probabilities = [0, (available - remaining) as u16, available as u16];
        writer
            .symbol(1, snacka::entropy::Model::new(&probabilities).unwrap())
            .unwrap();
        let zero = (1 << (bits - 1)) - 1;
        writer
            .raw(
                amplitudes.get(index as usize).copied().unwrap_or(zero),
                bits,
            )
            .unwrap();
    }
    writer.finish_padded().unwrap();
    repair_crc(&mut packet);
    packet
}

fn repair_crc(packet: &mut [u8]) {
    let end = packet.len() - 2;
    let mut crc = 0xffffu16;
    for &byte in &packet[..end] {
        crc ^= u16::from(byte) << 8;
        for _ in 0..8 {
            crc = if crc & 0x8000 != 0 {
                (crc << 1) ^ 0x1021
            } else {
                crc << 1
            };
        }
    }
    packet[end..].copy_from_slice(&crc.to_le_bytes());
}

#[test]
fn invalid_spectral_parameters_with_valid_checksums_preserve_state() {
    let mut decoder = Decoder::new();
    for packet in [
        spectral_packet(209, 4, &[]),
        spectral_packet(128, 4, &[15]),
        spectral_packet(128, 6, &[63]),
        spectral_packet(0, 4, &[8]),
        spectral_packet(0, 6, &[32]),
    ] {
        assert!(Packet::parse(&packet).is_ok());
        let mut output = [9.0; FRAME_SAMPLES];
        assert!(decoder.decode(&packet, &mut output).is_err());
        assert_eq!(output, [9.0; FRAME_SAMPLES]);
    }
    decoder
        .decode(&spectral_packet(0, 4, &[]), &mut [0.0; FRAME_SAMPLES])
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

#[test]
fn both_quantizers_and_gain_boundaries_match_independent_inverse_cosines() {
    let samples = FRAME_SAMPLES;
    let overlap = ALGORITHMIC_DELAY;
    let start = (samples - overlap) / 2;
    for bits in [4, 6] {
        let radius = (1i32 << (bits - 1)) - 1;
        let quantized = [radius, 1 - radius, radius / 3, -(radius / 5)];
        let amplitudes: Vec<_> = quantized.iter().map(|x| (x + radius) as u32).collect();
        for code in [1, 63, 64, 65, 128, 192, 208] {
            let exponent = if code <= 64 {
                -16.0 + f64::from(code) / 8.0
            } else {
                -8.0 + f64::from(code - 64) / 16.0
            };
            let gain = 2.0f64.powf(exponent);
            let mut decoder = Decoder::new();
            let mut output = [0.0; FRAME_SAMPLES];
            decoder
                .decode(&spectral_packet(code, bits, &amplitudes), &mut output)
                .unwrap();
            for (index, &actual) in output.iter().enumerate() {
                let position = start + index;
                let reflected = position.min(2 * samples - 1 - position);
                let window = if reflected >= start + overlap {
                    1.0
                } else {
                    let angle = std::f64::consts::FRAC_PI_2 * ((reflected - start) as f64 + 0.5)
                        / overlap as f64;
                    (std::f64::consts::FRAC_PI_2 * angle.sin().powi(2)).sin()
                };
                let expected: f64 = quantized
                    .iter()
                    .enumerate()
                    .map(|(frequency, &q)| {
                        let coefficient = if bits == 4 {
                            f64::from(q * q.abs()) * gain / 49.0
                        } else {
                            f64::from(q) * gain / 31.0
                        };
                        let phase = std::f64::consts::PI / samples as f64
                            * (position as f64 + 0.5 + samples as f64 / 2.0)
                            * (frequency as f64 + 0.5);
                        coefficient * phase.cos()
                    })
                    .sum::<f64>()
                    * window;
                assert!(
                    (f64::from(actual) - expected.clamp(-1.0, 1.0)).abs() < 2e-5,
                    "bits={bits}, gain={code}, position={index}: {actual} versus {expected}"
                );
            }
        }
    }
}

#[test]
fn incompatible_versions_are_rejected_before_consuming_history() {
    let mut packet = spectral_packet(0, 4, &[]);
    let mut decoder = Decoder::new();
    let mut output = [9.0; FRAME_SAMPLES];
    for version in [0, 1, 3, 7] {
        packet[2] = version << 5;
        repair_crc(&mut packet);
        assert_eq!(decoder.decode(&packet, &mut output), Err(Error::Version));
        assert_eq!(output, [9.0; FRAME_SAMPLES]);
    }
    packet[2] = 2 << 5;
    repair_crc(&mut packet);
    decoder.decode(&packet, &mut output).unwrap();
    assert_eq!(output, [0.0; FRAME_SAMPLES]);
}
