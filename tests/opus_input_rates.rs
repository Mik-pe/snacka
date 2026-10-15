// SPDX-License-Identifier: MIT
#![cfg(feature = "opus")]
//! Input-rate integration and optional public libopus interoperability oracles.

use snacka::opus::{
    Application, Bandwidth, Decoder, EncodeError, Encoder, EncoderConfig, FrameDuration, Mode,
    Packet, RateControl,
    multistream::{
        ChannelMapping, Error as MultistreamError, MultistreamDecoder, MultistreamEncoder,
    },
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

const RATES: [u32; 5] = [8_000, 12_000, 16_000, 24_000, 48_000];
const DURATIONS: [FrameDuration; 9] = [
    FrameDuration::Ms2_5,
    FrameDuration::Ms5,
    FrameDuration::Ms10,
    FrameDuration::Ms20,
    FrameDuration::Ms40,
    FrameDuration::Ms60,
    FrameDuration::Ms80,
    FrameDuration::Ms100,
    FrameDuration::Ms120,
];

thread_local! { static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) }; }
fn count() {
    let _ = ALLOCATIONS.try_with(|c| {
        if let Some(n) = c.get() {
            c.set(Some(n + 1));
        }
    });
}
struct Tracked;
// SAFETY: Every operation forwards its unchanged allocator contract to System;
// the thread-local allocation counter does not allocate or alter pointer ownership.
unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: Forwarding System's allocation contract unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: Forwarding System's allocation contract unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: System owns the allocation with this pointer and layout.
        unsafe { System.dealloc(ptr, layout) }
    }
    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        // SAFETY: Forwarding System's allocation contract unchanged.
        unsafe { System.realloc(ptr, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Tracked = Tracked;

fn config(
    rate: u32,
    mode: Mode,
    channels: usize,
    duration: FrameDuration,
    policy: RateControl,
) -> EncoderConfig {
    EncoderConfig::new(
        96_123 * channels as u32,
        channels,
        duration,
        if mode == Mode::Silk {
            Bandwidth::Wideband
        } else {
            Bandwidth::Fullband
        },
        Application::Voice,
    )
    .unwrap()
    .with_mode(mode)
    .unwrap()
    .with_sample_rate(rate)
    .unwrap()
    .with_rate_control(policy)
}
fn pcm(samples: usize, channels: usize, rate: u32, start: usize) -> Vec<f32> {
    (0..samples * channels)
        .map(|index| {
            let t = (start + index / channels) as f32 / rate as f32;
            0.18 * (std::f32::consts::TAU * (137.0 + (index % channels) as f32 * 61.0) * t).sin()
                + 0.035 * (std::f32::consts::TAU * 1297.0 * t).sin()
        })
        .collect()
}

#[test]
fn every_input_rate_mode_duration_channel_and_rate_policy_decodes() {
    let mut cases = 0;
    for rate in RATES {
        for mode in [Mode::Silk, Mode::Hybrid, Mode::Celt] {
            for duration in DURATIONS {
                if mode != Mode::Celt && duration.samples() < 480 {
                    continue;
                }
                for channels in [1, 2] {
                    for policy in [RateControl::Constant, RateControl::Variable] {
                        let settings = config(rate, mode, channels, duration, policy);
                        assert_eq!(
                            settings.input_samples(),
                            duration.samples() * rate as usize / 48_000
                        );
                        let mut encoder = Encoder::new(settings).unwrap();
                        let mut decoders: Vec<_> = RATES
                            .into_iter()
                            .map(|r| Decoder::with_sample_rate(channels, r).unwrap())
                            .collect();
                        #[cfg(feature = "reference-tests")]
                        let mut native: Vec<_> = RATES
                            .into_iter()
                            .map(|r| {
                                reference::Native::new(
                                    r,
                                    ChannelMapping::mono_stereo(channels as u8).unwrap(),
                                )
                            })
                            .collect();
                        let mut packet = vec![0xa5; settings.max_packet_bytes() + 1];
                        let mut decoded = vec![0.0; duration.samples() * channels];
                        #[cfg(feature = "reference-tests")]
                        let mut native_pcm = decoded.clone();
                        let mut bytes = 0;
                        assert_eq!(encoder.sample_rate(), rate);
                        assert_eq!(
                            encoder.algorithmic_delay_samples(),
                            if rate == 48_000 {
                                120
                            } else {
                                216 * rate as usize / 48_000
                            }
                        );
                        for frame in 0..3 {
                            let input = pcm(
                                settings.input_samples(),
                                channels,
                                rate,
                                frame * settings.input_samples(),
                            );
                            let length = encoder.encode(&input, &mut packet).unwrap();
                            assert_eq!(packet[length], 0xa5);
                            let parsed = Packet::parse(&packet[..length]).unwrap();
                            assert_eq!(parsed.samples(rate).unwrap(), settings.input_samples());
                            assert_eq!(parsed.toc().mode(), mode);
                            bytes += length;
                            if policy == RateControl::Constant {
                                assert_eq!(
                                    bytes as u64,
                                    u64::from(settings.bitrate())
                                        * duration.samples() as u64
                                        * (frame + 1) as u64
                                        / 384_000
                                );
                            }
                            for (index, (&out_rate, decoder)) in
                                RATES.iter().zip(&mut decoders).enumerate()
                            {
                                let expected = duration.samples() * out_rate as usize / 48_000;
                                assert_eq!(
                                    decoder.decode(&packet[..length], &mut decoded).unwrap(),
                                    expected
                                );
                                assert_eq!(decoder.final_range(), encoder.final_range());
                                assert!(
                                    decoded[..expected * channels].iter().all(|x| x.is_finite())
                                );
                                #[cfg(feature = "reference-tests")]
                                {
                                    assert_eq!(
                                        native[index].decode(
                                            &packet[..length],
                                            expected,
                                            &mut native_pcm
                                        ),
                                        encoder.final_range()
                                    );
                                    assert!(
                                        native_pcm[..expected * channels]
                                            .iter()
                                            .all(|x| x.is_finite())
                                    );
                                }
                                #[cfg(not(feature = "reference-tests"))]
                                let _ = index;
                            }
                            packet.fill(0xa5);
                        }
                        cases += 1;
                    }
                }
            }
        }
    }
    assert_eq!(cases, 460);
}

#[test]
fn mapped_rates_aliases_silence_and_mismatched_configurations() {
    let mapping = ChannelMapping::new(2, 1, &[2, 0, 1, 255, 0]).unwrap();
    for rate in RATES {
        for duration in [
            FrameDuration::Ms2_5,
            FrameDuration::Ms20,
            FrameDuration::Ms120,
        ] {
            let settings = [
                config(rate, Mode::Celt, 2, duration, RateControl::Constant),
                config(rate, Mode::Celt, 1, duration, RateControl::Variable),
            ];
            let mut encoder = MultistreamEncoder::new(mapping, &settings).unwrap();
            assert_eq!(encoder.sample_rate(), rate);
            assert_eq!(encoder.input_samples(), settings[0].input_samples());
            assert_eq!(
                encoder.algorithmic_delay_samples(),
                if rate == 48_000 {
                    120
                } else {
                    216 * rate as usize / 48_000
                }
            );
            let mut decoder = MultistreamDecoder::with_sample_rate(mapping, rate).unwrap();
            #[cfg(feature = "reference-tests")]
            let mut native = reference::Native::new(rate, mapping);
            let samples = encoder.input_samples();
            let mut packet = vec![0; encoder.max_packet_bytes()];
            let mut decoded = vec![0.0; samples * 5];
            #[cfg(feature = "reference-tests")]
            let mut native_pcm = decoded.clone();
            for frame in 0..3 {
                let length = encoder
                    .encode(&pcm(samples, 5, rate, frame * samples), &mut packet)
                    .unwrap();
                assert_eq!(
                    decoder.decode(&packet[..length], &mut decoded).unwrap(),
                    samples
                );
                assert_eq!(decoder.final_range(), encoder.final_range());
                for frame in decoded.as_chunks::<5>().0 {
                    assert_eq!(frame[1], frame[4]);
                    assert_eq!(frame[3], 0.0);
                }
                #[cfg(feature = "reference-tests")]
                {
                    assert_eq!(
                        native.decode(&packet[..length], samples, &mut native_pcm),
                        encoder.final_range()
                    );
                    assert!(native_pcm.iter().all(|x| x.is_finite()));
                }
            }
        }
    }
    let stereo = config(
        8_000,
        Mode::Celt,
        2,
        FrameDuration::Ms20,
        RateControl::Variable,
    );
    let mono = config(
        12_000,
        Mode::Celt,
        1,
        FrameDuration::Ms20,
        RateControl::Variable,
    );
    assert!(matches!(
        MultistreamEncoder::new(mapping, &[stereo, mono]),
        Err(MultistreamError::Encode(EncodeError::Argument))
    ));
    for rate in [0, 1, 7_999, 11_025, 44_100, 96_000, u32::MAX] {
        assert_eq!(
            EncoderConfig::default().with_sample_rate(rate),
            Err(EncodeError::Argument)
        );
    }
}

#[test]
fn rejected_inputs_preserve_filter_codec_and_fractional_clock_history() {
    for rate in RATES {
        let settings = config(
            rate,
            Mode::Hybrid,
            2,
            FrameDuration::Ms20,
            RateControl::Constant,
        );
        let mut encoder = Encoder::new(settings).unwrap();
        let mut untouched = Encoder::new(settings).unwrap();
        let input = pcm(settings.input_samples(), 2, rate, 0);
        let mut packet = vec![0; settings.max_packet_bytes()];
        let mut expected = packet.clone();
        encoder.encode(&input, &mut packet).unwrap();
        untouched.encode(&input, &mut expected).unwrap();
        for bad in [f32::NAN, f32::INFINITY, -1.01, 1.01] {
            let mut invalid = input.clone();
            let last = invalid.len() - 1;
            invalid[last] = bad;
            packet.fill(0xa5);
            assert_eq!(
                encoder.encode(&invalid, &mut packet),
                Err(EncodeError::Input)
            );
            assert!(packet.iter().all(|&x| x == 0xa5));
        }
        assert_eq!(
            encoder.encode(&input[..input.len() - 1], &mut packet),
            Err(EncodeError::Input)
        );
        assert_eq!(encoder.encode(&input, &mut []), Err(EncodeError::Output));
        let length = encoder.encode(&input, &mut packet).unwrap();
        assert_eq!(untouched.encode(&input, &mut expected).unwrap(), length);
        assert_eq!(packet, expected);
        assert_eq!(encoder.final_range(), untouched.final_range());
        encoder.reset();
        untouched.reset();
        assert_eq!(
            encoder.encode(&input, &mut packet).unwrap(),
            untouched.encode(&input, &mut expected).unwrap()
        );
        assert_eq!(packet, expected);
    }

    let mapping = ChannelMapping::new(2, 0, &[0, 1, 255]).unwrap();
    let settings = config(
        8_000,
        Mode::Celt,
        1,
        FrameDuration::Ms20,
        RateControl::Constant,
    );
    let mut mapped = MultistreamEncoder::new(mapping, &[settings, settings]).unwrap();
    let mut fresh = MultistreamEncoder::new(mapping, &[settings, settings]).unwrap();
    let input = pcm(settings.input_samples(), 3, 8_000, 0);
    let mut packet = vec![0xa5; mapped.max_packet_bytes()];
    let mut expected = packet.clone();
    let mut invalid = input.clone();
    let last = invalid.len() - 1;
    invalid[last] = f32::NAN;
    assert_eq!(
        mapped.encode(&invalid, &mut packet),
        Err(MultistreamError::Encode(EncodeError::Input))
    );
    assert_eq!(
        mapped.encode(&input, &mut []),
        Err(MultistreamError::Output)
    );
    assert!(packet.iter().all(|&byte| byte == 0xa5));
    let length = mapped.encode(&input, &mut packet).unwrap();
    assert_eq!(fresh.encode(&input, &mut expected).unwrap(), length);
    assert_eq!(packet, expected);
}

#[test]
fn dtx_clock_uses_packet_time_at_every_input_rate() {
    for rate in RATES {
        let settings = config(
            rate,
            Mode::Silk,
            1,
            FrameDuration::Ms20,
            RateControl::Constant,
        )
        .with_dtx(true)
        .unwrap();
        let mut encoder = Encoder::new(settings).unwrap();
        let mut packet = vec![0; settings.max_packet_bytes()];
        let input = vec![0.0; settings.input_samples()];
        for frame in 0..11 {
            encoder.encode(&input, &mut packet).unwrap();
            assert_eq!(encoder.was_dtx(), frame == 10);
        }
        let active = pcm(settings.input_samples(), 1, rate, 0);
        encoder.encode(&active, &mut packet).unwrap();
        assert!(!encoder.was_dtx());
        encoder.reset();
        encoder.encode(&input, &mut packet).unwrap();
        assert!(!encoder.was_dtx());
    }
}

#[test]
fn input_rate_changes_reset_only_interpolation_and_queued_changes_take_effect_after_packet() {
    let first = config(
        8_000,
        Mode::Celt,
        2,
        FrameDuration::Ms20,
        RateControl::Constant,
    );
    let second = first.with_sample_rate(24_000).unwrap();
    let mut encoder = Encoder::new(first).unwrap();
    let mut decoder = Decoder::new(2).unwrap();
    let mut packet = vec![0; first.max_packet_bytes()];
    let mut output = [0.0; 1920];
    encoder.prepare_reconfigure(second).unwrap();
    assert_eq!(encoder.config(), first);
    assert_eq!(
        encoder.encode(&[0.0; 960], &mut packet),
        Err(EncodeError::Input)
    );
    assert_eq!(encoder.config(), first);
    let length = encoder
        .encode(&pcm(first.input_samples(), 2, 8_000, 0), &mut packet)
        .unwrap();
    decoder.decode(&packet[..length], &mut output).unwrap();
    assert_eq!(encoder.config(), second);
    assert_eq!(decoder.final_range(), encoder.final_range());
    let length = encoder
        .encode(&pcm(second.input_samples(), 2, 24_000, 0), &mut packet)
        .unwrap();
    decoder.decode(&packet[..length], &mut output).unwrap();
    assert_eq!(decoder.final_range(), encoder.final_range());
    encoder.reconfigure(first).unwrap();
    encoder.reset();
    let mut fresh = Encoder::new(first).unwrap();
    let mut expected = packet.clone();
    let input = pcm(first.input_samples(), 2, 8_000, 0);
    assert_eq!(
        encoder.encode(&input, &mut packet).unwrap(),
        fresh.encode(&input, &mut expected).unwrap()
    );
    assert_eq!(packet, expected);
}

#[test]
fn full_scale_overshoot_inputs_encode_without_late_input_errors() {
    for rate in RATES {
        for mode in [Mode::Silk, Mode::Hybrid, Mode::Celt] {
            let settings = config(rate, mode, 2, FrameDuration::Ms20, RateControl::Constant);
            let mut encoder = Encoder::new(settings).unwrap();
            let mut decoder = Decoder::new(2).unwrap();
            let mut packet = vec![0; settings.max_packet_bytes()];
            let mut input = vec![0.0; settings.input_samples() * 2];
            let mut output = [0.0; 1920];
            for pattern in 0..4 {
                for (index, x) in input.iter_mut().enumerate() {
                    let n = index / 2;
                    *x = match pattern {
                        0 => {
                            if n.is_multiple_of(2) {
                                1.0
                            } else {
                                -1.0
                            }
                        }
                        1 => {
                            if (n / 7).is_multiple_of(2) {
                                1.0
                            } else {
                                -1.0
                            }
                        }
                        2 => {
                            if n == 0 {
                                1.0
                            } else {
                                0.0
                            }
                        }
                        _ => 1.0,
                    };
                }
                let length = encoder.encode(&input, &mut packet).unwrap();
                decoder.decode(&packet[..length], &mut output).unwrap();
                assert_eq!(decoder.final_range(), encoder.final_range());
                assert!(output.iter().all(|x| x.is_finite()));
            }
        }
    }
}

#[test]
fn processing_rate_changes_mapped_encoding_and_reset_do_not_allocate() {
    let settings = config(
        8_000,
        Mode::Celt,
        2,
        FrameDuration::Ms120,
        RateControl::Variable,
    );
    let mut encoder = Encoder::new(settings).unwrap();
    let mapping = ChannelMapping::new(2, 0, &[1, 0, 255]).unwrap();
    let configs = [config(
        12_000,
        Mode::Silk,
        1,
        FrameDuration::Ms20,
        RateControl::Constant,
    ); 2];
    let mut mapped = MultistreamEncoder::new(mapping, &configs).unwrap();
    let mut packet = vec![0; encoder.config().max_packet_bytes()];
    let mut transport = vec![0; mapped.max_packet_bytes()];
    let input = [0.05; 11520];
    let mapped_input = [0.02; 720];
    ALLOCATIONS.with(|c| c.set(Some(0)));
    for rate in RATES {
        encoder
            .reconfigure(settings.with_sample_rate(rate).unwrap())
            .unwrap();
        encoder
            .encode(&input[..encoder.config().input_samples() * 2], &mut packet)
            .unwrap();
        encoder.reset();
    }
    mapped.encode(&mapped_input, &mut transport).unwrap();
    mapped.reset();
    let allocations = ALLOCATIONS.with(|c| c.replace(None).unwrap());
    assert_eq!(allocations, 0);
}

#[cfg(feature = "reference-tests")]
mod reference {
    use super::*;
    use std::{ffi::c_void, ptr::NonNull};
    #[link(name = "opus")]
    unsafe extern "C" {
        fn opus_multistream_decoder_create(
            rate: i32,
            channels: i32,
            streams: i32,
            coupled: i32,
            mapping: *const u8,
            error: *mut i32,
        ) -> *mut c_void;
        fn opus_multistream_decoder_destroy(state: *mut c_void);
        fn opus_multistream_decoder_ctl(state: *mut c_void, request: i32, ...) -> i32;
        fn opus_multistream_decode_float(
            state: *mut c_void,
            data: *const u8,
            length: i32,
            output: *mut f32,
            samples: i32,
            fec: i32,
        ) -> i32;
    }
    pub(super) struct Native(NonNull<c_void>);
    impl Native {
        pub(super) fn new(rate: u32, mapping: ChannelMapping) -> Self {
            let mut error = 0;
            // SAFETY: Supported rate and validated channel counts/mapping; create
            // copies the mapping before its borrowed storage goes out of scope.
            let ptr = unsafe {
                opus_multistream_decoder_create(
                    rate as i32,
                    mapping.output_channels() as i32,
                    i32::from(mapping.streams()),
                    i32::from(mapping.coupled_streams()),
                    mapping.channels().as_ptr(),
                    &mut error,
                )
            };
            assert_eq!(error, 0);
            Self(NonNull::new(ptr).unwrap())
        }
        pub(super) fn decode(&mut self, packet: &[u8], samples: usize, output: &mut [f32]) -> u32 {
            // SAFETY: Unique live decoder, complete readable packet, and output
            // capacity covering the supplied sample count and configured channels.
            let count = unsafe {
                opus_multistream_decode_float(
                    self.0.as_ptr(),
                    packet.as_ptr(),
                    packet.len() as i32,
                    output.as_mut_ptr(),
                    samples as i32,
                    0,
                )
            };
            assert_eq!(count, samples as i32);
            let mut range = 0u32;
            // SAFETY: OPUS_GET_FINAL_RANGE takes a writable opus_uint32 pointer.
            let status = unsafe { opus_multistream_decoder_ctl(self.0.as_ptr(), 4031, &mut range) };
            assert_eq!(status, 0);
            range
        }
    }
    impl Drop for Native {
        fn drop(&mut self) {
            // SAFETY: This unique decoder was returned by the matching create API.
            unsafe { opus_multistream_decoder_destroy(self.0.as_ptr()) };
        }
    }
}
