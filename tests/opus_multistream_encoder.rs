// SPDX-License-Identifier: MIT
#![cfg(feature = "opus")]
//! Owned mapped encoding; the optional reference test alone links libopus.

use snacka::opus::{
    Application, Bandwidth, EncodeError, Encoder, EncoderConfig, FrameDuration, Mode, Packet,
    RateControl,
    multistream::{
        ChannelMapping, Error, MultistreamDecoder, MultistreamEncoder, MultistreamPacket,
    },
    repacketize::Repacketizer,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

fn track_allocation() {
    let _ = ALLOCATIONS.try_with(|counter| {
        if let Some(previous) = counter.get() {
            counter.set(Some(previous + 1));
        }
    });
}

struct Tracked;
// SAFETY: All allocation operations delegate their original arguments to System.
// The thread-local counter does not allocate or alter allocator ownership.
unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        track_allocation();
        // SAFETY: Forwarding the allocator contract unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        track_allocation();
        // SAFETY: Forwarding the allocator contract unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: System owns the allocation described by this pointer and layout.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        track_allocation();
        // SAFETY: Forwarding the allocator contract unchanged.
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Tracked = Tracked;

fn config(channels: usize, mode: Mode, duration: FrameDuration, bitrate: u32) -> EncoderConfig {
    EncoderConfig::new(
        bitrate,
        channels,
        duration,
        if mode == Mode::Silk {
            Bandwidth::Wideband
        } else {
            Bandwidth::Fullband
        },
        Application::Audio,
    )
    .unwrap()
    .with_mode(mode)
    .unwrap()
}

fn signal(samples: usize, channels: usize, offset: usize) -> Vec<f32> {
    (0..samples * channels)
        .map(|index| {
            let sample = index / channels + offset;
            let channel = index % channels;
            (sample as f32 * (0.031 + channel as f32 * 0.009)).sin() * 0.12
        })
        .collect()
}

#[test]
fn configuration_rejects_missing_streams_channels_durations_and_coupled_sources() {
    let mapping = ChannelMapping::new(2, 1, &[0, 1, 2]).unwrap();
    let stereo = config(2, Mode::Celt, FrameDuration::Ms20, 64_000);
    let mono = config(1, Mode::Silk, FrameDuration::Ms20, 24_000);
    assert!(matches!(
        MultistreamEncoder::new(mapping, &[]),
        Err(Error::Streams)
    ));
    assert!(matches!(
        MultistreamEncoder::new(mapping, &[stereo]),
        Err(Error::Streams)
    ));
    assert!(matches!(
        MultistreamEncoder::new(mapping, &[mono, mono]),
        Err(Error::Mapping)
    ));
    let short = config(1, Mode::Celt, FrameDuration::Ms10, 24_000);
    assert!(matches!(
        MultistreamEncoder::new(mapping, &[stereo, short]),
        Err(Error::Duration)
    ));
    let missing = ChannelMapping::new(2, 1, &[0, 2, 255]).unwrap();
    assert!(matches!(
        MultistreamEncoder::new(missing, &[stereo, mono]),
        Err(Error::Mapping)
    ));
}

#[test]
fn mapping_selects_first_alias_ignores_silent_slots_and_matches_independent_encoders() {
    let mapping = ChannelMapping::new(2, 1, &[2, 0, 1, 255, 0]).unwrap();
    let configs = [
        config(2, Mode::Hybrid, FrameDuration::Ms20, 64_000),
        config(1, Mode::Silk, FrameDuration::Ms20, 24_000),
    ];
    let mut encoder = MultistreamEncoder::new(mapping, &configs).unwrap();
    let mut independent: Vec<_> = configs
        .into_iter()
        .map(|c| Encoder::new(c).unwrap())
        .collect();
    let mut decoder = MultistreamDecoder::new(mapping).unwrap();
    let mut output = vec![0xa5; encoder.max_packet_bytes() + 19];
    for frame in 0..3 {
        let input = signal(960, 5, frame * 960);
        let length = encoder.encode(&input, &mut output).unwrap();
        assert!(output[length..].iter().all(|&byte| byte == 0xa5));
        let transport = MultistreamPacket::parse(&output[..length], 2).unwrap();
        for (stream, actual) in transport.packets().enumerate() {
            let sources: &[usize] = if stream == 0 { &[1, 2] } else { &[0] };
            let pcm: Vec<_> = input
                .as_chunks::<5>()
                .0
                .iter()
                .flat_map(|frame| sources.iter().map(move |&source| frame[source]))
                .collect();
            let mut expected = vec![0; configs[stream].max_packet_bytes()];
            let bytes = independent[stream].encode(&pcm, &mut expected).unwrap();
            let expected = Packet::parse(&expected[..bytes]).unwrap();
            assert_eq!(actual.toc().config(), expected.toc().config());
            assert_eq!(
                actual.frames().collect::<Vec<_>>(),
                expected.frames().collect::<Vec<_>>()
            );
        }
        let expected_range = independent
            .iter()
            .fold(0, |range, enc| range ^ enc.final_range());
        assert_eq!(encoder.final_range(), expected_range);
        let mut pcm = vec![0.0; input.len()];
        assert_eq!(decoder.decode(&output[..length], &mut pcm).unwrap(), 960);
        assert_eq!(encoder.final_range(), decoder.final_range());
        for frame in pcm.as_chunks::<5>().0 {
            assert_eq!(frame[3], 0.0);
            assert_eq!(frame[1], frame[4]);
        }
        output.fill(0xa5);
    }
    assert_eq!(encoder.mapping(), &mapping);
    assert_eq!(encoder.channels(), 5);
    assert!(encoder.stream_encoder(2).is_none());
}

#[test]
fn unused_mono_stream_encodes_silence() {
    let mapping = ChannelMapping::new(2, 0, &[0, 255]).unwrap();
    let settings = config(1, Mode::Celt, FrameDuration::Ms20, 32_000);
    let mut encoder = MultistreamEncoder::new(mapping, &[settings, settings]).unwrap();
    let mut mono = Encoder::new(settings).unwrap();
    let mut output = vec![0; encoder.max_packet_bytes()];
    let length = encoder.encode(&signal(960, 2, 0), &mut output).unwrap();
    let transport = MultistreamPacket::parse(&output[..length], 2).unwrap();
    let second = transport.packets().nth(1).unwrap();
    let mut expected = [0; 80];
    let length = mono.encode(&[0.0; 960], &mut expected).unwrap();
    let expected = Packet::parse(&expected[..length]).unwrap();
    assert_eq!(
        second.frames().collect::<Vec<_>>(),
        expected.frames().collect::<Vec<_>>()
    );
}

#[test]
fn invalid_pcm_and_capacity_leave_every_stream_history_and_output_unchanged() {
    let mapping = ChannelMapping::new(2, 0, &[0, 1]).unwrap();
    let settings = config(1, Mode::Celt, FrameDuration::Ms20, 32_003);
    let mut encoder = MultistreamEncoder::new(mapping, &[settings, settings]).unwrap();
    let mut pristine = MultistreamEncoder::new(mapping, &[settings, settings]).unwrap();
    let mut input = signal(960, 2, 0);
    let mut output = vec![0xa5; encoder.max_packet_bytes()];
    assert_eq!(
        encoder.encode(&input[..input.len() - 1], &mut output),
        Err(Error::Encode(EncodeError::Input))
    );
    input[1731] = f32::NAN;
    assert_eq!(
        encoder.encode(&input, &mut output),
        Err(Error::Encode(EncodeError::Input))
    );
    input[1731] = 1.01;
    assert_eq!(
        encoder.encode(&input, &mut output),
        Err(Error::Encode(EncodeError::Input))
    );
    input[1731] = 0.1;
    let short = output.len() - 1;
    assert_eq!(
        encoder.encode(&input, &mut output[..short]),
        Err(Error::Output)
    );
    assert!(output.iter().all(|&byte| byte == 0xa5));
    assert_eq!(encoder.final_range(), 0);
    let mut expected = output.clone();
    let length = encoder.encode(&input, &mut output).unwrap();
    assert_eq!(pristine.encode(&input, &mut expected).unwrap(), length);
    assert_eq!(output, expected);
    encoder.reset();
    pristine.reset();
    assert_eq!(encoder.final_range(), 0);
    assert!(!encoder.was_dtx());
    assert_eq!(encoder.encode(&input, &mut output).unwrap(), length);
    assert_eq!(pristine.encode(&input, &mut expected).unwrap(), length);
    assert_eq!(output, expected);
}

#[test]
fn constant_rate_preserves_fractional_aggregate_budgets_and_self_delimiter_costs() {
    let mapping = ChannelMapping::new(2, 0, &[0, 1]).unwrap();
    let configs = [
        config(1, Mode::Silk, FrameDuration::Ms20, 32_003).with_rate_control(RateControl::Constant),
        config(1, Mode::Celt, FrameDuration::Ms20, 48_007).with_rate_control(RateControl::Constant),
    ];
    let mut encoder = MultistreamEncoder::new(mapping, &configs).unwrap();
    let mut output = vec![0; encoder.max_packet_bytes()];
    for frame in 0..24 {
        let length = encoder
            .encode(&signal(960, 2, frame * 960), &mut output)
            .unwrap();
        let transport = MultistreamPacket::parse(&output[..length], 2).unwrap();
        let first = transport.packets().next().unwrap();
        let mut builder = Repacketizer::new();
        builder.push_packet(&first).unwrap();
        let delimiter = builder
            .self_delimited_size(0..builder.frame_count())
            .unwrap()
            - builder.output_size(0..builder.frame_count()).unwrap();
        let expected = configs
            .iter()
            .map(|config| {
                let numerator = u64::from(config.bitrate()) * 960;
                ((frame + 1) as u64 * numerator / 384_000 - frame as u64 * numerator / 384_000)
                    as usize
            })
            .sum::<usize>()
            + delimiter;
        assert_eq!(length, expected);
    }
}

#[test]
fn processing_reset_and_status_do_not_allocate_and_dtx_requires_every_stream() {
    let mapping = ChannelMapping::new(2, 0, &[0, 1, 255]).unwrap();
    let settings = config(1, Mode::Silk, FrameDuration::Ms20, 24_000)
        .with_dtx(true)
        .unwrap();
    let mut encoder = MultistreamEncoder::new(mapping, &[settings, settings]).unwrap();
    let input = [0.0; 960 * 3];
    let mut output = vec![0; encoder.max_packet_bytes()];
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    for _ in 0..24 {
        encoder.encode(&input, &mut output).unwrap();
        assert_eq!(
            encoder.was_dtx(),
            encoder.stream_encoder(0).unwrap().was_dtx()
                && encoder.stream_encoder(1).unwrap().was_dtx()
        );
        let _ = encoder.final_range();
    }
    let suppressed = encoder.was_dtx();
    encoder.reset();
    encoder.encode(&input, &mut output).unwrap();
    let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(allocations, 0);
    assert!(suppressed);
    assert!(!encoder.was_dtx());

    for frame in 0..18 {
        let mut active = signal(960, 3, frame * 960);
        for sample in active.as_chunks_mut::<3>().0 {
            sample[0] = 0.0;
        }
        encoder.encode(&active, &mut output).unwrap();
    }
    assert!(encoder.stream_encoder(0).unwrap().was_dtx());
    assert!(!encoder.stream_encoder(1).unwrap().was_dtx());
    assert!(!encoder.was_dtx());
}

#[test]
fn active_constant_rate_stream_moves_padding_to_empty_final_dtx_stream() {
    let mapping = ChannelMapping::new(2, 0, &[0, 1]).unwrap();
    let duration = FrameDuration::Ms120;
    let samples = duration.samples();
    let configs = [
        config(1, Mode::Silk, duration, 128_003).with_rate_control(RateControl::Constant),
        config(1, Mode::Silk, duration, 24_000)
            .with_dtx(true)
            .unwrap(),
    ];
    let mut encoder = MultistreamEncoder::new(mapping, &configs).unwrap();
    let mut decoder = MultistreamDecoder::new(mapping).unwrap();
    let mut packet = vec![0; encoder.max_packet_bytes()];
    let mut decoded = vec![0.0; samples * 2];
    let mut suppressed_packets = 0;
    #[cfg(feature = "reference-tests")]
    let mut native = reference::Native::new(mapping);
    #[cfg(feature = "reference-tests")]
    let mut native_pcm = decoded.clone();

    for frame in 0..12 {
        let mut input = signal(samples, 2, frame * samples);
        for channels in input.as_chunks_mut::<2>().0 {
            channels[1] = 0.0;
        }
        let length = encoder.encode(&input, &mut packet).unwrap();
        let transport = MultistreamPacket::parse(&packet[..length], 2).unwrap();
        assert_eq!(transport.samples(48_000).unwrap(), samples);
        assert!(!encoder.was_dtx());
        if encoder.stream_encoder(1).unwrap().was_dtx() {
            suppressed_packets += 1;
            let mut streams = transport.packets();
            let active = streams.next().unwrap();
            let inactive = streams.next().unwrap();
            assert_eq!(active.padding_len(), 0);
            assert!(inactive.frames().all(|frame| frame.is_empty()));
            assert!(inactive.padding_len() > 0);
            assert_eq!(inactive.samples(48_000).unwrap(), samples);
        }
        assert_eq!(
            decoder.decode(&packet[..length], &mut decoded).unwrap(),
            samples
        );
        assert_eq!(decoder.final_range(), encoder.final_range());
        assert!(decoded.iter().all(|sample| sample.is_finite()));
        #[cfg(feature = "reference-tests")]
        {
            assert_eq!(
                native.decode(&packet[..length], samples, &mut native_pcm),
                encoder.final_range()
            );
            assert!(native_pcm.iter().all(|sample| sample.is_finite()));
        }
    }
    assert!(suppressed_packets >= 2);
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
            packet: *const u8,
            length: i32,
            output: *mut f32,
            samples: i32,
            fec: i32,
        ) -> i32;
    }

    pub(super) struct Native(NonNull<c_void>);
    impl Native {
        pub(super) fn new(mapping: ChannelMapping) -> Self {
            let mut error = 0;
            // SAFETY: The mapping is validated, all documented counts/rates are
            // supported, and the error and mapping storage live through creation.
            let pointer = unsafe {
                opus_multistream_decoder_create(
                    48_000,
                    mapping.output_channels() as i32,
                    i32::from(mapping.streams()),
                    i32::from(mapping.coupled_streams()),
                    mapping.channels().as_ptr(),
                    &mut error,
                )
            };
            assert_eq!(error, 0);
            Self(NonNull::new(pointer).unwrap())
        }
        pub(super) fn decode(&mut self, packet: &[u8], samples: usize, output: &mut [f32]) -> u32 {
            // SAFETY: The owned decoder has matching channels, and input/output
            // buffers hold the complete advertised packet and PCM interval.
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
            // SAFETY: OPUS_GET_FINAL_RANGE takes a live writable opus_uint32 pointer.
            let status = unsafe { opus_multistream_decoder_ctl(self.0.as_ptr(), 4031, &mut range) };
            assert_eq!(status, 0);
            range
        }
    }
    impl Drop for Native {
        fn drop(&mut self) {
            // SAFETY: This unique state was returned by the matching create API.
            unsafe { opus_multistream_decoder_destroy(self.0.as_ptr()) };
        }
    }

    #[test]
    fn owned_surround_and_aliased_packets_decode_in_libopus_with_exact_ranges() {
        for mapping in [
            ChannelMapping::mono_stereo(1).unwrap(),
            ChannelMapping::mono_stereo(2).unwrap(),
            ChannelMapping::new(4, 2, &[0, 4, 1, 2, 3, 5]).unwrap(),
            ChannelMapping::new(2, 1, &[2, 0, 1, 255, 0]).unwrap(),
        ] {
            for duration in [
                FrameDuration::Ms10,
                FrameDuration::Ms20,
                FrameDuration::Ms40,
                FrameDuration::Ms60,
                FrameDuration::Ms80,
                FrameDuration::Ms100,
                FrameDuration::Ms120,
            ] {
                for mode in [Mode::Silk, Mode::Hybrid, Mode::Celt] {
                    let configs: Vec<_> = (0..mapping.streams())
                        .map(|stream| {
                            let channels = if stream < mapping.coupled_streams() {
                                2
                            } else {
                                1
                            };
                            config(channels, mode, duration, 48_000 * channels as u32)
                                .with_rate_control(RateControl::Constant)
                        })
                        .collect();
                    let mut encoder = MultistreamEncoder::new(mapping, &configs).unwrap();
                    let mut native = Native::new(mapping);
                    let mut owned = MultistreamDecoder::new(mapping).unwrap();
                    let samples = duration.samples();
                    let mut packet = vec![0; encoder.max_packet_bytes()];
                    let mut native_pcm = vec![0.0; samples * mapping.output_channels()];
                    let mut owned_pcm = native_pcm.clone();
                    for frame in 0..3 {
                        let input = signal(samples, mapping.output_channels(), frame * samples);
                        let length = encoder.encode(&input, &mut packet).unwrap();
                        assert_eq!(
                            native.decode(&packet[..length], samples, &mut native_pcm),
                            encoder.final_range()
                        );
                        assert_eq!(
                            owned.decode(&packet[..length], &mut owned_pcm).unwrap(),
                            samples
                        );
                        assert_eq!(owned.final_range(), encoder.final_range());
                        assert!(native_pcm.iter().chain(&owned_pcm).all(|x| x.is_finite()));
                        if mode == Mode::Celt {
                            for (&a, &b) in owned_pcm.iter().zip(&native_pcm) {
                                assert!((a - b).abs() < 1e-5, "CELT {a} != {b}");
                            }
                        }
                    }
                }
            }
        }
    }
}
