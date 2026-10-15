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

fn stream_state(encoder: &Encoder) -> (EncoderConfig, u32, bool, bool, bool) {
    (
        encoder.config(),
        encoder.final_range(),
        encoder.has_fec(),
        encoder.was_dtx(),
        encoder.has_transition_redundancy(),
    )
}

fn adapted(
    channels: usize,
    duration: FrameDuration,
    sample_rate: u32,
    stage: usize,
) -> EncoderConfig {
    let (bitrate, bandwidth, application, rate, fec) = match stage {
        0 => (
            128_003,
            Bandwidth::Fullband,
            Application::Audio,
            RateControl::Constant,
            false,
        ),
        1 => (
            48_003,
            Bandwidth::Wideband,
            Application::Voice,
            RateControl::Constant,
            true,
        ),
        2 => (
            64_007,
            Bandwidth::Fullband,
            Application::Voice,
            RateControl::Variable,
            true,
        ),
        _ => (
            96_011,
            Bandwidth::Fullband,
            Application::Audio,
            RateControl::Variable,
            false,
        ),
    };
    EncoderConfig::new(
        bitrate * channels as u32,
        channels,
        duration,
        bandwidth,
        application,
    )
    .unwrap()
    .with_sample_rate(sample_rate)
    .unwrap()
    .with_fec(fec)
    .unwrap()
    .with_rate_control(rate)
}

fn compare_adapted_packet(
    encoder: &mut MultistreamEncoder,
    independent: &mut [Encoder],
    input: &[f32],
    output: &mut [u8],
) {
    output.fill(0xa5);
    let length = encoder.encode(input, output).unwrap();
    let mut expected = Vec::new();
    for (stream, owner) in independent.iter_mut().enumerate() {
        let pcm: Vec<_> = input
            .as_chunks::<5>()
            .0
            .iter()
            .flat_map(|frame| match stream {
                0 => vec![frame[1], frame[2]],
                1 => vec![frame[0]],
                _ => vec![0.0],
            })
            .collect();
        let mut packet = vec![0; owner.config().max_packet_bytes()];
        let count = owner.encode(&pcm, &mut packet).unwrap();
        packet.truncate(count);
        expected.push(packet);
        assert_eq!(
            stream_state(encoder.stream_encoder(stream).unwrap()),
            stream_state(owner)
        );
    }
    let references: Vec<_> = expected.iter().map(Vec::as_slice).collect();
    let mut packet = vec![0; output.len()];
    let count = MultistreamPacket::write(&references, &mut packet).unwrap();
    let mut target = expected.iter().map(Vec::len).sum::<usize>();
    for bytes in &expected[..expected.len() - 1] {
        let mut builder = Repacketizer::new();
        builder.push(bytes).unwrap();
        let frames = 0..builder.frame_count();
        target += builder.self_delimited_size(frames.clone()).unwrap()
            - builder.output_size(frames).unwrap();
    }
    assert_eq!(length, target);
    let actual = MultistreamPacket::parse(&output[..length], 3).unwrap();
    for (actual, expected) in actual.packets().zip(&expected) {
        let expected = Packet::parse(expected).unwrap();
        assert_eq!(actual.toc().byte() & 0xfc, expected.toc().byte() & 0xfc);
        assert_eq!(
            actual.frames().collect::<Vec<_>>(),
            expected.frames().collect::<Vec<_>>()
        );
    }
    #[cfg(feature = "reference-tests")]
    {
        // The ordinary packet composer deliberately compacts padding. The
        // encoder's documented CBR aggregate retains the emitted byte budgets;
        // public native padding supplies an independent full-header oracle.
        reference::pad(&mut packet, count, target, 3);
        assert_eq!(&output[..length], &packet[..target]);
    }
    #[cfg(not(feature = "reference-tests"))]
    assert!(count <= target);
    assert!(output[length..].iter().all(|&byte| byte == 0xa5));
    assert_eq!(
        encoder.final_range(),
        independent
            .iter()
            .fold(0, |range, owner| range ^ owner.final_range())
    );
}

