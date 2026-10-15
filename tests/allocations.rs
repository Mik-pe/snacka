// SPDX-License-Identifier: MIT
use snacka::FrameDuration;
use snacka::entropy::{Decoder, Encoder, Model};
use snacka::transform::{Complex, FftPlan, MdctPlan, MdctWorkspace};
use snacka::{
    Config, Decoder as AudioDecoder, Encoder as AudioEncoder, FRAME_SAMPLES, MAX_PACKET_BYTES,
    Packet,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::Duration;

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

fn count() {
    let _ = ALLOCATIONS.try_with(|counter| {
        if let Some(previous) = counter.get() {
            counter.set(Some(previous + 1));
        }
    });
}

struct Tracked;
// SAFETY: Every allocation operation delegates its exact arguments to System.
// Counting uses allocation-free thread local storage and does not affect pointers.
unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: The allocator contract is forwarded to System unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: The allocator contract is forwarded to System unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: System allocated the pointer with this layout.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        // SAFETY: The allocator contract is forwarded to System unchanged.
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Tracked = Tracked;

#[cfg(feature = "opus")]
#[test]
fn opus_pcm_format_conversion_never_allocates() {
    use snacka::opus::pcm;
    let source16 = [i16::MIN, -1, 0, 1, i16::MAX];
    let source24 = [i32::MIN, -8388608, 0, 8388607, i32::MAX];
    let mut floating = [0.0; 8];
    let mut result16 = [0; 8];
    let mut result24 = [0; 8];
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    for _ in 0..10 {
        pcm::i16_to_f32(black_box(&source16), black_box(&mut floating)).unwrap();
        pcm::i24_to_f32(black_box(&source24), black_box(&mut floating)).unwrap();
        pcm::f32_to_i16(black_box(&floating), black_box(&mut result16)).unwrap();
        pcm::f32_to_i24(black_box(&floating), black_box(&mut result24)).unwrap();
        assert_eq!(
            pcm::f32_to_i16(&[f32::NAN], &mut result16),
            Err(pcm::Error::Input)
        );
        assert_eq!(
            pcm::f32_to_i24(&floating, &mut result24[..1]),
            Err(pcm::Error::Output)
        );
    }
    let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(allocations, 0);
}

#[cfg(feature = "opus")]
#[test]
fn opus_processing_reset_and_rate_changes_do_not_allocate() {
    use snacka::opus::{
        Bandwidth, CeltConfig, CeltDecoder, CeltEncoder, CeltFrameDuration, Decoder, Recovery,
        multistream::{ChannelMapping, MultistreamDecoder, MultistreamPacket},
        ogg::{OggDecoder, OpusHead, OpusIdentification},
        projection::{ProjectionDecoder, ProjectionMapping},
    };

    let durations = [
        CeltFrameDuration::Ms2_5,
        CeltFrameDuration::Ms5,
        CeltFrameDuration::Ms10,
        CeltFrameDuration::Ms20,
    ];
    // Keep this twelve-codec test harness on the heap before tracking begins;
    // inline aggregate construction exceeds small debug test-thread stacks.
    let mut encoders: Vec<_> = (0..8)
        .map(|index| {
            CeltEncoder::new(
                CeltConfig::new(
                    32_000,
                    durations[index / 2],
                    index % 2 + 1,
                    Bandwidth::Fullband,
                )
                .unwrap(),
            )
        })
        .collect();
    let mut celt: Vec<_> = (1..=2)
        .map(|channels| CeltDecoder::new(channels).unwrap())
        .collect();
    let mut universal: Vec<_> = (1..=2)
        .map(|channels| Decoder::new(channels).unwrap())
        .collect();
    let output_rates = [8_000u32, 12_000, 16_000, 24_000, 48_000];
    let mut mobile: Vec<_> = output_rates
        .into_iter()
        .flat_map(|rate| {
            (1..=2).map(move |channels| Decoder::with_sample_rate(channels, rate).unwrap())
        })
        .collect();
    let mapping = ChannelMapping::new(2, 1, &[2, 0, 1, 255, 0]).unwrap();
    let mut mapped: Vec<_> = output_rates
        .into_iter()
        .map(|rate| MultistreamDecoder::with_sample_rate(mapping, rate).unwrap())
        .collect();
    let mut mapped_output = vec![0.0; 28_800];
    let projection = ProjectionMapping::new(
        2,
        1,
        4,
        vec![
            32_767, 0, 0, 8192, 0, 32_767, 0, -8192, 0, 0, 32_767, 16_384,
        ],
    )
    .unwrap();
    let mut projected_gain: Vec<_> = output_rates
        .into_iter()
        .map(|rate| ProjectionDecoder::with_sample_rate(projection.clone(), rate).unwrap())
        .collect();
    let identification = OpusIdentification::projected(
        OpusHead {
            channels: 4,
            pre_skip: 120,
            input_sample_rate: 48_000,
            output_gain_q8: 0,
        },
        projection,
    )
    .unwrap();
    let mut projected: Vec<_> = output_rates
        .into_iter()
        .map(|rate| OggDecoder::with_sample_rate(&identification, rate).unwrap())
        .collect();
    let input: [f32; 1920] = std::array::from_fn(|index| (index as f32 * 0.057).sin() * 0.25);
    let mut packet = [0; 1276];
    let mut transport = [0; 2554];
    let mut output = [0.0; 11520];

    // Assemble coded SILK packets before counting allocations. The independent
    // fixture covers all native SILK rates, channels, durations and LBRR.
    let data = include_bytes!("fixtures/opus-silk-decode.bin");
    let cases = u32::from_le_bytes(data[8..12].try_into().unwrap()) as usize;
    let mut packets = Vec::with_capacity(cases);
    let mut position = 12;
    for _ in 0..cases {
        let band = data[position + 2];
        let duration = data[position + 3];
        let channels = usize::from(data[position + 4]);
        let length =
            u16::from_le_bytes(data[position + 5..position + 7].try_into().unwrap()) as usize;
        let samples =
            u16::from_le_bytes(data[position + 7..position + 9].try_into().unwrap()) as usize;
        let reset = u32::from_le_bytes(data[position + 13..position + 17].try_into().unwrap()) != 0;
        let duration_code = match duration {
            10 => 0,
            20 => 1,
            40 => 2,
            60 => 3,
            _ => unreachable!(),
        };
        position += 21;
        let mut packet = Vec::with_capacity(length + 1);
        packet.push(((band * 4 + duration_code) << 3) | ((channels as u8 - 1) << 2));
        packet.extend_from_slice(&data[position..position + length]);
        packets.push((channels, reset, packet));
        position += length + samples * channels * 2;
    }
    assert_eq!(position, data.len());
    let mapped_packets: Vec<_> = packets
        .iter()
        .map(|(_, reset, packet)| {
            let mut transport = Vec::with_capacity(packet.len() * 2 + 2);
            transport.push(packet[0]);
            let length = packet.len() - 1;
            if length < 252 {
                transport.push(length as u8);
            } else {
                let first = 252 + length % 4;
                transport.push(first as u8);
                transport.push(((length - first) / 4) as u8);
            }
            transport.extend_from_slice(&packet[1..]);
            transport.extend_from_slice(packet);
            (*reset, transport)
        })
        .collect();

    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    let mut actual_recovery = false;
    let mut actual_mapped_recovery = false;
    let mut actual_partial_recovery = false;
    let mut actual_projected_recovery = false;
    let mut actual_projected_gain_recovery = false;
    for (index, encoder) in encoders.iter_mut().enumerate() {
        let channels = index % 2 + 1;
        for frame in 0..64 {
            universal[channels - 1].set_gain_q8(if frame & 1 == 0 { 3072 } else { 0 });
            encoder
                .set_bitrate(if frame & 1 == 0 { 32_000 } else { 48_000 })
                .unwrap();
            let length = encoder
                .encode(
                    &input[..durations[index / 2].samples() * channels],
                    &mut packet,
                )
                .unwrap();
            celt[channels - 1]
                .decode(&packet[..length], &mut output)
                .unwrap();
            universal[channels - 1]
                .decode(&packet[..length], &mut output)
                .unwrap();
        }
        encoder.reset();
        celt[channels - 1].reset();
        universal[channels - 1].reset();
        celt[channels - 1]
            .conceal(durations[index / 2].samples(), &mut output)
            .unwrap();
    }
    for (channels, reset, packet) in &packets {
        if *reset {
            universal[*channels - 1].reset();
        }
        universal[*channels - 1]
            .decode(packet, &mut output)
            .unwrap();
        black_box(&output);
        for (decoder, rate) in mobile
            .iter_mut()
            .zip(output_rates.into_iter().flat_map(|rate| [rate; 2]))
        {
            decoder.set_gain_q8(if *reset { i16::MIN } else { 3072 });
            if *reset {
                decoder.reset();
            }
            let samples = decoder.decode(packet, &mut output).unwrap();
            assert_eq!(decoder.last_packet_samples(), samples);
            // Exercise exact 2.5 ms partial LP prediction and both actual LBRR
            // recovery and absent-redundancy fallback without new storage.
            decoder.conceal(rate as usize / 400, &mut output).unwrap();
            assert_eq!(decoder.last_packet_samples(), rate as usize / 400);
            actual_recovery |=
                decoder.recover(packet, samples, &mut output).unwrap() == Recovery::Fec;
            actual_partial_recovery |= decoder
                .recover(packet, rate as usize * 3 / 400, &mut output)
                .unwrap()
                == Recovery::Fec;
            assert_eq!(decoder.last_packet_samples(), rate as usize * 3 / 400);
            decoder.decode(packet, &mut output).unwrap();
            black_box(&output);
        }
    }
    for ((reset, packet), (_, _, ordinary)) in mapped_packets.iter().zip(&packets) {
        let length = MultistreamPacket::write(&[ordinary, ordinary], &mut transport).unwrap();
        assert_eq!(&transport[..length], packet);
        let packed = MultistreamPacket::parse(&transport[..length], 2).unwrap();
        assert_eq!(packed.streams(), 2);
        for (decoder, rate) in mapped.iter_mut().zip(output_rates) {
            let gain = if *reset { i16::MIN } else { 3072 };
            decoder.set_gain_q8(gain);
            if *reset {
                decoder.reset();
            }
            assert_eq!(decoder.gain_q8(), gain);
            let samples = decoder.decode(packet, &mut mapped_output).unwrap();
            decoder
                .conceal(rate as usize / 400, &mut mapped_output)
                .unwrap();
            actual_mapped_recovery |= decoder
                .recover(packet, samples, &mut mapped_output)
                .unwrap()
                == Recovery::Fec;
            decoder.decode(packet, &mut mapped_output).unwrap();
            assert!(
                mapped_output[..samples * 5]
                    .as_chunks::<5>()
                    .0
                    .iter()
                    .all(|sample| sample[3] == 0.0)
            );
            black_box(&mapped_output);
        }
        for (decoder, rate) in projected.iter_mut().zip(output_rates) {
            if *reset {
                decoder.reset();
            }
            let samples = decoder.decode(packet, &mut mapped_output).unwrap();
            decoder
                .conceal(rate as usize / 400, &mut mapped_output)
                .unwrap();
            actual_projected_recovery |= decoder
                .recover(packet, samples, &mut mapped_output)
                .unwrap()
                == Recovery::Fec;
            decoder
                .recover(packet, rate as usize * 3 / 400, &mut mapped_output)
                .unwrap();
            decoder.decode(packet, &mut mapped_output).unwrap();
            assert!(
                mapped_output[..samples * 4]
                    .iter()
                    .all(|sample| sample.is_finite())
            );
            black_box(&mapped_output);
        }
        for (decoder, rate) in projected_gain.iter_mut().zip(output_rates) {
            let gain = if *reset { i16::MAX } else { 3072 };
            decoder.set_gain_q8(gain);
            if *reset {
                decoder.reset();
            }
            assert_eq!(decoder.gain_q8(), gain);
            let samples = decoder.decode(packet, &mut mapped_output).unwrap();
            decoder
                .conceal(rate as usize / 400, &mut mapped_output)
                .unwrap();
            actual_projected_gain_recovery |= decoder
                .recover(packet, samples, &mut mapped_output)
                .unwrap()
                == Recovery::Fec;
            decoder.decode(packet, &mut mapped_output).unwrap();
            black_box(&mapped_output);
        }
    }
    let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(allocations, 0);
    assert!(
        actual_recovery,
        "allocation gate must execute real in-band FEC"
    );
    assert!(
        actual_mapped_recovery,
        "mapped allocation gate must execute real in-band FEC"
    );
    assert!(
        actual_partial_recovery,
        "allocation gate must execute a true partial redundancy tail"
    );
    assert!(
        actual_projected_recovery,
        "Ogg projection allocation gate must execute real in-band FEC"
    );
    assert!(
        actual_projected_gain_recovery,
        "projection gain allocation gate must execute real in-band FEC"
    );
}

#[cfg(feature = "opus")]
#[test]
fn opus_call_controls_and_generated_redundancy_do_not_allocate() {
    use snacka::opus::{
        Application, Bandwidth, Decoder, Encoder, EncoderConfig, FrameDuration, RateControl,
        Recovery,
    };

    let speech: Vec<_> = (1..=2)
        .map(|channels| {
            EncoderConfig::new(
                96_000 * channels as u32,
                channels,
                FrameDuration::Ms20,
                Bandwidth::Wideband,
                Application::Voice,
            )
            .unwrap()
            .with_fec(true)
            .unwrap()
            .with_dtx(true)
            .unwrap()
        })
        .collect();
    let audio: Vec<_> = (1..=2)
        .map(|channels| {
            EncoderConfig::new(
                96_000 * channels as u32,
                channels,
                FrameDuration::Ms20,
                Bandwidth::Fullband,
                Application::Audio,
            )
            .unwrap()
            .with_rate_control(RateControl::Constant)
        })
        .collect();
    let hybrid: Vec<_> = (1..=2)
        .map(|channels| {
            EncoderConfig::new(
                96_000 * channels as u32,
                channels,
                FrameDuration::Ms40,
                Bandwidth::Superwideband,
                Application::Voice,
            )
            .unwrap()
            .with_rate_control(RateControl::Constant)
            .with_fec(true)
            .unwrap()
        })
        .collect();
    let mut encoders: Vec<_> = speech
        .iter()
        .copied()
        .map(|config| Encoder::new(config).unwrap())
        .collect();
    let mut decoders: Vec<_> = (1..=2)
        .map(|channels| Decoder::new(channels).unwrap())
        .collect();
    let mut input = [0.0f32; 5760];
    let silence = [0.0f32; 1920];
    let mut output = [0.0f32; 11520];
    let mut packet = [0u8; 4096];

    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    let mut generated_fec = false;
    let mut suppressed = false;
    let mut refreshed = false;
    let mut beginning = false;
    let mut ending = false;
    for (index, (encoder, decoder)) in encoders.iter_mut().zip(&mut decoders).enumerate() {
        let channels = index + 1;
        for frame in 0..20 {
            for sample in 0..960 {
                for channel in 0..channels {
                    input[sample * channels + channel] = 0.15
                        * ((frame * 960 + sample) as f32
                            * std::f32::consts::TAU
                            * (180.0 + channel as f32 * 40.0)
                            / 48_000.0)
                            .sin();
                }
            }
            let length = encoder
                .encode(&input[..960 * channels], &mut packet)
                .unwrap();
            if encoder.has_fec() {
                generated_fec |= decoder
                    .recover(&packet[..length], 960, &mut output)
                    .unwrap()
                    == Recovery::Fec;
            }
            assert_eq!(decoder.decode(&packet[..length], &mut output).unwrap(), 960);
            assert_eq!(decoder.final_range(), encoder.final_range());
        }
        for frame in 0..40 {
            let length = encoder
                .encode(&silence[..960 * channels], &mut packet)
                .unwrap();
            suppressed |= encoder.was_dtx();
            refreshed |= frame >= 10 && !encoder.was_dtx();
            assert_eq!(decoder.decode(&packet[..length], &mut output).unwrap(), 960);
            assert_eq!(decoder.final_range(), encoder.final_range());
        }
        let length = encoder
            .encode(&input[..960 * channels], &mut packet)
            .unwrap();
        assert!(!encoder.was_dtx());
        decoder.decode(&packet[..length], &mut output).unwrap();
        assert_eq!(decoder.final_range(), encoder.final_range());

        for lp in [speech[index], hybrid[index]] {
            encoder.reconfigure(audio[index]).unwrap();
            let length = encoder
                .encode(&input[..960 * channels], &mut packet)
                .unwrap();
            decoder.decode(&packet[..length], &mut output).unwrap();
            assert_eq!(decoder.final_range(), encoder.final_range());
            encoder.reconfigure(lp).unwrap();
            for frame in 0..3 {
                if frame == 2 {
                    encoder.prepare_reconfigure(audio[index]).unwrap();
                }
                let samples = encoder.config().duration().samples();
                let length = encoder
                    .encode(&input[..samples * channels], &mut packet)
                    .unwrap();
                beginning |= frame == 0 && encoder.has_transition_redundancy();
                ending |= frame == 2 && encoder.has_transition_redundancy();
                assert_eq!(
                    decoder.decode(&packet[..length], &mut output).unwrap(),
                    samples
                );
                assert_eq!(decoder.final_range(), encoder.final_range());
            }
            assert_eq!(encoder.config(), audio[index]);
            let length = encoder
                .encode(&input[..960 * channels], &mut packet)
                .unwrap();
            decoder.decode(&packet[..length], &mut output).unwrap();
            assert_eq!(decoder.final_range(), encoder.final_range());
        }
        encoder.reset();
        decoder.reset();
        black_box(&output);
    }
    let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(allocations, 0);
    assert!(
        generated_fec,
        "generated LBRR must reach allocation tracking"
    );
    assert!(
        suppressed && refreshed,
        "DTX suppression and refresh must both run"
    );
    assert!(
        beginning && ending,
        "both transition redundancy positions must run"
    );
}

#[test]
fn adaptation_construction_feedback_updates_and_timeouts_never_allocate() {
    use snacka::adaptive::{BitrateController, Feedback};
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    let mut rate = BitrateController::new(24_000, 12_000, 32_000, 39_984, Duration::ZERO).unwrap();
    for second in 1..=100 {
        rate.update(
            Duration::from_secs(second),
            Feedback {
                frames: 50,
                lost: if second < 10 { 5 } else { 0 },
                rtt: Some(Duration::from_millis(30)),
            },
        );
        rate.set_maximum(24_000).unwrap();
        black_box(rate.bitrate());
    }
    rate.tick(Duration::from_secs(105));
    let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(allocations, 0);
}

#[test]
fn steady_state_processing_never_allocates() {
    let model = Model::new(&[0, 27, 200, 256]).unwrap();
    let fft = FftPlan::new(480).unwrap();
    let mdct = MdctPlan::new(960, 120).unwrap();
    let mut workspace = MdctWorkspace::new(&mdct);
    let mut transform_output = [Complex::ZERO; 480];
    let mut transform_recovered = [Complex::ZERO; 480];
    let transform_input = [Complex::ZERO; 480];
    let mdct_input = [0.0; 1_920];
    let mut coefficients = [0.0; 960];
    let mut mdct_output = [0.0; 1_920];
    let mut entropy = [0; 128];
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let mut audio_encoder = AudioEncoder::new(
            Config::default()
                .with_frame_duration(duration)
                .with_harmonic_speech(true),
        );
        let mut audio_decoder = AudioDecoder::with_frame_duration(duration);
        let mut encoded = [0; MAX_PACKET_BYTES];
        let input: [f32; FRAME_SAMPLES] = std::array::from_fn(|i| (i as f32 * 0.15).sin());
        let dense: [f32; FRAME_SAMPLES] = std::array::from_fn(|i| {
            (1..=12)
                .map(|tone| {
                    ((i as f32 + 0.5)
                        * ((tone as f32 + 0.5) * std::f32::consts::TAU * 50.0 / 48_000.0))
                        .cos()
                })
                .sum::<f32>()
                / 20.0
        });
        let voiced: [f32; FRAME_SAMPLES] = std::array::from_fn(|i| {
            (1..=20)
                .map(|h| {
                    let f0 = 200.0f64;
                    let amplitude = (-((h as f64 * f0 - 1300.0) / 600.0).powi(2)).exp() * 0.07;
                    amplitude
                        * (std::f64::consts::TAU * h as f64 * f0 * i as f64 / 48000.0
                            + h as f64 * 0.21)
                            .cos()
                })
                .sum::<f64>() as f32
        });
        let silence = [0.0; FRAME_SAMPLES];
        let mut models = 0u8;
        let mut harmonic_frames = 0usize;
        let mut gain_widths = 0u16;
        let mut scaled = [0.0; FRAME_SAMPLES];
        let mut decoded_storage = [0.0; FRAME_SAMPLES];
        let decoded = &mut decoded_storage[..duration.samples()];
        ALLOCATIONS.with(|counter| counter.set(Some(0)));
        for bitrate in [12_345, 12_800, 14_400, 16_000, 24_000] {
            audio_encoder.set_bitrate(bitrate).unwrap();
            for frame in 0..1_000 {
                let input = &match frame / 250 {
                    0 => {
                        if (100..200).contains(&frame) {
                            &voiced
                        } else {
                            &input
                        }
                    }
                    1 | 2 => &dense,
                    _ => &silence,
                }[..duration.samples()];
                let level = 0.2 + (frame % 40) as f32 * 0.02;
                for (value, &source) in scaled.iter_mut().zip(input) {
                    *value = level * source;
                }
                let input = &scaled[..duration.samples()];
                let length = audio_encoder
                    .encode(black_box(input), &mut encoded)
                    .unwrap();
                let parsed = Packet::parse(black_box(&encoded[..length])).unwrap();
                black_box(parsed.payload());
                harmonic_frames += usize::from(encoded[0] & 14 == 14);
                models |= if parsed.gain_bands() == 4 { 2 } else { 1 };
                if parsed.gain_bands() == 4 {
                    let bits = (parsed.payload().len() * 240 / duration.samples()).clamp(5, 8);
                    gain_widths |= 1 << bits;
                }
                audio_decoder.decode(&encoded[..length], decoded).unwrap();
                black_box(&*decoded);
                audio_decoder.conceal(decoded).unwrap();
                audio_encoder.encode(input, &mut encoded).unwrap();
                black_box(&*decoded);
                let mut encoder = Encoder::new(&mut entropy);
                encoder.symbol(1, model).unwrap();
                encoder.uniform(935, 1_000).unwrap();
                encoder.raw(0xa5, 8).unwrap();
                let length = encoder.finish().unwrap();
                let mut decoder = Decoder::new(&entropy[..length]);
                black_box(decoder.symbol(model));
                black_box(decoder.uniform(1_000).unwrap());
                black_box(decoder.raw(8).unwrap());
                fft.forward(black_box(&transform_input), &mut transform_output)
                    .unwrap();
                fft.inverse(black_box(&transform_output), &mut transform_recovered)
                    .unwrap();
                black_box(&transform_recovered);
                mdct.forward(black_box(&mdct_input), &mut coefficients, &mut workspace)
                    .unwrap();
                mdct.inverse(black_box(&coefficients), &mut mdct_output, &mut workspace)
                    .unwrap();
                black_box(&mdct_output);
            }
        }
        assert_eq!(
            models, 3,
            "both spectral models must reach allocation tracking"
        );
        audio_encoder.set_bitrate(12_345).unwrap();
        audio_encoder.finish(&mut encoded).unwrap();
        audio_encoder.reset();
        audio_decoder.reset();
        let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
        assert_eq!(allocations, 0);
        assert!(
            harmonic_frames > 0,
            "harmonic decoding must reach allocation tracking"
        );
        assert_eq!(
            gain_widths,
            if duration == FrameDuration::Ms10 {
                0b1_1110_0000
            } else {
                0b1_1100_0000
            }
        );
    }
}
