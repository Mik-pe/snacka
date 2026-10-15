// SPDX-License-Identifier: MIT
#![cfg(feature = "opus")]
//! Actual family-3 projection encoding, matrix math and native wire interoperability.

use snacka::opus::{
    Application, Bandwidth, EncodeError, Encoder, EncoderConfig, FrameDuration, Mode, Packet,
    RateControl,
    multistream::{Error, MultistreamDecoder, MultistreamPacket},
    projection::{ProjectionDecoder, ProjectionEncoder, ProjectionMapping},
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}

struct Tracked;
fn allocation() {
    let _ = ALLOCATIONS.try_with(|counter| {
        if let Some(previous) = counter.get() {
            counter.set(Some(previous + 1));
        }
    });
}
// SAFETY: Every operation delegates the original allocator arguments to System.
// The thread-local counter neither allocates nor changes allocator ownership.
unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocation();
        // SAFETY: Forwarding the allocation contract unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocation();
        // SAFETY: Forwarding the allocation contract unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: System owns the allocation described by pointer and layout.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocation();
        // SAFETY: Forwarding the allocation contract unchanged.
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Tracked = Tracked;

// Asymmetric signed orthogonal projection, rows permuted from the four-channel
// Hadamard basis. The independently written row oracle catches transposed data.
const ROWS: [[f32; 4]; 4] = [
    [0.5, 0.5, -0.5, -0.5],
    [0.5, -0.5, -0.5, 0.5],
    [0.5, 0.5, 0.5, 0.5],
    [0.5, -0.5, 0.5, -0.5],
];

fn square() -> (ProjectionMapping, Vec<f32>) {
    let mixing = vec![
        0.5, 0.5, 0.5, 0.5, 0.5, -0.5, 0.5, -0.5, -0.5, -0.5, 0.5, 0.5, -0.5, 0.5, 0.5, -0.5,
    ];
    let demixing = vec![
        16_384, 16_384, -16_384, -16_384, 16_384, -16_384, -16_384, 16_384, 16_384, 16_384, 16_384,
        16_384, 16_384, -16_384, 16_384, -16_384,
    ];
    (ProjectionMapping::new(3, 1, 4, demixing).unwrap(), mixing)
}

fn configs(mode: Mode, duration: FrameDuration) -> [EncoderConfig; 3] {
    [2, 1, 1].map(|channels| {
        let config = EncoderConfig::new(
            96_003 * channels as u32,
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
        .with_rate_control(RateControl::Constant);
        if mode == Mode::Celt {
            config
        } else {
            config.with_fec(true).unwrap()
        }
    })
}

fn signal(samples: usize, channels: usize, start: usize) -> Vec<f32> {
    signal_at_rate(samples, channels, start, 48_000)
}

fn signal_at_rate(samples: usize, channels: usize, start: usize, rate: u32) -> Vec<f32> {
    (0..samples * channels)
        .map(|index| {
            let time = (start + index / channels) as f32 / rate as f32;
            let frequency = 173.0 + (index % channels) as f32 * 109.0;
            let phase = std::f32::consts::TAU * frequency * time;
            0.12 * phase.sin() + 0.035 * (2.0 * phase).sin()
        })
        .collect()
}

#[test]
fn signed_projection_matches_independent_coded_pcm_and_stream_encoders() {
    for mode in [Mode::Silk, Mode::Hybrid, Mode::Celt] {
        for duration in [FrameDuration::Ms20, FrameDuration::Ms120] {
            for input_rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
                let (mapping, mixing) = square();
                let settings = configs(mode, duration)
                    .map(|config| config.with_sample_rate(input_rate).unwrap());
                let mut projected =
                    ProjectionEncoder::new(mapping.clone(), mixing.clone(), &settings).unwrap();
                let mut independent: Vec<_> = settings
                    .into_iter()
                    .map(|config| Encoder::new(config).unwrap())
                    .collect();
                let samples = settings[0].input_samples();
                assert_eq!(projected.sample_rate(), input_rate);
                assert_eq!(projected.input_samples(), samples);
                assert_eq!(
                    projected.algorithmic_delay_samples(),
                    independent[0].algorithmic_delay_samples()
                );
                let mut packet = vec![0xa5; projected.max_packet_bytes() + 17];
                for index in 0..4 {
                    let input = signal_at_rate(samples, 4, index * samples, input_rate);
                    let length = projected.encode(&input, &mut packet).unwrap();
                    assert!(packet[length..].iter().all(|&byte| byte == 0xa5));
                    let parsed = MultistreamPacket::parse(&packet[..length], 3).unwrap();
                    for (stream, actual) in parsed.packets().enumerate() {
                        let coded: &[usize] = match stream {
                            0 => &[0, 1],
                            1 => &[2],
                            _ => &[3],
                        };
                        let mut pcm = Vec::with_capacity(samples * coded.len());
                        for frame in input.as_chunks::<4>().0 {
                            for &row in coded {
                                let mixed: f64 = ROWS[row]
                                    .iter()
                                    .zip(frame)
                                    .map(|(&coefficient, &sample)| {
                                        f64::from(coefficient) * f64::from(sample)
                                    })
                                    .sum();
                                pcm.push(mixed as f32);
                            }
                        }
                        let mut expected = vec![0; settings[stream].max_packet_bytes()];
                        let bytes = independent[stream].encode(&pcm, &mut expected).unwrap();
                        let expected = Packet::parse(&expected[..bytes]).unwrap();
                        assert_eq!(actual.toc().config(), expected.toc().config());
                        assert_eq!(
                            actual.frames().collect::<Vec<_>>(),
                            expected.frames().collect::<Vec<_>>()
                        );
                    }
                    assert_eq!(
                        projected.final_range(),
                        independent
                            .iter()
                            .fold(0, |range, encoder| range ^ encoder.final_range())
                    );
                    packet.fill(0xa5);
                }
                assert_eq!(projected.channels(), 4);
                assert_eq!(projected.mapping(), &mapping);
                assert_eq!(projected.mixing_matrix(), mixing);
                assert!(projected.stream_encoder(3).is_none());
            }
        }
    }
}

