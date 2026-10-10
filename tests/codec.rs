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
    profile_packet(gain, bits, amplitudes, 0)
}

fn profile_packet(gain: u32, bits: u32, amplitudes: &[u32], profile: u8) -> Vec<u8> {
    let length = if profile & 1 == 0 { 60 } else { 30 };
    profile_packet_with_length(gain, bits, amplitudes, profile, length).0
}

fn profile_packet_with_length(
    gain: u32,
    bits: u32,
    amplitudes: &[u32],
    profile: u8,
    length: usize,
) -> (Vec<u8>, usize) {
    let samples = if profile & 1 == 0 { 960usize } else { 480 };
    let bins = samples / (1 + usize::from(profile >> 1));
    let mut packet = vec![0; length];
    let end = packet.len() - 2;
    packet[..2].copy_from_slice(&[128 | profile, 0]);
    let mut count = 0;
    let mut log_combinations = 0.0f64;
    for candidate in 1..=bins {
        log_combinations += ((bins + 1 - candidate) as f64 / candidate as f64).log2();
        if log_combinations.ceil() as usize + bits as usize * candidate + 12 > (end - 2) * 8 {
            break;
        }
        count = candidate;
    }
    let mut writer = snacka::entropy::Encoder::new(&mut packet[2..end]);
    writer.raw(gain, 8).unwrap();
    writer.raw(u32::from(bits == 6), 1).unwrap();
    for index in 0..count {
        let available = bins - index;
        let remaining = count - index;
        let probabilities = [0, (available - remaining) as u16, available as u16];
        writer
            .symbol(1, snacka::entropy::Model::new(&probabilities).unwrap())
            .unwrap();
        let zero = (1 << (bits - 1)) - 1;
        writer
            .raw(amplitudes.get(index).copied().unwrap_or(zero), bits)
            .unwrap();
    }
    writer.finish_padded().unwrap();
    repair_crc(&mut packet);
    (packet, count)
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
    for profile in 0..6 {
        let duration = if profile & 1 == 0 {
            FrameDuration::Ms20
        } else {
            FrameDuration::Ms10
        };
        let samples = duration.samples();
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
                let mut decoder = Decoder::with_frame_duration(duration);
                let mut output = vec![0.0; samples];
                decoder
                    .decode(
                        &profile_packet(code, bits, &amplitudes, profile),
                        &mut output,
                    )
                    .unwrap();
                for (index, &actual) in output.iter().enumerate() {
                    let position = start + index;
                    let reflected = position.min(2 * samples - 1 - position);
                    let window = if reflected >= start + overlap {
                        1.0
                    } else {
                        let angle = std::f64::consts::FRAC_PI_2
                            * ((reflected - start) as f64 + 0.5)
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
}

#[test]
fn incompatible_versions_are_rejected_before_consuming_history() {
    let mut packet = spectral_packet(0, 4, &[]);
    let mut decoder = Decoder::new();
    let mut output = [9.0; FRAME_SAMPLES];
    for version in [0, 1, 2, 3, 5, 6, 7] {
        packet[0] = version << 5;
        repair_crc(&mut packet);
        assert_eq!(decoder.decode(&packet, &mut output), Err(Error::Version));
        assert_eq!(output, [9.0; FRAME_SAMPLES]);
    }
    packet[0] = 4 << 5;
    repair_crc(&mut packet);
    decoder.decode(&packet, &mut output).unwrap();
    assert_eq!(output, [0.0; FRAME_SAMPLES]);
}

#[test]
fn concealed_periodic_audio_preserves_phase_and_long_losses_reach_exact_silence() {
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let samples = duration.samples();
        for frequency in [50.0, 80.0, 117.3, 220.0, 497.0] {
            let mut encoder = Encoder::new(Config::default().with_frame_duration(duration));
            let mut decoder = Decoder::with_frame_duration(duration);
            let mut packet = [0; MAX_PACKET_BYTES];
            let mut output = vec![0.0; samples];
            for frame in 0..9 {
                let input: Vec<_> = (0..samples)
                    .map(|index| {
                        let time = (frame * samples + index) as f64 / 48_000.0;
                        (0.6 * (std::f64::consts::TAU * frequency * time).sin()) as f32
                    })
                    .collect();
                let length = encoder.encode(&input, &mut packet).unwrap();
                decoder.decode(&packet[..length], &mut output).unwrap();
            }
            decoder.conceal(&mut output).unwrap();
            let mut energy = 0.0;
            let mut error = 0.0;
            for (index, &actual) in output.iter().enumerate() {
                let time = (9 * samples + index - ALGORITHMIC_DELAY) as f64 / 48_000.0;
                let expected = 0.6 * (std::f64::consts::TAU * frequency * time).sin();
                energy += expected * expected;
                error += (expected - f64::from(actual)).powi(2);
            }
            let snr = 10.0 * (energy / error).log10();
            assert!(snr > 14.0, "{duration:?} {frequency} Hz: {snr} dB");
            for _ in 0..12 {
                decoder.conceal(&mut output).unwrap();
                assert!(output.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
            }
            assert!(output.iter().all(|&x| x == 0.0));
            decoder.reset();
            decoder.conceal(&mut output).unwrap();
            assert!(output.iter().all(|&x| x == 0.0));
        }
    }
}

#[test]
fn concealment_refusal_preserves_pitch_history_and_recovery() {
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let samples = duration.samples();
        let mut encoder = Encoder::new(Config::default().with_frame_duration(duration));
        let mut decoder = Decoder::with_frame_duration(duration);
        let mut control = Decoder::with_frame_duration(duration);
        let mut packet = [0; MAX_PACKET_BYTES];
        let mut output = vec![0.0; samples];
        let mut expected = vec![0.0; samples];
        for frame in 0..7 {
            let input: Vec<_> = (0..samples)
                .map(|index| (0.5 * (0.027 * (frame * samples + index) as f64).sin()) as f32)
                .collect();
            let length = encoder.encode(&input, &mut packet).unwrap();
            if frame == 4 || frame == 5 {
                let mut short = [7.0; 12];
                assert_eq!(decoder.conceal(&mut short), Err(Error::Output));
                assert_eq!(short, [7.0; 12]);
                decoder.conceal(&mut output).unwrap();
                control.conceal(&mut expected).unwrap();
                assert_eq!(
                    decoder.decode(&packet[..length], &mut output),
                    Err(Error::Sequence {
                        expected: (frame + 1) as u8,
                        actual: frame as u8,
                    })
                );
            } else {
                decoder.decode(&packet[..length], &mut output).unwrap();
                control.decode(&packet[..length], &mut expected).unwrap();
            }
            assert_eq!(output, expected);
        }
    }
}

#[test]
fn loss_recovery_error_cannot_propagate_beyond_the_short_overlap() {
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let samples = duration.samples();
        let mut encoder = Encoder::new(Config::default().with_frame_duration(duration));
        let mut decoder = Decoder::with_frame_duration(duration);
        let mut uninterrupted = Decoder::with_frame_duration(duration);
        let mut packet = [0; MAX_PACKET_BYTES];
        let mut output = vec![0.0; samples];
        let mut expected = vec![0.0; samples];
        for frame in 0..8 {
            let input: Vec<_> = (0..samples)
                .map(|index| {
                    let position = (frame * samples + index) as f64;
                    (0.3 * (position * 0.023).sin() + 0.2 * (position * 0.071).sin()) as f32
                })
                .collect();
            let length = encoder.encode(&input, &mut packet).unwrap();
            uninterrupted
                .decode(&packet[..length], &mut expected)
                .unwrap();
            if frame == 4 || frame == 5 {
                decoder.conceal(&mut output).unwrap();
            } else {
                decoder.decode(&packet[..length], &mut output).unwrap();
                let uncertain = if frame == 6 { ALGORITHMIC_DELAY } else { 0 };
                assert_eq!(&output[uncertain..], &expected[uncertain..]);
            }
        }
    }
}

#[test]
fn unknown_bandwidth_profiles_are_refused_without_consuming_history() {
    let mut packet = spectral_packet(0, 4, &[]);
    let mut decoder = Decoder::new();
    let mut output = [7.0; FRAME_SAMPLES];
    for profile in 6..16 {
        packet[0] = 128 | profile;
        repair_crc(&mut packet);
        assert_eq!(decoder.decode(&packet, &mut output), Err(Error::Profile));
        assert_eq!(output, [7.0; FRAME_SAMPLES]);
    }
    for profile in [0, 2, 4] {
        let mut packet = profile_packet(0, 4, &[], profile);
        packet[1] = profile / 2;
        repair_crc(&mut packet);
        let parsed = Packet::parse(&packet).unwrap();
        assert_eq!(parsed.bandwidth_hz(), 24_000 / (1 + u32::from(profile / 2)));
        decoder.decode(&packet, &mut output).unwrap();
        assert!(output.iter().all(|&value| value == 0.0));
    }
}

#[test]
fn adaptive_bandwidth_retains_high_frequency_audio_and_changes_without_reset() {
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let samples = duration.samples();
        let mut encoder = Encoder::new(Config::default().with_frame_duration(duration));
        let mut decoder = Decoder::with_frame_duration(duration);
        let mut packet = [0; MAX_PACKET_BYTES];
        let mut output = vec![0.0; samples];
        let mut counts = [0usize; 3];
        for frame in 0..60 {
            let input: Vec<_> = (0..samples)
                .map(|index| {
                    let time = (frame * samples + index) as f64 / 48_000.0;
                    (0..46)
                        .map(|tone| {
                            let (start, spacing) = match frame / 20 {
                                0 => (8, samples / 240),
                                1 => (samples / 3 + 8, samples / 480),
                                _ => (samples / 2 + 8, samples / 160),
                            };
                            let bin = start + tone * spacing;
                            let frequency = (bin as f64 + 0.5) * 24_000.0 / samples as f64;
                            0.014
                                * (std::f64::consts::TAU * frequency * time + tone as f64 * 0.718)
                                    .sin()
                        })
                        .sum::<f64>() as f32
                })
                .collect();
            let length = encoder.encode(&input, &mut packet).unwrap();
            let parsed = Packet::parse(&packet[..length]).unwrap();
            assert_eq!(parsed.frame_duration(), duration);
            if frame % 20 >= 4 {
                assert_eq!(parsed.bandwidth_hz(), [8_000, 12_000, 24_000][frame / 20]);
            }
            counts[(24_000 / parsed.bandwidth_hz() - 1) as usize] += 1;
            decoder.decode(&packet[..length], &mut output).unwrap();
            assert!(output.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
        }
        assert!(counts.iter().all(|&count| count >= 16));
    }
}

#[test]
fn all_bandwidth_budgets_match_independently_constructed_cosine_packets() {
    for profile in 0..6 {
        let duration = if profile & 1 == 0 {
            FrameDuration::Ms20
        } else {
            FrameDuration::Ms10
        };
        let samples = duration.samples();
        let divisor = 8_000 / duration.milliseconds() as usize;
        let start = (samples - ALGORITHMIC_DELAY) / 2;
        for bits in [4, 6] {
            let radius = (1u32 << (bits - 1)) - 1;
            let amplitudes: Vec<_> = (0..100)
                .map(|i| if i % 2 == 0 { 0 } else { 2 * radius })
                .collect();
            for length in 12_000 / divisor..=32_000 / divisor {
                let (packet, count) =
                    profile_packet_with_length(64, bits, &amplitudes, profile, length);
                assert!(count <= 83);
                let mut decoder = Decoder::with_frame_duration(duration);
                let mut output = vec![0.0; samples];
                decoder.decode(&packet, &mut output).unwrap();
                for (index, &actual) in output.iter().enumerate() {
                    let position = start + index;
                    let reflected = position.min(2 * samples - 1 - position);
                    let window = if reflected >= start + ALGORITHMIC_DELAY {
                        1.0
                    } else {
                        let angle = std::f64::consts::FRAC_PI_2
                            * ((reflected - start) as f64 + 0.5)
                            / ALGORITHMIC_DELAY as f64;
                        (std::f64::consts::FRAC_PI_2 * angle.sin().powi(2)).sin()
                    };
                    let expected: f64 = (0..count)
                        .map(|frequency| {
                            let sign = if frequency % 2 == 0 { -1.0 } else { 1.0 };
                            let phase = std::f64::consts::PI / samples as f64
                                * (position as f64 + 0.5 + samples as f64 / 2.0)
                                * (frequency as f64 + 0.5);
                            sign * phase.cos() / 256.0
                        })
                        .sum::<f64>()
                        * window;
                    assert!(
                        (f64::from(actual) - expected).abs() < 1e-6,
                        "profile={profile} bits={bits} length={length} sample={index}"
                    );
                }
            }
        }
    }
}

#[test]
fn complete_version_three_packets_are_rejected_without_consuming_history() {
    let legacy_short = [
        83, 78, 97, 0, 255, 255, 255, 255, 252, 14, 238, 238, 0, 235, 39,
    ];
    let legacy_long = [
        83, 78, 96, 0, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 255, 224,
        238, 238, 238, 238, 238, 238, 238, 238, 0, 130, 8,
    ];
    for (duration, bytes) in [
        (FrameDuration::Ms10, legacy_short.as_slice()),
        (FrameDuration::Ms20, legacy_long.as_slice()),
    ] {
        let mut checksummed = bytes.to_vec();
        repair_crc(&mut checksummed);
        assert_eq!(checksummed, bytes);
        let mut encoder = Encoder::new(Config::default().with_frame_duration(duration));
        let mut decoder = Decoder::with_frame_duration(duration);
        let mut output = vec![9.0; duration.samples()];
        assert_eq!(decoder.decode(bytes, &mut output), Err(Error::Version));
        assert!(output.iter().all(|&x| x == 9.0));
        let mut packet = [0; MAX_PACKET_BYTES];
        let length = encoder
            .encode(&vec![0.0; duration.samples()], &mut packet)
            .unwrap();
        let info = decoder.decode(&packet[..length], &mut output).unwrap();
        assert_eq!(info.sequence, 0);
        assert!(output.iter().all(|&x| x == 0.0));
    }
}

#[test]
fn out_of_range_packet_lengths_preserve_decoder_history() {
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let config = Config::new(12_000).unwrap().with_frame_duration(duration);
        let mut encoder = Encoder::new(config);
        let mut decoder = Decoder::with_frame_duration(duration);
        let mut packet = [0; MAX_PACKET_BYTES];
        let length = encoder
            .encode(&vec![0.0; duration.samples()], &mut packet)
            .unwrap();
        let mut output = vec![9.0; duration.samples()];
        for end in 0..length {
            assert_eq!(
                decoder.decode(&packet[..end], &mut output),
                Err(Error::Packet)
            );
            assert!(output.iter().all(|&x| x == 9.0));
        }
        let maximum = Config::new(32_000)
            .unwrap()
            .with_frame_duration(duration)
            .maximum_packet_bytes();
        for excessive in [maximum + 1, MAX_PACKET_BYTES + 1, 1024] {
            let mut malformed = packet[..length].to_vec();
            malformed.resize(excessive, 0);
            repair_crc(&mut malformed);
            assert_eq!(decoder.decode(&malformed, &mut output), Err(Error::Packet));
            assert!(output.iter().all(|&x| x == 9.0));
        }
        decoder.decode(&packet[..length], &mut output).unwrap();
        assert!(output.iter().all(|&x| x == 0.0));
    }
}

#[test]
fn checksum_rejects_contiguous_bit_flips_through_sixteen_bits_for_every_profile() {
    for profile in 0..6 {
        let divisor = if profile & 1 == 0 { 400 } else { 800 };
        for length in [12_000 / divisor, 32_000 / divisor] {
            let (packet, _) = profile_packet_with_length(64, 4, &[], profile, length);
            assert_eq!(Packet::parse(&packet).unwrap().payload().len(), length - 4);
            for burst in 1..=16 {
                for start in 0..=packet.len() * 8 - burst {
                    let mut damaged = packet.clone();
                    for bit in start..start + burst {
                        damaged[bit / 8] ^= 1 << (7 - bit % 8);
                    }
                    assert!(
                        Packet::parse(&damaged).is_err(),
                        "profile={profile} length={length} start={start} burst={burst}"
                    );
                }
            }
        }
    }
}