#[test]
fn immediate_reconfiguration_matches_ordinary_histories_across_modes_and_rates() {
    let mapping = ChannelMapping::new(3, 1, &[2, 0, 1, 255, 0]).unwrap();
    for sample_rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
        for duration in [FrameDuration::Ms20, FrameDuration::Ms120] {
            let initial = [2, 1, 1].map(|channels| adapted(channels, duration, sample_rate, 0));
            let mut encoder = MultistreamEncoder::new(mapping, &initial).unwrap();
            let mut independent: Vec<_> = initial
                .into_iter()
                .map(|config| Encoder::new(config).unwrap())
                .collect();
            let mut output = vec![0xa5; encoder.max_packet_bytes() + 17];
            let mut sequence = 0;
            for stage in [0, 1, 2, 3, 1, 0] {
                let settings =
                    [2, 1, 1].map(|channels| adapted(channels, duration, sample_rate, stage));
                let previous: Vec<_> = independent.iter().map(stream_state).collect();
                encoder.reconfigure(&settings).unwrap();
                for ((owner, &setting), old) in independent.iter_mut().zip(&settings).zip(previous)
                {
                    owner.reconfigure(setting).unwrap();
                    assert_eq!(
                        (
                            owner.final_range(),
                            owner.has_fec(),
                            owner.was_dtx(),
                            owner.has_transition_redundancy()
                        ),
                        (old.1, old.2, old.3, old.4)
                    );
                }
                assert_eq!(
                    encoder.max_packet_bytes(),
                    settings.iter().map(|c| c.max_packet_bytes()).sum::<usize>() + 4
                );
                assert_eq!(encoder.mapping(), &mapping);
                assert_eq!(encoder.sample_rate(), sample_rate);
                for _ in 0..3 {
                    let input = signal(
                        initial[0].input_samples(),
                        5,
                        sequence * initial[0].input_samples(),
                    );
                    sequence += 1;
                    compare_adapted_packet(&mut encoder, &mut independent, &input, &mut output);
                }
            }
        }
    }
}

#[test]
fn list_refusals_are_atomic_and_original_per_stream_ceilings_allow_regrowth() {
    let mapping = ChannelMapping::new(3, 1, &[2, 0, 1, 255, 0]).unwrap();
    for rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
        let initial = [2, 1, 1].map(|channels| adapted(channels, FrameDuration::Ms20, rate, 0));
        let mut encoder = MultistreamEncoder::new(mapping, &initial).unwrap();
        let mut control = MultistreamEncoder::new(mapping, &initial).unwrap();
        let mut output = vec![0xa5; encoder.max_packet_bytes() + 7];
        let mut expected = output.clone();
        let input = signal(initial[0].input_samples(), 5, 0);
        for settings in [
            [2, 1, 1].map(|channels| adapted(channels, FrameDuration::Ms20, rate, 1)),
            initial,
        ] {
            encoder.reconfigure(&settings).unwrap();
            control.reconfigure(&settings).unwrap();
            let count = encoder.encode(&input, &mut output).unwrap();
            assert_eq!(control.encode(&input, &mut expected).unwrap(), count);
            assert_eq!(output, expected);
        }
        // A valid first target changes application, mode and FEC. A failure
        // in the last child must not partially apply that policy/cache change.
        let mut target = [2, 1, 1].map(|channels| adapted(channels, FrameDuration::Ms20, rate, 1));
        let mut cases = Vec::new();
        cases.push((Vec::new(), Error::Streams));
        target[2] = adapted(2, FrameDuration::Ms20, rate, 1);
        cases.push((target.to_vec(), Error::Mapping));
        target[2] = adapted(1, FrameDuration::Ms120, rate, 0);
        cases.push((target.to_vec(), Error::Duration));
        target[2] = adapted(
            1,
            FrameDuration::Ms20,
            if rate == 48_000 { 24_000 } else { 48_000 },
            1,
        );
        cases.push((target.to_vec(), Error::Encode(EncodeError::Argument)));
        target[2] = config(1, Mode::Celt, FrameDuration::Ms20, 400_000)
            .with_sample_rate(rate)
            .unwrap();
        // The total fits original aggregate storage; this child still cannot
        // borrow capacity from its neighbours' packet buffers.
        target[0] = config(2, Mode::Celt, FrameDuration::Ms20, 6000)
            .with_sample_rate(rate)
            .unwrap();
        target[1] = config(1, Mode::Celt, FrameDuration::Ms20, 6000)
            .with_sample_rate(rate)
            .unwrap();
        assert!(
            target.iter().map(|c| c.max_packet_bytes()).sum::<usize>()
                < initial.iter().map(|c| c.max_packet_bytes()).sum::<usize>()
        );
        cases.push((target.to_vec(), Error::Output));
        target[0] = config(2, Mode::Celt, FrameDuration::Ms20, 510_000)
            .with_sample_rate(rate)
            .unwrap();
        target[2] = adapted(2, FrameDuration::Ms20, rate, 1);
        cases.push((target.to_vec(), Error::Mapping)); // Geometry before capacity.
        for (request, error) in cases {
            let before: Vec<_> = (0..3)
                .map(|i| stream_state(encoder.stream_encoder(i).unwrap()))
                .collect();
            let capacity = encoder.max_packet_bytes();
            assert_eq!(encoder.reconfigure(&request), Err(error));
            assert_eq!(encoder.max_packet_bytes(), capacity);
            for (i, old) in before.into_iter().enumerate() {
                assert_eq!(stream_state(encoder.stream_encoder(i).unwrap()), old);
            }
            output.fill(0xa5);
            expected.fill(0xa5);
            assert_eq!(
                encoder.encode(&input, &mut output),
                control.encode(&input, &mut expected)
            );
            assert_eq!(
                output, expected,
                "rejected list changed prediction or cached history"
            );
        }
        let length = encoder.max_packet_bytes() - 1;
        assert_eq!(
            encoder.encode(&input, &mut output[..length]),
            Err(Error::Output)
        );
        output.fill(0xa5);
        expected.fill(0xa5);
        assert_eq!(
            encoder.encode(&input, &mut output),
            control.encode(&input, &mut expected)
        );
        assert_eq!(output, expected);
        encoder.reset();
        for (stream, setting) in initial.into_iter().enumerate() {
            assert_eq!(encoder.stream_encoder(stream).unwrap().config(), setting);
        }
    }
}