#[test]
fn rectangular_pairs_and_silent_rows_match_an_independent_matrix_oracle() {
    let cases = [
        // C=3 < K=4: expand the non-diegetic stereo-plus-mono layout.
        (
            ProjectionMapping::new(
                3,
                1,
                3,
                vec![
                    16_384, 16_384, 0, 16_384, -16_384, 0, 0, 0, 16_384, 0, 0, 16_384,
                ],
            )
            .unwrap(),
            vec![1.0, 1.0, 0.0, 0.0, 1.0, -1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0],
            vec![
                vec![1.0, 1.0, 0.0],
                vec![1.0, -1.0, 0.0],
                vec![0.0, 0.0, 1.0],
                vec![0.0, 0.0, 1.0],
            ],
        ),
        // C=6 > K=4: retain W/Z and non-diegetic L/R, omit ACN rows 1/2.
        (
            ProjectionMapping::new(
                3,
                1,
                6,
                vec![
                    8_192, 0, 0, 16_384, 0, 0, 8_192, 0, 0, -16_384, 0, 0, 0, 0, 0, 0, 16_384, 0,
                    0, 0, 0, 0, 0, 16_384,
                ],
            )
            .unwrap(),
            vec![
                2.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, -1.0, 0.0, 0.0,
                0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 2.0,
            ],
            vec![
                vec![2.0, 0.0, 0.0, 1.0, 0.0, 0.0],
                vec![2.0, 0.0, 0.0, -1.0, 0.0, 0.0],
                vec![0.0, 0.0, 0.0, 0.0, 2.0, 0.0],
                vec![0.0, 0.0, 0.0, 0.0, 0.0, 2.0],
            ],
        ),
    ];
    for (mapping, mixing, expected_rows) in cases {
        let settings = configs(Mode::Celt, FrameDuration::Ms20);
        let mut independent: Vec<_> = settings
            .into_iter()
            .map(|config| Encoder::new(config).unwrap())
            .collect();
        let mut encoder = ProjectionEncoder::new(mapping.clone(), mixing, &settings).unwrap();
        let mut decoder = ProjectionDecoder::new(mapping.clone()).unwrap();
        let mut coded = MultistreamDecoder::new(*mapping.coded_layout()).unwrap();
        let channels = mapping.channels();
        let mut packet = vec![0; encoder.max_packet_bytes()];
        let mut output = vec![0.0; 960 * channels];
        let mut unprojected = vec![0.0; 960 * 4];
        for index in 0..3 {
            let input = signal(960, channels, index * 960);
            let length = encoder.encode(&input, &mut packet).unwrap();
            let parsed = MultistreamPacket::parse(&packet[..length], 3).unwrap();
            let mut first = 0;
            for (actual, independent) in parsed.packets().zip(&mut independent) {
                let count = independent.config().channels();
                let mut pcm = Vec::with_capacity(960 * count);
                for frame in input.chunks_exact(channels) {
                    for row in &expected_rows[first..first + count] {
                        let value: f64 = row
                            .iter()
                            .zip(frame)
                            .map(|(&coefficient, &sample)| coefficient * f64::from(sample))
                            .sum();
                        pcm.push(value as f32);
                    }
                }
                first += count;
                let mut expected = vec![0; independent.config().max_packet_bytes()];
                let bytes = independent.encode(&pcm, &mut expected).unwrap();
                let expected = Packet::parse(&expected[..bytes]).unwrap();
                assert_eq!(
                    actual.frames().collect::<Vec<_>>(),
                    expected.frames().collect::<Vec<_>>()
                );
            }
            assert_eq!(decoder.decode(&packet[..length], &mut output).unwrap(), 960);
            assert_eq!(
                coded.decode(&packet[..length], &mut unprojected).unwrap(),
                960
            );
            assert_eq!(decoder.final_range(), encoder.final_range());
            for (frame, actual) in unprojected
                .as_chunks::<4>()
                .0
                .iter()
                .zip(output.chunks_exact(channels))
            {
                for (row, &actual) in actual.iter().enumerate() {
                    let expected: f64 = (0..4)
                        .map(|column| {
                            f64::from(frame[column])
                                * f64::from(mapping.matrix()[column * channels + row])
                                / 32768.0
                        })
                        .sum();
                    assert!((f64::from(actual) - expected).abs() < 1e-6);
                }
            }
            if channels == 6 {
                for frame in output.as_chunks::<6>().0 {
                    assert_eq!(frame[1], 0.0);
                    assert_eq!(frame[2], 0.0);
                }
            }
        }
    }
}

#[test]
fn malformed_matrices_and_stream_geometry_are_rejected_at_construction() {
    let (mapping, mixing) = square();
    let settings = configs(Mode::Celt, FrameDuration::Ms20);
    assert!(matches!(
        ProjectionEncoder::new(mapping.clone(), mixing[..15].to_vec(), &settings),
        Err(Error::Mapping)
    ));
    for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
        let mut bad = mixing.clone();
        bad[15] = value;
        assert!(matches!(
            ProjectionEncoder::new(mapping.clone(), bad, &settings),
            Err(Error::Mapping)
        ));
    }
    assert!(matches!(
        ProjectionEncoder::new(mapping.clone(), mixing.clone(), &[]),
        Err(Error::Streams)
    ));
    let mut bad = settings;
    bad[0] = settings[1];
    assert!(matches!(
        ProjectionEncoder::new(mapping.clone(), mixing.clone(), &bad),
        Err(Error::Mapping)
    ));
    bad = settings;
    bad[2] = configs(Mode::Celt, FrameDuration::Ms120)[2];
    assert!(matches!(
        ProjectionEncoder::new(mapping.clone(), mixing.clone(), &bad),
        Err(Error::Duration)
    ));
    bad = settings;
    bad[2] = bad[2].with_sample_rate(8_000).unwrap();
    assert!(matches!(
        ProjectionEncoder::new(mapping, mixing, &bad),
        Err(Error::Encode(EncodeError::Argument))
    ));
}