#[test]
fn enabling_fec_recovers_a_real_tail_and_disabling_preserves_previous_status_without_allocation() {
    use snacka::opus::Recovery;
    let mapping = ChannelMapping::new(2, 0, &[0, 1]).unwrap();
    let off = config(1, Mode::Silk, FrameDuration::Ms20, 64_000);
    let on = off.with_fec(true).unwrap();
    let mut encoder = MultistreamEncoder::new(mapping, &[off, off]).unwrap();
    let mut decoder = MultistreamDecoder::new(mapping).unwrap();
    let inputs: [Vec<f32>; 6] = std::array::from_fn(|index| signal(960, 2, index * 960));
    let mut packet = vec![0xa5; encoder.max_packet_bytes()];
    let mut output = vec![0.0; 1920 * 2];
    #[cfg(feature = "reference-tests")]
    let (mut native, mut native_plc) = (
        reference::Native::new(mapping),
        reference::Native::new(mapping),
    );
    #[cfg(feature = "reference-tests")]
    let (mut reference, mut concealed) = (output.clone(), output.clone());
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    for (index, input) in inputs.iter().enumerate().take(2) {
        if index == 1 {
            encoder.reconfigure(&[on, on]).unwrap();
        }
        let length = encoder.encode(input, &mut packet).unwrap();
        decoder.decode(&packet[..length], &mut output).unwrap();
        #[cfg(feature = "reference-tests")]
        {
            native.decode(&packet[..length], 960, &mut reference);
            native_plc.decode(&packet[..length], 960, &mut concealed);
        }
    }
    for input in &inputs[2..4] {
        encoder.encode(input, &mut packet).unwrap();
    }
    let length = encoder.encode(&inputs[4], &mut packet).unwrap();
    assert!((0..2).all(|stream| encoder.stream_encoder(stream).unwrap().has_fec()));
    assert_eq!(
        decoder
            .recover(&packet[..length], 1920, &mut output)
            .unwrap(),
        Recovery::Fec
    );
    assert_eq!(decoder.last_packet_samples(), 1920);
    #[cfg(feature = "reference-tests")]
    {
        native.recover(&packet[..length], 1920, &mut reference);
        native_plc.conceal(1920, &mut concealed);
        assert!(
            reference[1920..]
                .iter()
                .zip(&concealed[1920..])
                .any(|(a, b)| a.to_bits() != b.to_bits())
        );
        assert!(reference.iter().all(|sample| sample.is_finite()));
    }
    assert!(output[..1920].iter().any(|&sample| sample != 0.0));
    assert!(output[1920..].iter().any(|&sample| sample != 0.0));
    decoder.decode(&packet[..length], &mut output).unwrap();
    assert_eq!(decoder.final_range(), encoder.final_range());
    #[cfg(feature = "reference-tests")]
    assert_eq!(
        native.decode(&packet[..length], 960, &mut reference),
        encoder.final_range()
    );
    encoder.reconfigure(&[off, off]).unwrap();
    assert!((0..2).all(|stream| encoder.stream_encoder(stream).unwrap().has_fec()));
    encoder.encode(&inputs[5], &mut packet).unwrap();
    assert!((0..2).all(|stream| !encoder.stream_encoder(stream).unwrap().has_fec()));
    encoder.reset();
    let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(allocations, 0);
    assert!((0..2).all(|stream| encoder.stream_encoder(stream).unwrap().config() == off));
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
    let smaller = config(1, Mode::Silk, FrameDuration::Ms20, 16_000)
        .with_dtx(true)
        .unwrap();
    let off = settings.with_dtx(false).unwrap();
    let oversized = config(1, Mode::Celt, FrameDuration::Ms20, 128_000);
    let mut encoder = MultistreamEncoder::new(mapping, &[settings, settings]).unwrap();
    let input = [0.0; 960 * 3];
    let mut output = vec![0; encoder.max_packet_bytes()];
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    encoder.reconfigure(&[smaller, smaller]).unwrap();
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
    encoder.reconfigure(&[settings, settings]).unwrap();
    assert_eq!(encoder.was_dtx(), suppressed);
    encoder.reconfigure(&[off, off]).unwrap();
    assert_eq!(encoder.was_dtx(), suppressed);
    assert_eq!(encoder.reconfigure(&[off, oversized]), Err(Error::Output));
    encoder.encode(&input, &mut output).unwrap();
    assert!(!encoder.was_dtx());
    encoder.reconfigure(&[settings, settings]).unwrap();
    encoder.reset();
    assert_eq!(encoder.stream_encoder(0).unwrap().config(), settings);
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
        fn opus_multistream_packet_pad(
            data: *mut u8,
            length: i32,
            target: i32,
            streams: i32,
        ) -> i32;
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

    pub(super) fn pad(packet: &mut [u8], length: usize, target: usize, streams: i32) {
        assert!(length <= target && target <= packet.len());
        // SAFETY: Public packet padding receives valid compact transport bytes
        // and a complete physical buffer for the requested target length.
        let code = unsafe {
            opus_multistream_packet_pad(packet.as_mut_ptr(), length as i32, target as i32, streams)
        };
        assert_eq!(code, 0);
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
            self.process(Some(packet), samples, false, output)
        }
        pub(super) fn recover(&mut self, packet: &[u8], samples: usize, output: &mut [f32]) -> u32 {
            self.process(Some(packet), samples, true, output)
        }
        pub(super) fn conceal(&mut self, samples: usize, output: &mut [f32]) -> u32 {
            self.process(None, samples, false, output)
        }
        fn process(
            &mut self,
            packet: Option<&[u8]>,
            samples: usize,
            fec: bool,
            output: &mut [f32],
        ) -> u32 {
            let (pointer, length) = packet.map_or((std::ptr::null(), 0), |packet| {
                (packet.as_ptr(), packet.len() as i32)
            });
            // SAFETY: The owned decoder has matching channels, and input/output
            // buffers hold the complete advertised packet and PCM interval;
            // null/zero data requests the public PLC operation.
            let count = unsafe {
                opus_multistream_decode_float(
                    self.0.as_ptr(),
                    pointer,
                    length,
                    output.as_mut_ptr(),
                    samples as i32,
                    i32::from(fec),
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