#[test]
fn late_mixture_overload_and_invalid_arguments_preserve_history_and_output() {
    let (mapping, mixing) = square();
    let settings = configs(Mode::Celt, FrameDuration::Ms20);
    let mut encoder = ProjectionEncoder::new(mapping.clone(), mixing.clone(), &settings).unwrap();
    let mut pristine = ProjectionEncoder::new(mapping, mixing, &settings).unwrap();
    let input = signal(960, 4, 0);
    let mut output = vec![0xa5; encoder.max_packet_bytes()];
    let mut expected = output.clone();
    // Prime both histories so refusal must preserve a real prior interval.
    let length = encoder.encode(&input, &mut output).unwrap();
    assert_eq!(pristine.encode(&input, &mut expected).unwrap(), length);
    let range = encoder.final_range();
    output.fill(0xa5);
    let short = output.len() - 1;
    assert_eq!(
        encoder.encode(&input, &mut output[..short]),
        Err(Error::Output)
    );
    assert_eq!(
        encoder.encode(&input[..input.len() - 1], &mut output),
        Err(Error::Encode(EncodeError::Input))
    );
    for value in [f32::NAN, f32::INFINITY, 1.01] {
        let mut bad = input.clone();
        *bad.last_mut().unwrap() = value;
        assert_eq!(
            encoder.encode(&bad, &mut output),
            Err(Error::Encode(EncodeError::Input))
        );
    }
    let mut overloaded = input.clone();
    // Only the final sample and final coded row exceed normalized headroom.
    overloaded[input.len() - 4..].copy_from_slice(&[1.0, -1.0, 1.0, -1.0]);
    assert_eq!(
        encoder.encode(&overloaded, &mut output),
        Err(Error::Encode(EncodeError::Input))
    );
    assert_eq!(encoder.final_range(), range);
    assert!(!encoder.was_dtx());
    assert!(output.iter().all(|&byte| byte == 0xa5));
    let next = signal(960, 4, 960);
    let length = encoder.encode(&next, &mut output).unwrap();
    assert_eq!(pristine.encode(&next, &mut expected).unwrap(), length);
    assert_eq!(output[..length], expected[..length]);
    assert_eq!(encoder.final_range(), pristine.final_range());
    encoder.reset();
    pristine.reset();
    assert_eq!(encoder.final_range(), 0);
    let length = encoder.encode(&input, &mut output).unwrap();
    assert_eq!(pristine.encode(&input, &mut expected).unwrap(), length);
    assert_eq!(output[..length], expected[..length]);
}

#[test]
fn mixing_processing_reset_and_status_do_not_allocate() {
    let (mapping, mut mixing) = square();
    // Coded channel 2 is permanently silent even when source audio is active.
    for column in mixing.as_chunks_mut::<4>().0 {
        column[2] = 0.0;
    }
    let settings =
        configs(Mode::Silk, FrameDuration::Ms20).map(|config| config.with_dtx(true).unwrap());
    let mut encoder = ProjectionEncoder::new(mapping, mixing, &settings).unwrap();
    let input = signal(960, 4, 0);
    let mut output = vec![0; encoder.max_packet_bytes()];
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    for _ in 0..14 {
        encoder.encode(&input, &mut output).unwrap();
        let _ = encoder.final_range();
        let _ = encoder.was_dtx();
    }
    let silent = encoder.stream_encoder(1).unwrap().was_dtx();
    let active = encoder.stream_encoder(2).unwrap().was_dtx();
    encoder.reset();
    let reset_range = encoder.final_range();
    let reset_dtx = encoder.was_dtx();
    encoder.encode(&input, &mut output).unwrap();
    let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(allocations, 0);
    assert!(silent);
    assert!(!active);
    assert_eq!(reset_range, 0);
    assert!(!reset_dtx);
}

#[cfg(feature = "reference-tests")]
mod reference {
    use super::*;
    use snacka::opus::ogg::{
        OggDecoder, OggReader, OggWriter, OpusHead, OpusIdentification, OpusTags,
    };
    use std::{ffi::c_void, io::Cursor, ptr::NonNull};

    #[link(name = "opus")]
    unsafe extern "C" {
        fn opus_projection_decoder_create(
            rate: i32,
            channels: i32,
            streams: i32,
            coupled: i32,
            matrix: *mut u8,
            matrix_size: i32,
            error: *mut i32,
        ) -> *mut c_void;
        fn opus_projection_decoder_destroy(state: *mut c_void);
        fn opus_projection_decoder_ctl(state: *mut c_void, request: i32, ...) -> i32;
        fn opus_projection_decode_float(
            state: *mut c_void,
            packet: *const u8,
            length: i32,
            output: *mut f32,
            samples: i32,
            fec: i32,
        ) -> i32;
    }

    struct Native {
        state: NonNull<c_void>,
        channels: usize,
    }
    impl Native {
        fn from_opus_head(sample_rate: u32, header: &[u8]) -> Self {
            assert_eq!(&header[..8], b"OpusHead");
            assert_eq!(header[18], 3);
            let channels = usize::from(header[9]);
            let mut matrix = header[21..].to_vec();
            assert_eq!(
                matrix.len(),
                channels * usize::from(header[19] + header[20]) * 2
            );
            let mut error = 0;
            // SAFETY: Counts and the live Q15 matrix bytes came from the actual
            // serialized family-3 header, with precisely checked dimensions.
            let state = unsafe {
                opus_projection_decoder_create(
                    sample_rate as i32,
                    channels as i32,
                    i32::from(header[19]),
                    i32::from(header[20]),
                    matrix.as_mut_ptr(),
                    matrix.len() as i32,
                    &mut error,
                )
            };
            assert_eq!(error, 0);
            Self {
                state: NonNull::new(state).unwrap(),
                channels,
            }
        }

        fn decode(&mut self, packet: &[u8], samples: usize, output: &mut [f32]) -> u32 {
            assert!(output.len() >= samples * self.channels);
            // SAFETY: Owned state, input bytes and writable interleaved PCM
            // match the advertised packet length, channels and sample capacity.
            let decoded = unsafe {
                opus_projection_decode_float(
                    self.state.as_ptr(),
                    packet.as_ptr(),
                    packet.len() as i32,
                    output.as_mut_ptr(),
                    samples as i32,
                    0,
                )
            };
            assert_eq!(decoded, samples as i32);
            let mut range = 0u32;
            // SAFETY: OPUS_GET_FINAL_RANGE takes a live writable opus_uint32 pointer.
            let status =
                unsafe { opus_projection_decoder_ctl(self.state.as_ptr(), 4031, &mut range) };
            assert_eq!(status, 0);
            range
        }
    }
    impl Drop for Native {
        fn drop(&mut self) {
            // SAFETY: This unique state was returned by the matching create API.
            unsafe { opus_projection_decoder_destroy(self.state.as_ptr()) };
        }
    }

    #[test]
    fn q15_signed_endpoints_use_exact_denominator_in_owned_and_native_demixing() {
        let coefficients = [-32768, 32767, -16384, 8192];
        let mut demixing = vec![0; 16];
        let mut mixing = vec![0.0; 16];
        for (index, (coefficient, projection)) in coefficients
            .into_iter()
            .zip([-1.0, 1.0, -2.0, 4.0])
            .enumerate()
        {
            demixing[index * 4 + index] = coefficient;
            mixing[index * 4 + index] = projection;
        }
        let mapping = ProjectionMapping::new(3, 1, 4, demixing).unwrap();
        let header = OpusIdentification::projected(
            OpusHead {
                channels: 4,
                pre_skip: 0,
                input_sample_rate: 48_000,
                output_gain_q8: 0,
            },
            mapping.clone(),
        )
        .unwrap()
        .to_bytes()
        .unwrap();
        assert_eq!(&header[21..23], &[0, 128]);
        assert_eq!(&header[31..33], &[255, 127]);
        for input_rate in [8_000, 48_000] {
            let settings = configs(Mode::Celt, FrameDuration::Ms20)
                .map(|config| config.with_sample_rate(input_rate).unwrap());
            let mut encoder =
                ProjectionEncoder::new(mapping.clone(), mixing.clone(), &settings).unwrap();
            let samples = encoder.input_samples();
            let mut packet = vec![0; encoder.max_packet_bytes()];
            let mut packets = Vec::new();
            for index in 0..4 {
                let input = signal_at_rate(samples, 4, index * samples, input_rate);
                let length = encoder.encode(&input, &mut packet).unwrap();
                packets.push((packet[..length].to_vec(), encoder.final_range()));
            }
            for output_rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
                let mut owned =
                    ProjectionDecoder::with_sample_rate(mapping.clone(), output_rate).unwrap();
                let mut coded =
                    MultistreamDecoder::with_sample_rate(*mapping.coded_layout(), output_rate)
                        .unwrap();
                let mut native = Native::from_opus_head(output_rate, &header);
                let count = output_rate as usize / 50;
                let mut a = vec![0.0; count * 4];
                let mut b = a.clone();
                let mut unprojected = a.clone();
                for (packet, range) in &packets {
                    assert_eq!(owned.decode(packet, &mut a).unwrap(), count);
                    assert_eq!(coded.decode(packet, &mut unprojected).unwrap(), count);
                    assert_eq!(native.decode(packet, count, &mut b), *range);
                    assert_eq!(owned.final_range(), *range);
                    for (index, ((&own, &foreign), &coded)) in
                        a.iter().zip(&b).zip(&unprojected).enumerate()
                    {
                        let expected =
                            f64::from(coded) * f64::from(coefficients[index % 4]) / 32768.0;
                        assert!((f64::from(own) - expected).abs() < 1e-6);
                        assert!((own - foreign).abs() < 1e-5);
                    }
                }
            }
        }
    }

    #[test]
    fn family3_ogg_packets_decode_natively_with_serialized_matrices_and_trimming() {
        for mode in [Mode::Silk, Mode::Hybrid, Mode::Celt] {
            for duration in [FrameDuration::Ms20, FrameDuration::Ms120] {
                for input_rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
                    let (mapping, mixing) = square();
                    let settings = configs(mode, duration)
                        .map(|config| config.with_sample_rate(input_rate).unwrap());
                    let mut encoder = ProjectionEncoder::new(mapping, mixing, &settings).unwrap();
                    let delay_48khz =
                        encoder.algorithmic_delay_samples() * 48_000 / input_rate as usize;
                    let identification = OpusIdentification::projected(
                        OpusHead {
                            channels: 4,
                            pre_skip: delay_48khz as u16,
                            input_sample_rate: input_rate,
                            output_gain_q8: -512,
                        },
                        encoder.mapping().clone(),
                    )
                    .unwrap();
                    let samples = duration.samples();
                    let mut writer = OggWriter::new_mapped(
                        Vec::new(),
                        0x504a_0003,
                        identification,
                        &OpusTags::default(),
                    )
                    .unwrap();
                    let mut packet = vec![0; encoder.max_packet_bytes()];
                    let mut ranges = Vec::new();
                    let mut original = Vec::new();
                    for index in 0..4 {
                        let input = signal_at_rate(
                            encoder.input_samples(),
                            4,
                            index * encoder.input_samples(),
                            input_rate,
                        );
                        let length = encoder.encode(&input, &mut packet).unwrap();
                        ranges.push(encoder.final_range());
                        writer.write_packet(&packet[..length]).unwrap();
                        original.extend_from_slice(&input);
                    }
                    let valid = 4 * samples - 2 * delay_48khz;
                    let ogg = writer.finish(valid as u64).unwrap();
                    // Obtain the native decoder's C/N/M and D directly from the
                    // physical first page, independently of the owned header parser.
                    let laces = usize::from(ogg[26]);
                    assert_eq!(laces, 1);
                    let header_begin = 27 + laces;
                    let header = &ogg[header_begin..header_begin + usize::from(ogg[27])];
                    assert_eq!(&header[21..25], &[0, 64, 0, 64]);
                    for rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
                        let mut reader = OggReader::new(Cursor::new(&ogg)).unwrap();
                        assert_eq!(reader.identification().mapping_family, 3);
                        assert_eq!(reader.identification().to_bytes().unwrap(), header);
                        let mut native = Native::from_opus_head(rate, header);
                        let mut owned =
                            OggDecoder::with_sample_rate(reader.identification(), rate).unwrap();
                        let count = samples * rate as usize / 48_000;
                        let mut a = vec![0.0; count * 4];
                        let mut b = a.clone();
                        let mut kept = Vec::new();
                        let gain = reader.head().gain_factor();
                        let mut index = 0;
                        while let Some(packet) = reader.next_packet().unwrap() {
                            assert_eq!(packet.samples_48khz, samples);
                            assert_eq!(owned.decode(&packet.data, &mut a).unwrap(), count);
                            assert_eq!(native.decode(&packet.data, count, &mut b), ranges[index]);
                            assert_eq!(owned.final_range(), ranges[index]);
                            assert!(a.iter().chain(&b).all(|sample| sample.is_finite()));
                            if mode == Mode::Celt {
                                for (&owned, &native) in a.iter().zip(&b) {
                                    assert!((owned - native).abs() < 1e-5);
                                }
                            }
                            let begin = packet.keep.start * rate as usize / 48_000;
                            let end = packet.keep.end * rate as usize / 48_000;
                            kept.extend(a[begin * 4..end * 4].iter().map(|sample| sample * gain));
                            index += 1;
                        }
                        assert_eq!(index, ranges.len());
                        assert_eq!(kept.len(), valid * rate as usize / 48_000 * 4);
                        if mode == Mode::Celt && rate == input_rate {
                            let (error, energy) = original[..kept.len()].iter().zip(&kept).fold(
                                (0.0f64, 0.0f64),
                                |(error, energy), (&source, &decoded)| {
                                    let source = f64::from(source * gain);
                                    (
                                        error + (source - f64::from(decoded)).powi(2),
                                        energy + source * source,
                                    )
                                },
                            );
                            assert!(
                                error / energy < 0.1,
                                "projection roundtrip relative error {}",
                                error / energy
                            );
                        }
                    }
                }
            }
        }
    }
}
