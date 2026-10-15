// SPDX-License-Identifier: MIT
#![cfg(all(feature = "opus", feature = "reference-tests"))]
//! Public libopus mobile transport oracle, isolated from production builds.
//! Run with `cargo test --release --features opus,reference-tests --test opus_calls_reference`.

use std::{ffi::c_void, ptr::NonNull};

use snacka::opus::multistream::{
    ChannelMapping, Error as MultistreamError, MultistreamDecoder, MultistreamPacket,
};
use snacka::opus::projection::{ProjectionDecoder, ProjectionMapping};
use snacka::opus::repacketize::Repacketizer;
use snacka::opus::{DecodeError, Decoder, Mode, Packet, Recovery};

const RATES: [u32; 5] = [8_000, 12_000, 16_000, 24_000, 48_000];
const BITRATE: i32 = 4002;
const MAX_BANDWIDTH: i32 = 4004;
const VBR: i32 = 4006;
const BANDWIDTH: i32 = 4008;
const FEC: i32 = 4012;
const LOSS: i32 = 4014;
const DTX: i32 = 4016;
const CHANNELS: i32 = 4022;
const SIGNAL: i32 = 4024;
const RESET: i32 = 4028;
const FINAL_RANGE: i32 = 4031;
const SET_GAIN: i32 = 4034;
const GET_GAIN: i32 = 4045;
const LAST_PACKET_DURATION: i32 = 4039;
const SET_PHASE_DISABLED: i32 = 4046;
const GET_PHASE_DISABLED: i32 = 4047;
const DECODER_STATE: i32 = 5122;

// Only installed, documented opus.h and opus_multistream.h entry points.
#[link(name = "opus")]
unsafe extern "C" {
    fn opus_encoder_create(rate: i32, channels: i32, app: i32, error: *mut i32) -> *mut c_void;
    fn opus_encoder_destroy(state: *mut c_void);
    fn opus_encoder_ctl(state: *mut c_void, request: i32, ...) -> i32;
    fn opus_encode_float(
        state: *mut c_void,
        input: *const f32,
        samples: i32,
        output: *mut u8,
        capacity: i32,
    ) -> i32;
    fn opus_decoder_create(rate: i32, channels: i32, error: *mut i32) -> *mut c_void;
    fn opus_decoder_destroy(state: *mut c_void);
    fn opus_decoder_ctl(state: *mut c_void, request: i32, ...) -> i32;
    fn opus_decode_float(
        state: *mut c_void,
        packet: *const u8,
        length: i32,
        output: *mut f32,
        samples: i32,
        fec: i32,
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

struct Encoder {
    state: NonNull<c_void>,
    channels: usize,
}

impl Encoder {
    fn new(channels: usize, app: i32, bandwidth: i32, bitrate: i32) -> Self {
        let mut error = 0;
        // SAFETY: Supported channels/rate/application and live writable error storage.
        let state = unsafe { opus_encoder_create(48_000, channels as i32, app, &mut error) };
        assert_eq!(error, 0);
        let encoder = Self {
            state: NonNull::new(state).unwrap(),
            channels,
        };
        for (control, value) in [
            (BITRATE, bitrate),
            (VBR, 0),
            (CHANNELS, channels as i32),
            (MAX_BANDWIDTH, bandwidth),
            (BANDWIDTH, bandwidth),
        ] {
            encoder.set(control, value);
        }
        encoder
    }

    fn set(&self, control: i32, value: i32) {
        // SAFETY: Every control here takes a documented int; the owned encoder is live.
        let status = unsafe { opus_encoder_ctl(self.state.as_ptr(), control, value) };
        assert_eq!(status, 0);
    }

    fn encode(&mut self, input: &[f32]) -> Vec<u8> {
        assert_eq!(input.len() % self.channels, 0);
        let mut packet = vec![0; 8_192];
        // SAFETY: PCM and writable packet storage match the advertised lengths.
        let length = unsafe {
            opus_encode_float(
                self.state.as_ptr(),
                input.as_ptr(),
                (input.len() / self.channels) as i32,
                packet.as_mut_ptr(),
                packet.len() as i32,
            )
        };
        assert!(length > 0, "native encoder returned {length}");
        packet.truncate(length as usize);
        packet
    }
}

impl Drop for Encoder {
    fn drop(&mut self) {
        // SAFETY: This object solely owns the matching create allocation and frees it once.
        unsafe { opus_encoder_destroy(self.state.as_ptr()) };
    }
}

struct NativeDecoder {
    state: NonNull<c_void>,
    channels: usize,
    kind: u8,
}

impl NativeDecoder {
    fn new(rate: u32, channels: usize) -> Self {
        let mut error = 0;
        // SAFETY: Supported rate/channels with writable error output.
        let state = unsafe { opus_decoder_create(rate as i32, channels as i32, &mut error) };
        assert_eq!(error, 0);
        Self {
            state: NonNull::new(state).unwrap(),
            channels,
            kind: 0,
        }
    }

    fn mapped(rate: u32, mapping: ChannelMapping) -> Self {
        let mut error = 0;
        // SAFETY: Validated counts and map; native create copies its live mapping bytes.
        let state = unsafe {
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
        Self {
            state: NonNull::new(state).unwrap(),
            channels: mapping.output_channels(),
            kind: 1,
        }
    }

    fn projected(rate: u32, mapping: &ProjectionMapping) -> Self {
        let mut error = 0;
        let mut matrix: Vec<_> = mapping
            .matrix()
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        // SAFETY: Validated counts and complete initialized little-endian matrix;
        // native create copies the supplied bytes before this buffer is dropped.
        let state = unsafe {
            opus_projection_decoder_create(
                rate as i32,
                mapping.channels() as i32,
                i32::from(mapping.coded_layout().streams()),
                i32::from(mapping.coded_layout().coupled_streams()),
                matrix.as_mut_ptr(),
                matrix.len() as i32,
                &mut error,
            )
        };
        assert_eq!(error, 0);
        Self {
            state: NonNull::new(state).unwrap(),
            channels: mapping.channels(),
            kind: 2,
        }
    }

    fn decode(
        &mut self,
        packet: Option<&[u8]>,
        samples: usize,
        fec: bool,
        output: &mut [f32],
    ) -> usize {
        assert!(output.len() >= samples * self.channels);
        let (data, length) = packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len() as i32));
        // SAFETY: Owned state and packet are live; output holds the full interleaved
        // capacity. Null packet with zero length is the documented loss API.
        let result = unsafe {
            if self.kind == 2 {
                opus_projection_decode_float(
                    self.state.as_ptr(),
                    data,
                    length,
                    output.as_mut_ptr(),
                    samples as i32,
                    i32::from(fec),
                )
            } else if self.kind == 1 {
                opus_multistream_decode_float(
                    self.state.as_ptr(),
                    data,
                    length,
                    output.as_mut_ptr(),
                    samples as i32,
                    i32::from(fec),
                )
            } else {
                opus_decode_float(
                    self.state.as_ptr(),
                    data,
                    length,
                    output.as_mut_ptr(),
                    samples as i32,
                    i32::from(fec),
                )
            }
        };
        assert!(result > 0, "native decoder returned {result}");
        result as usize
    }

    fn final_range(&self) -> u32 {
        let mut range = 0;
        // SAFETY: GET_FINAL_RANGE takes the live writable opus_uint32 pointer.
        let status = unsafe {
            if self.kind == 2 {
                opus_projection_decoder_ctl(
                    self.state.as_ptr(),
                    FINAL_RANGE,
                    &mut range as *mut u32,
                )
            } else if self.kind == 1 {
                opus_multistream_decoder_ctl(
                    self.state.as_ptr(),
                    FINAL_RANGE,
                    &mut range as *mut u32,
                )
            } else {
                opus_decoder_ctl(self.state.as_ptr(), FINAL_RANGE, &mut range as *mut u32)
            }
        };
        assert_eq!(status, 0);
        range
    }

    fn reset(&mut self) {
        // SAFETY: RESET_STATE takes no variadic arguments and the decoder is owned.
        let status = unsafe {
            if self.kind == 2 {
                opus_projection_decoder_ctl(self.state.as_ptr(), RESET)
            } else if self.kind == 1 {
                opus_multistream_decoder_ctl(self.state.as_ptr(), RESET)
            } else {
                opus_decoder_ctl(self.state.as_ptr(), RESET)
            }
        };
        assert_eq!(status, 0);
    }

    fn set_gain_q8(&mut self, gain: i16) {
        // SAFETY: SET_GAIN takes a promoted int in the signed Q8 range on
        // ordinary, multistream and projection decoders.
        let status = unsafe {
            match self.kind {
                2 => opus_projection_decoder_ctl(self.state.as_ptr(), SET_GAIN, i32::from(gain)),
                1 => opus_multistream_decoder_ctl(self.state.as_ptr(), SET_GAIN, i32::from(gain)),
                _ => opus_decoder_ctl(self.state.as_ptr(), SET_GAIN, i32::from(gain)),
            }
        };
        assert_eq!(status, 0);
    }

    fn gain_q8(&self) -> i32 {
        let mut gain = 0;
        // SAFETY: GET_GAIN takes a live writable int pointer on each decoder API.
        let status = unsafe {
            match self.kind {
                2 => opus_projection_decoder_ctl(
                    self.state.as_ptr(),
                    GET_GAIN,
                    &mut gain as *mut i32,
                ),
                1 => opus_multistream_decoder_ctl(
                    self.state.as_ptr(),
                    GET_GAIN,
                    &mut gain as *mut i32,
                ),
                _ => opus_decoder_ctl(self.state.as_ptr(), GET_GAIN, &mut gain as *mut i32),
            }
        };
        assert_eq!(status, 0);
        gain
    }

    fn last_packet_samples(&self) -> usize {
        assert_eq!(self.kind, 0);
        let mut samples = 0;
        // SAFETY: Ordinary GET_LAST_PACKET_DURATION writes the live int slot;
        // the returned duration is measured at the decoder's output rate.
        let status = unsafe {
            opus_decoder_ctl(
                self.state.as_ptr(),
                LAST_PACKET_DURATION,
                &mut samples as *mut i32,
            )
        };
        assert_eq!(status, 0);
        assert!(samples >= 0);
        samples as usize
    }

    fn set_phase_inversion(&mut self, enabled: bool) {
        let disabled = i32::from(!enabled);
        // SAFETY: SET_PHASE_INVERSION_DISABLED takes one promoted int on all
        // three decoder APIs; the state stays owned and live.
        let code = unsafe {
            match self.kind {
                2 => opus_projection_decoder_ctl(self.state.as_ptr(), SET_PHASE_DISABLED, disabled),
                1 => {
                    opus_multistream_decoder_ctl(self.state.as_ptr(), SET_PHASE_DISABLED, disabled)
                }
                _ => opus_decoder_ctl(self.state.as_ptr(), SET_PHASE_DISABLED, disabled),
            }
        };
        assert_eq!(code, 0);
    }

    fn phase_inversion(&self) -> bool {
        let mut disabled = -1;
        // SAFETY: GET_PHASE_INVERSION_DISABLED writes a live int slot on each
        // decoder API without transferring ownership.
        let code = unsafe {
            match self.kind {
                2 => opus_projection_decoder_ctl(
                    self.state.as_ptr(),
                    GET_PHASE_DISABLED,
                    &mut disabled as *mut i32,
                ),
                1 => opus_multistream_decoder_ctl(
                    self.state.as_ptr(),
                    GET_PHASE_DISABLED,
                    &mut disabled as *mut i32,
                ),
                _ => opus_decoder_ctl(
                    self.state.as_ptr(),
                    GET_PHASE_DISABLED,
                    &mut disabled as *mut i32,
                ),
            }
        };
        assert_eq!(code, 0);
        assert!((0..=1).contains(&disabled));
        disabled == 0
    }

    fn stream_gain_q8(&self, stream: usize) -> i32 {
        self.stream_control_i32(stream, GET_GAIN)
    }

    fn stream_phase_inversion(&self, stream: usize) -> bool {
        self.stream_control_i32(stream, GET_PHASE_DISABLED) == 0
    }

    fn stream_control_i32(&self, stream: usize, request: i32) -> i32 {
        assert!(self.kind == 1 || self.kind == 2);
        let mut decoder = std::ptr::null_mut::<c_void>();
        // SAFETY: GET_DECODER_STATE takes an in-range stream index and a live
        // writable decoder-pointer slot. The parent owns the returned child.
        let status = unsafe {
            if self.kind == 2 {
                opus_projection_decoder_ctl(
                    self.state.as_ptr(),
                    DECODER_STATE,
                    stream as i32,
                    &mut decoder,
                )
            } else {
                opus_multistream_decoder_ctl(
                    self.state.as_ptr(),
                    DECODER_STATE,
                    stream as i32,
                    &mut decoder,
                )
            }
        };
        assert_eq!(status, 0);
        assert!(!decoder.is_null());
        let mut gain = 0;
        // SAFETY: The borrowed ordinary decoder remains live with its parent;
        // GET_GAIN/GET_PHASE_INVERSION_DISABLED write the supplied int without
        // transferring ownership.
        let status = unsafe { opus_decoder_ctl(decoder, request, &mut gain as *mut i32) };
        assert_eq!(status, 0);
        gain
    }
}

impl Drop for NativeDecoder {
    fn drop(&mut self) {
        // SAFETY: Each state is destroyed once through its matching public constructor.
        unsafe {
            if self.kind == 2 {
                opus_projection_decoder_destroy(self.state.as_ptr())
            } else if self.kind == 1 {
                opus_multistream_decoder_destroy(self.state.as_ptr())
            } else {
                opus_decoder_destroy(self.state.as_ptr())
            }
        }
    }
}

fn voice(samples: usize, channels: usize, start: usize) -> Vec<f32> {
    (0..samples * channels)
        .map(|index| {
            let sample = start + index / channels;
            let channel = index % channels;
            let t = sample as f32 / 48_000.0;
            let pitch = 137.0 + 29.0 * (std::f32::consts::TAU * 1.3 * t).sin();
            let phase = std::f32::consts::TAU * pitch * t + channel as f32 * 0.4;
            0.16 * phase.sin() + 0.055 * (2.0 * phase).sin() + 0.024 * (7.0 * phase).sin()
        })
        .collect()
}

fn multistream_celt_packets(frames: usize) -> Vec<Vec<u8>> {
    let mut encoders = [
        Encoder::new(2, 2051, 1105, 64_000),
        Encoder::new(1, 2051, 1103, 32_000),
        Encoder::new(1, 2051, 1101, 24_000),
    ];
    (0..frames)
        .map(|frame| {
            let packets: Vec<_> = encoders
                .iter_mut()
                .map(|encoder| encoder.encode(&voice(960, encoder.channels, frame * 960)))
                .collect();
            let inputs: Vec<_> = packets.iter().map(Vec::as_slice).collect();
            let mut transport = vec![0; packets.iter().map(Vec::len).sum::<usize>() + 4];
            let length =
                snacka::opus::multistream::MultistreamPacket::write(&inputs, &mut transport)
                    .unwrap();
            transport.truncate(length);
            transport
        })
        .collect()
}

fn finite(output: &[f32], context: &str) {
    assert!(
        output.iter().all(|v| v.is_finite() && v.abs() < 8.0),
        "invalid PCM: {context}"
    );
}

fn same_celt(owned: &[f32], native: &[f32], context: &str) {
    finite(owned, context);
    let error = owned
        .iter()
        .zip(native)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0, f32::max);
    assert!(error < 2e-4, "CELT PCM maximum error {error}: {context}");
}

struct GainDecoders {
    owned: [Decoder; 2],
    native: [NativeDecoder; 2],
    output: [Vec<f32>; 4],
    channels: usize,
    gain: i16,
}

fn compare_implicit_short_to_plc(pair: &mut GainDecoders, short: &[u8], tail: Option<&[u8]>) {
    let rate = pair.owned[0].sample_rate();
    let parsed = Packet::parse(short).unwrap();
    let missing = parsed.frame_samples(rate).unwrap();
    let count = parsed.samples(rate).unwrap();
    let prefix = missing * pair.channels;
    let sentinel = f32::from_bits(0x7fc0_1234);
    for output in &mut pair.output {
        output.fill(sentinel);
    }
    assert_eq!(
        pair.owned[0].decode(short, &mut pair.output[0]).unwrap(),
        count
    );
    assert_eq!(
        pair.native[0].decode(Some(short), count, false, &mut pair.output[2]),
        count
    );
    assert_eq!(
        pair.owned[1]
            .conceal(missing, &mut pair.output[1][..prefix])
            .unwrap(),
        missing
    );
    assert_eq!(
        pair.native[1].decode(None, missing, false, &mut pair.output[3][..prefix]),
        missing
    );
    if let Some(tail) = tail {
        assert_eq!(count, missing * 2);
        assert_eq!(
            pair.owned[1]
                .decode(tail, &mut pair.output[1][prefix..])
                .unwrap(),
            missing
        );
        assert_eq!(
            pair.native[1].decode(Some(tail), missing, false, &mut pair.output[3][prefix..]),
            missing
        );
    } else {
        assert_eq!(count, missing);
    }
    assert_eq!(pair.owned[0].final_range(), pair.owned[1].final_range());
    assert_eq!(pair.native[0].final_range(), pair.native[1].final_range());
    assert_eq!(pair.owned[0].final_range(), pair.native[0].final_range());
    assert_eq!(pair.owned[0].last_packet_samples(), count);
    assert_eq!(pair.native[0].last_packet_samples(), count);
    assert_eq!(pair.owned[1].last_packet_samples(), missing);
    assert_eq!(pair.native[1].last_packet_samples(), missing);
    for (first, second) in [(0, 1), (2, 3)] {
        for (&a, &b) in pair.output[first][..count * pair.channels]
            .iter()
            .zip(&pair.output[second])
        {
            assert!(a.is_finite() && b.is_finite());
            assert_eq!(
                a.to_bits(),
                b.to_bits(),
                "implicit short frame must preserve explicit-PLC history"
            );
        }
    }
    for output in &pair.output {
        assert!(
            output[count * pair.channels..]
                .iter()
                .all(|v| v.to_bits() == sentinel.to_bits())
        );
    }
    assert!(pair.owned.iter().all(|d| d.gain_q8() == 0));
    assert!(pair.native.iter().all(|d| d.gain_q8() == 0));
}

fn short_frame_continuation_packets() -> Vec<&'static [u8]> {
    let data = include_bytes!("fixtures/opus-short-frame-continuation.bin");
    let mut packets = Vec::new();
    let mut position = 0;
    while position < data.len() {
        let length = u32::from_be_bytes(data[position..position + 4].try_into().unwrap()) as usize;
        position += 8;
        packets.push(&data[position..position + length]);
        position += length;
    }
    assert_eq!(packets.len(), 6);
    packets
}

#[test]
fn implicit_short_frames_preserve_explicit_plc_and_coded_continuation() {
    // Reuse the exact diagnostic Hybrid/CELT witnesses, including inter-coded
    // continuations that expose history changes hidden by intra-coded packets.
    // The adjacent JSON records the two existing stream hashes and indexes.
    let packets = short_frame_continuation_packets();
    let mut cases = 0;
    for bank in packets.as_chunks::<3>().0 {
        let continuation = Packet::parse(bank[2]).unwrap();
        let toc = continuation.toc().byte() & 0xfc;
        let first_frame = continuation.frames().next().unwrap();
        let mut single_frame = vec![toc];
        single_frame.extend_from_slice(first_frame);
        for rate in RATES {
            for channels in [1, 2] {
                // Fresh, primed, and primed-then-reset histories must all use
                // the same PLC policy for implicit and explicit missing audio.
                for history in 0..3 {
                    for body in [None, Some(0), Some(255)] {
                        for mixed in [false, true] {
                            let mut pair = GainDecoders::new(rate, channels);
                            if history != 0 {
                                for packet in &bank[..2] {
                                    let samples =
                                        Packet::parse(packet).unwrap().samples(rate).unwrap();
                                    pair.check(Some(packet), samples, false);
                                }
                            }
                            if history == 2 {
                                pair.reset();
                            }
                            let mut short = vec![toc | if mixed { 2 } else { 0 }];
                            if mixed {
                                short.push(u8::from(body.is_some()));
                            }
                            if let Some(byte) = body {
                                short.push(byte);
                            }
                            if mixed {
                                short.extend_from_slice(first_frame);
                            }
                            compare_implicit_short_to_plc(
                                &mut pair,
                                &short,
                                mixed.then_some(single_frame.as_slice()),
                            );
                            pair.check(Some(bank[2]), continuation.samples(rate).unwrap(), false);
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 360);
}

#[test]
fn last_packet_samples_tracks_whole_packets_plc_and_refusals() {
    let packets = short_frame_continuation_packets();
    let long_packet = packets[5];
    for rate in RATES {
        for channels in [1, 2] {
            let mut pair = GainDecoders::new(rate, channels);
            let total = Packet::parse(long_packet).unwrap().samples(rate).unwrap();
            pair.check(Some(long_packet), total, false);
            let quantum = rate as usize / 400;
            pair.check(None, quantum * 3, false);
            pair.check(Some(&[0x60]), rate as usize / 100, false);
            pair.check(Some(&[0xfc, 0]), rate as usize / 50, false);
            pair.set_gain(3072);
            for decoder in &mut pair.owned {
                let previous = decoder.last_packet_samples();
                decoder.set_phase_inversion(false);
                assert_eq!(decoder.last_packet_samples(), previous);
                let mut output = vec![13.0; total * channels + 7];
                assert!(decoder.decode(&[3], &mut output).is_err());
                assert_eq!(
                    decoder.conceal(usize::MAX, &mut output),
                    Err(DecodeError::Argument)
                );
                assert_eq!(
                    decoder.recover(long_packet, usize::MAX, &mut output),
                    Err(DecodeError::Argument)
                );
                assert_eq!(decoder.last_packet_samples(), previous);
                assert!(output.iter().all(|&v| v == 13.0));
            }
            pair.refusals_preserve_output_and_history(long_packet, total);
            pair.reset();
            pair.check(Some(long_packet), total, false);
        }
    }
}

impl GainDecoders {
    fn new(rate: u32, channels: usize) -> Self {
        let pair = Self {
            owned: std::array::from_fn(|_| Decoder::with_sample_rate(channels, rate).unwrap()),
            native: std::array::from_fn(|_| NativeDecoder::new(rate, channels)),
            output: std::array::from_fn(|_| vec![0.0; rate as usize * 120 / 1000 * channels + 7]),
            channels,
            gain: 0,
        };
        for decoder in &pair.owned {
            assert_eq!(decoder.gain_q8(), 0);
            assert_eq!(decoder.last_packet_samples(), 0);
        }
        for decoder in &pair.native {
            assert_eq!(decoder.gain_q8(), 0);
            assert_eq!(decoder.last_packet_samples(), 0);
        }
        pair
    }

    fn set_gain(&mut self, gain: i16) {
        self.gain = gain;
        self.owned[1].set_gain_q8(gain);
        self.native[1].set_gain_q8(gain);
        assert_eq!(self.owned[1].gain_q8(), gain);
        assert_eq!(self.native[1].gain_q8(), i32::from(gain));
    }

    fn reset(&mut self) {
        for decoder in &mut self.owned {
            decoder.reset();
            assert_eq!(decoder.last_packet_samples(), 0);
        }
        for decoder in &mut self.native {
            decoder.reset();
            assert_eq!(decoder.last_packet_samples(), 0);
        }
        assert_eq!(self.owned[1].gain_q8(), self.gain);
        assert_eq!(self.native[1].gain_q8(), i32::from(self.gain));
    }

    fn check(&mut self, packet: Option<&[u8]>, samples: usize, fec: bool) -> Option<Recovery> {
        // A NaN suffix detects accidental extra writes without constraining
        // legitimate large positive-gain float output.
        let sentinel = f32::from_bits(0x7fc0_1234);
        for output in &mut self.output {
            output.fill(sentinel);
        }
        let mut recovery = None;
        for index in 0..2 {
            let decoder = &mut self.owned[index];
            let output = &mut self.output[index];
            match (packet, fec) {
                (Some(packet), true) => {
                    let actual = decoder.recover(packet, samples, output).unwrap();
                    if index == 0 {
                        recovery = Some(actual);
                    } else {
                        assert_eq!(Some(actual), recovery);
                    }
                }
                (Some(packet), false) => {
                    assert_eq!(decoder.decode(packet, output).unwrap(), samples);
                }
                (None, _) => {
                    assert_eq!(decoder.conceal(samples, output).unwrap(), samples);
                }
            }
            assert_eq!(
                self.native[index].decode(packet, samples, fec, &mut self.output[index + 2]),
                samples
            );
        }
        assert_eq!(self.owned[0].final_range(), self.owned[1].final_range());
        assert_eq!(self.native[0].final_range(), self.native[1].final_range());
        for decoder in &self.owned {
            assert_eq!(decoder.last_packet_samples(), samples);
        }
        for decoder in &self.native {
            assert_eq!(decoder.last_packet_samples(), samples);
        }
        if !fec {
            assert_eq!(self.owned[0].final_range(), self.native[0].final_range());
        }
        let count = samples * self.channels;
        let factor = 10.0f64.powf(f64::from(self.gain) / (20.0 * 256.0));
        for (plain, gained) in [(0, 1), (2, 3)] {
            for (&a, &b) in self.output[plain][..count]
                .iter()
                .zip(&self.output[gained][..count])
            {
                assert!(a.is_finite() && b.is_finite());
                if self.gain == 0 {
                    assert_eq!(
                        a.to_bits(),
                        b.to_bits(),
                        "decoder histories differ at gain 0"
                    );
                } else {
                    // Public APIs specify the dB formula, not a particular
                    // libm approximation. This independent f64 oracle allows
                    // 8 ppm relative float error, including near-zero output.
                    let expected = f64::from(a) * factor;
                    assert!(
                        (f64::from(b) - expected).abs() <= expected.abs() * 8e-6 + 1e-30,
                        "gain {}: {a} -> {b}, expected {expected}, implementation {plain}",
                        self.gain
                    );
                }
            }
        }
        for output in &self.output {
            assert!(
                output[count..]
                    .iter()
                    .all(|v| v.to_bits() == sentinel.to_bits())
            );
        }
        recovery
    }

    fn refusals_preserve_output_and_history(&mut self, packet: &[u8], samples: usize) {
        let output = &mut self.output[1];
        output.fill(13.0);
        let range = self.owned[1].final_range();
        let previous_samples = self.owned[1].last_packet_samples();
        assert_eq!(
            self.owned[1].decode(packet, &mut output[..samples * self.channels - 1]),
            Err(DecodeError::Output)
        );
        assert_eq!(
            self.owned[1].conceal(samples - 1, output),
            Err(DecodeError::Argument)
        );
        assert_eq!(
            self.owned[1].recover(packet, samples - 1, output),
            Err(DecodeError::Argument)
        );
        assert_eq!(
            self.owned[1].recover(packet, samples, &mut output[..samples * self.channels - 1]),
            Err(DecodeError::Output)
        );
        assert_eq!(self.owned[1].final_range(), range);
        assert_eq!(self.owned[1].last_packet_samples(), previous_samples);
        assert!(output.iter().all(|&v| v == 13.0));
    }
}

#[test]
fn ordinary_output_gain_matches_public_controls_and_preserves_history() {
    let mut cases = 0;
    let mut above_unity = false;
    for (mode, app, bandwidth, bitrate) in [
        (Mode::Silk, 2048, 1103, 24_000),
        (Mode::Hybrid, 2048, 1105, 24_000),
        (Mode::Celt, 2051, 1105, 64_000),
    ] {
        for coded_channels in [1, 2] {
            let mut encoder = Encoder::new(
                coded_channels,
                app,
                bandwidth,
                bitrate * coded_channels as i32,
            );
            encoder.set(SIGNAL, 3001);
            let packets: Vec<_> = (0..8)
                .map(|index| encoder.encode(&voice(960, coded_channels, index * 960)))
                .collect();
            assert!(
                packets
                    .iter()
                    .all(|p| Packet::parse(p).unwrap().toc().mode() == mode)
            );
            let mut repacketizer = Repacketizer::new();
            for packet in &packets[..6] {
                repacketizer.push(packet).unwrap();
            }
            let mut long_packet = vec![0; 8192];
            let long_length = repacketizer.write(0..6, &mut long_packet).unwrap();
            long_packet.truncate(long_length);
            for rate in RATES {
                for channels in [1, 2] {
                    let samples = rate as usize / 50;
                    let mut pair = GainDecoders::new(rate, channels);
                    for (index, gain) in [0, i16::MIN, -3072, -1, 1, 3072, i16::MAX]
                        .into_iter()
                        .enumerate()
                    {
                        pair.set_gain(gain);
                        pair.check(Some(&packets[index]), samples, false);
                        above_unity |= pair.output[1][..samples * channels]
                            .iter()
                            .any(|v| v.abs() > 1.0);
                        pair.check(None, samples, false);
                        assert_eq!(
                            pair.check(Some(&packets[index + 1]), samples, true),
                            Some(Recovery::Concealed)
                        );
                        pair.set_gain(0);
                        pair.check(Some(&packets[index + 1]), samples, false);
                    }
                    pair.set_gain(3072);
                    pair.reset();
                    pair.refusals_preserve_output_and_history(&packets[0], samples);
                    pair.check(Some(&packets[0]), samples, false);
                    pair.set_gain(0);
                    pair.check(Some(&packets[1]), samples, false);
                    pair.set_gain(i16::MAX);
                    pair.reset();
                    pair.check(Some(&long_packet), samples * 6, false);
                    cases += 1;
                }
            }
        }
    }
    assert_eq!(cases, 60);
    assert!(above_unity, "positive gain must not clamp floating output");
}

#[test]
fn ordinary_output_gain_applies_once_to_actual_fec_and_its_concealed_prefix() {
    for coded_channels in [1, 2] {
        let mut encoder = Encoder::new(coded_channels, 2048, 1103, 24_000 * coded_channels as i32);
        encoder.set(SIGNAL, 3001);
        encoder.set(FEC, 1);
        encoder.set(LOSS, 30);
        let packets: Vec<_> = (0..12)
            .map(|index| encoder.encode(&voice(960, coded_channels, index * 960)))
            .collect();
        for rate in RATES {
            for channels in [1, 2] {
                let samples = rate as usize / 50;
                let mut pair = GainDecoders::new(rate, channels);
                pair.check(Some(&packets[0]), samples, false);
                pair.check(Some(&packets[1]), samples, false);
                pair.set_gain(3072);
                // Two missing 20 ms packets: PLC for the older prefix and
                // actual LBRR for the immediately preceding packet.
                assert_eq!(
                    pair.check(Some(&packets[4]), samples * 2, true),
                    Some(Recovery::Fec)
                );
                assert!(
                    pair.output[0][..samples * channels]
                        .iter()
                        .any(|&v| v != 0.0)
                );
                assert!(
                    pair.output[0][samples * channels..samples * channels * 2]
                        .iter()
                        .any(|&v| v != 0.0)
                );
                pair.check(Some(&packets[4]), samples, false);
                pair.set_gain(0);
                pair.check(Some(&packets[5]), samples, false);
            }
        }
    }
}

#[test]
fn celt_mobile_rates_durations_bandwidths_and_channel_conversion() {
    let mut cases = 0;
    let mut mismatches = Vec::new();
    for frame in [120, 240, 480, 960] {
        for bandwidth in [1101, 1103, 1104, 1105] {
            for coded_channels in [1, 2] {
                let mut encoder = Encoder::new(coded_channels, 2051, bandwidth, 96_000);
                let packets: Vec<_> = (0..4)
                    .map(|index| encoder.encode(&voice(frame, coded_channels, index * frame)))
                    .collect();
                for rate in RATES {
                    for channels in [1, 2] {
                        let mut owned = Decoder::with_sample_rate(channels, rate).unwrap();
                        let mut native = NativeDecoder::new(rate, channels);
                        let samples = frame * rate as usize / 48_000;
                        let mut a = vec![0.0; samples * channels];
                        let mut b = a.clone();
                        for (index, packet) in packets.iter().enumerate() {
                            assert_eq!(Packet::parse(packet).unwrap().toc().mode(), Mode::Celt);
                            let context = format!(
                                "rate {rate}, coded {coded_channels}, output {channels}, bandwidth {bandwidth}, frame {frame}, index {index}"
                            );
                            assert_eq!(owned.decode(packet, &mut a).unwrap(), samples, "{context}");
                            assert_eq!(
                                native.decode(Some(packet), samples, false, &mut b),
                                samples
                            );
                            assert_eq!(
                                owned.final_range(),
                                native.final_range(),
                                "range {context}"
                            );
                            finite(&a, &context);
                            let error = a
                                .iter()
                                .zip(&b)
                                .map(|(a, b)| (a - b).abs())
                                .fold(0.0, f32::max);
                            if error >= 2e-4 {
                                if mismatches.is_empty() && index == 0 {
                                    let mut stereo = NativeDecoder::new(rate, 2);
                                    let mut stereo_pcm = vec![0.0; samples * 2];
                                    stereo.decode(Some(packet), samples, false, &mut stereo_pcm);
                                    let average: Vec<_> = stereo_pcm
                                        .as_chunks::<2>()
                                        .0
                                        .iter()
                                        .map(|s| (s[0] + s[1]) * 0.5)
                                        .collect();
                                    let mono_average_error = average
                                        .iter()
                                        .zip(&b)
                                        .map(|(a, b)| (a - b).abs())
                                        .fold(0.0, f32::max);
                                    eprintln!(
                                        "native mono vs averaged native stereo error {mono_average_error}: {context}"
                                    );
                                    let directory =
                                        std::path::Path::new("/tmp/snacka-opus-channel-oracle");
                                    std::fs::create_dir_all(directory).unwrap();
                                    std::fs::write(directory.join("packet.opus.raw"), packet)
                                        .unwrap();
                                    for (name, pcm) in [
                                        ("native-mono.f32", &b),
                                        ("native-stereo.f32", &stereo_pcm),
                                        ("owned-mono.f32", &a),
                                    ] {
                                        let bytes: Vec<_> = pcm
                                            .iter()
                                            .flat_map(|value| value.to_le_bytes())
                                            .collect();
                                        std::fs::write(directory.join(name), bytes).unwrap();
                                    }
                                }
                                mismatches.push(format!("{error}: {context}"));
                            }
                            cases += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(cases, 1_280);
    assert!(
        mismatches.is_empty(),
        "{} PCM mismatches: {}",
        mismatches.len(),
        mismatches.join("\n")
    );
}

#[test]
fn repacketized_cbr_vbr_padding_and_maximum_duration_decode_natively() {
    for channels in [1, 2] {
        for count in [1, 2, 3, 48] {
            for vbr in [false, true] {
                let mut encoder = Encoder::new(channels, 2051, 1105, 96_000);
                let packets: Vec<_> = (0..count)
                    .map(|frame| {
                        if vbr {
                            encoder.set(BITRATE, if frame % 2 == 0 { 96_000 } else { 128_000 });
                        }
                        encoder.encode(&voice(120, channels, frame * 120))
                    })
                    .collect();
                let mut builder = Repacketizer::new();
                for packet in &packets {
                    builder.push(packet).unwrap();
                }
                let size = builder.output_size(0..count).unwrap();
                for padding in [0, 1, 257] {
                    let mut packet = vec![0; size + padding];
                    assert_eq!(
                        builder
                            .write_padded(0..count, &mut packet, size + padding)
                            .unwrap(),
                        packet.len()
                    );
                    assert_eq!(Packet::parse(&packet).unwrap().frame_count(), count);
                    for rate in RATES {
                        let samples = count * 120 * rate as usize / 48_000;
                        let mut owned = Decoder::with_sample_rate(channels, rate).unwrap();
                        let mut native = NativeDecoder::new(rate, channels);
                        let mut a = vec![0.0; samples * channels];
                        let mut b = a.clone();
                        assert_eq!(owned.decode(&packet, &mut a).unwrap(), samples);
                        assert_eq!(
                            native.decode(Some(&packet), samples, false, &mut b),
                            samples
                        );
                        assert_eq!(owned.final_range(), native.final_range());
                        same_celt(&a, &b, "repacketized mobile packet");
                    }
                }
            }
        }
    }
}

#[test]
fn owned_long_packets_match_native_durations_ranges_and_return_from_loss() {
    use snacka::opus::{
        Application, Bandwidth, Encoder as OwnedEncoder, EncoderConfig, FrameDuration, RateControl,
    };
    for (mode, bandwidth) in [
        (Mode::Silk, Bandwidth::Narrowband),
        (Mode::Silk, Bandwidth::Mediumband),
        (Mode::Silk, Bandwidth::Wideband),
        (Mode::Hybrid, Bandwidth::Superwideband),
        (Mode::Hybrid, Bandwidth::Fullband),
        (Mode::Celt, Bandwidth::Narrowband),
        (Mode::Celt, Bandwidth::Wideband),
        (Mode::Celt, Bandwidth::Superwideband),
        (Mode::Celt, Bandwidth::Fullband),
    ] {
        for channels in [1, 2] {
            for duration in [
                FrameDuration::Ms80,
                FrameDuration::Ms100,
                FrameDuration::Ms120,
            ] {
                for policy in [RateControl::Constant, RateControl::Variable] {
                    let config = EncoderConfig::new(
                        64_000 * channels as u32,
                        channels,
                        duration,
                        bandwidth,
                        Application::Voice,
                    )
                    .unwrap()
                    .with_mode(mode)
                    .unwrap()
                    .with_rate_control(policy);
                    let config = if mode == Mode::Celt {
                        config
                    } else {
                        config.with_fec(true).unwrap()
                    };
                    let mut encoder = OwnedEncoder::new(config).unwrap();
                    let samples = duration.samples();
                    let mut packet = vec![0; config.max_packet_bytes()];
                    let mut decoders: Vec<_> = RATES
                        .into_iter()
                        .map(|rate| {
                            (
                                Decoder::with_sample_rate(channels, rate).unwrap(),
                                NativeDecoder::new(rate, channels),
                            )
                        })
                        .collect();
                    let mut a = vec![0.0; samples * channels];
                    let mut b = a.clone();
                    let mut recovered = false;
                    for index in 0..8 {
                        let length = encoder
                            .encode(&voice(samples, channels, index * samples), &mut packet)
                            .unwrap();
                        if index == 0 {
                            assert!(
                                !encoder.has_fec(),
                                "intra-packet LBRR is not previous-packet recovery"
                            );
                        }
                        for (&rate, (owned, native)) in RATES.iter().zip(&mut decoders) {
                            let expected = samples * rate as usize / 48_000;
                            if index == 5 {
                                continue;
                            }
                            if index == 6 {
                                let recovery =
                                    owned.recover(&packet[..length], expected, &mut a).unwrap();
                                recovered |= recovery == Recovery::Fec;
                                assert_eq!(
                                    native.decode(Some(&packet[..length]), expected, true, &mut b),
                                    expected
                                );
                                assert_eq!(encoder.has_fec(), recovery == Recovery::Fec);
                            }
                            if index == 3 {
                                assert_eq!(owned.conceal(expected, &mut a).unwrap(), expected);
                                assert_eq!(native.decode(None, expected, false, &mut b), expected);
                                continue;
                            }
                            assert_eq!(owned.decode(&packet[..length], &mut a).unwrap(), expected);
                            assert_eq!(
                                native.decode(Some(&packet[..length]), expected, false, &mut b),
                                expected
                            );
                            assert_eq!(owned.final_range(), encoder.final_range());
                            assert_eq!(native.final_range(), encoder.final_range());
                            assert!(
                                a[..expected * channels]
                                    .iter()
                                    .chain(&b[..expected * channels])
                                    .all(|value| value.is_finite())
                            );
                        }
                    }
                    assert_eq!(recovered, mode != Mode::Celt);
                }
            }
        }
    }
}

trait ComposedGainDecoder {
    fn set_phase_inversion(&mut self, enabled: bool);
    fn phase_inversion(&self) -> bool;
    fn set_gain_q8(&mut self, gain: i16);
    fn gain_q8(&self) -> i16;
    fn reset(&mut self);
    fn final_range(&self) -> u32;
    fn decode(&mut self, packet: &[u8], output: &mut [f32]) -> Result<usize, MultistreamError>;
    fn conceal(&mut self, samples: usize, output: &mut [f32]) -> Result<usize, MultistreamError>;
    fn recover(
        &mut self,
        packet: &[u8],
        samples: usize,
        output: &mut [f32],
    ) -> Result<Recovery, MultistreamError>;
}

macro_rules! composed_gain_decoder {
    ($codec:ty) => {
        impl ComposedGainDecoder for $codec {
            fn set_phase_inversion(&mut self, enabled: bool) {
                <$codec>::set_phase_inversion(self, enabled);
            }
            fn phase_inversion(&self) -> bool {
                <$codec>::phase_inversion(self)
            }
            fn set_gain_q8(&mut self, gain: i16) {
                <$codec>::set_gain_q8(self, gain);
            }
            fn gain_q8(&self) -> i16 {
                <$codec>::gain_q8(self)
            }
            fn reset(&mut self) {
                <$codec>::reset(self);
            }
            fn final_range(&self) -> u32 {
                <$codec>::final_range(self)
            }
            fn decode(
                &mut self,
                packet: &[u8],
                output: &mut [f32],
            ) -> Result<usize, MultistreamError> {
                <$codec>::decode(self, packet, output)
            }
            fn conceal(
                &mut self,
                samples: usize,
                output: &mut [f32],
            ) -> Result<usize, MultistreamError> {
                <$codec>::conceal(self, samples, output)
            }
            fn recover(
                &mut self,
                packet: &[u8],
                samples: usize,
                output: &mut [f32],
            ) -> Result<Recovery, MultistreamError> {
                <$codec>::recover(self, packet, samples, output)
            }
        }
    };
}
composed_gain_decoder!(MultistreamDecoder);
composed_gain_decoder!(ProjectionDecoder);

fn composed_gain_operation<D: ComposedGainDecoder>(
    owned: &mut [D; 2],
    native: &mut [NativeDecoder; 2],
    output: &mut [Vec<f32>; 4],
    packet: Option<&[u8]>,
    samples: usize,
    fec: bool,
) -> Option<Recovery> {
    let sentinel = f32::from_bits(0x7fc0_1234);
    for buffer in &mut *output {
        buffer.fill(sentinel);
    }
    let mut recovery = None;
    for index in 0..2 {
        match (packet, fec) {
            (Some(packet), true) => {
                let actual = owned[index]
                    .recover(packet, samples, &mut output[index])
                    .unwrap();
                if index == 0 {
                    recovery = Some(actual);
                } else {
                    assert_eq!(Some(actual), recovery);
                }
            }
            (Some(packet), false) => {
                assert_eq!(
                    owned[index].decode(packet, &mut output[index]).unwrap(),
                    samples
                );
            }
            (None, _) => {
                assert_eq!(
                    owned[index].conceal(samples, &mut output[index]).unwrap(),
                    samples
                );
            }
        }
        assert_eq!(
            native[index].decode(packet, samples, fec, &mut output[index + 2]),
            samples
        );
    }
    assert_eq!(owned[0].final_range(), owned[1].final_range());
    assert_eq!(native[0].final_range(), native[1].final_range());
    if !fec {
        assert_eq!(owned[0].final_range(), native[0].final_range());
    }
    let count = samples * native[0].channels;
    let gain = owned[1].gain_q8();
    let factor = 10.0f64.powf(f64::from(gain) / 5120.0);
    for (plain, gained) in [(0, 1), (2, 3)] {
        let scale = output[plain][..count]
            .iter()
            .map(|sample| f64::from(sample.abs()))
            .fold(0.0, f64::max)
            * factor;
        for (&a, &b) in output[plain][..count].iter().zip(&output[gained][..count]) {
            assert!(a.is_finite() && b.is_finite());
            if gain == 0 {
                assert_eq!(
                    a.to_bits(),
                    b.to_bits(),
                    "gain changed composed codec history"
                );
            } else {
                let expected = f64::from(a) * factor;
                // A projected output can nearly cancel. Include the other
                // output rows' scale rather than dividing by that near-zero
                // result; the independent oracle below checks gained coded
                // channels against the owned demix contract using contribution
                // bounds, without requiring native floating-point bit order.
                assert!((f64::from(b) - expected).abs() <= (expected.abs() + scale) * 8e-6 + 1e-30);
            }
        }
    }
    for buffer in output {
        assert!(
            buffer[count..]
                .iter()
                .all(|v| v.to_bits() == sentinel.to_bits())
        );
    }
    recovery
}

fn composed_gain_contract<D: ComposedGainDecoder>(
    owned: impl Fn(u32) -> D,
    native: impl Fn(u32) -> NativeDecoder,
    packets: &[Vec<u8>],
    actual_fec: bool,
    silent_channel: Option<usize>,
    alias: Option<(usize, usize)>,
) {
    let mut above_unity = false;
    for rate in RATES {
        let samples = rate as usize / 50;
        for gain in [0, i16::MIN, -3072, -1, 1, 3072, i16::MAX] {
            let mut owned = std::array::from_fn(|_| owned(rate));
            let mut native = std::array::from_fn(|_| native(rate));
            assert!(owned.iter().all(|d| d.gain_q8() == 0));
            assert!(native.iter().all(|d| d.gain_q8() == 0));
            let channels = native[0].channels;
            let mut output = std::array::from_fn(|_| vec![0.0; samples * 2 * channels + 7]);
            owned[1].set_gain_q8(gain);
            native[1].set_gain_q8(gain);
            for reset in [false, true] {
                if reset {
                    for decoder in &mut owned {
                        decoder.reset();
                    }
                    for decoder in &mut native {
                        decoder.reset();
                    }
                }
                assert_eq!(owned[1].gain_q8(), gain);
                assert_eq!(native[1].gain_q8(), i32::from(gain));
                for stream in 0..3 {
                    assert_eq!(native[0].stream_gain_q8(stream), 0);
                    assert_eq!(native[1].stream_gain_q8(stream), i32::from(gain));
                }
                for packet in &packets[..2] {
                    composed_gain_operation(
                        &mut owned,
                        &mut native,
                        &mut output,
                        Some(packet),
                        samples,
                        false,
                    );
                }
                composed_gain_operation(&mut owned, &mut native, &mut output, None, samples, false);
                assert_eq!(
                    composed_gain_operation(
                        &mut owned,
                        &mut native,
                        &mut output,
                        Some(&packets[5]),
                        samples * 2,
                        true
                    ),
                    Some(if actual_fec {
                        Recovery::Fec
                    } else {
                        Recovery::Concealed
                    })
                );
                if actual_fec {
                    for plain in [0, 2] {
                        assert!(
                            output[plain][..samples * channels]
                                .iter()
                                .any(|&v| v != 0.0)
                        );
                        assert!(
                            output[plain][samples * channels..samples * channels * 2]
                                .iter()
                                .any(|&v| v != 0.0)
                        );
                    }
                }
                composed_gain_operation(
                    &mut owned,
                    &mut native,
                    &mut output,
                    Some(&packets[5]),
                    samples,
                    false,
                );
                above_unity |= output[1][..samples * channels]
                    .iter()
                    .any(|v| v.abs() > 1.0);
                for buffer in &output {
                    for frame in buffer[..samples * channels].chunks_exact(channels) {
                        if let Some(channel) = silent_channel {
                            assert_eq!(frame[channel], 0.0);
                        }
                        if let Some((first, second)) = alias {
                            assert_eq!(frame[first].to_bits(), frame[second].to_bits());
                        }
                    }
                }
                let before = output[1].clone();
                let range = owned[1].final_range();
                assert!(
                    owned[1]
                        .decode(&packets[0], &mut output[1][..samples * channels - 1])
                        .is_err()
                );
                assert_eq!(owned[1].final_range(), range);
                assert!(
                    output[1]
                        .iter()
                        .zip(&before)
                        .all(|(a, b)| a.to_bits() == b.to_bits())
                );
            }
            owned[1].set_gain_q8(0);
            native[1].set_gain_q8(0);
            composed_gain_operation(
                &mut owned,
                &mut native,
                &mut output,
                Some(&packets[6]),
                samples,
                false,
            );
        }
    }
    assert!(
        above_unity,
        "composed float output gain must preserve headroom"
    );
}

#[test]
fn mapped_and_projected_gain_controls_match_native_and_preserve_history() {
    // Stream 2 is entirely omitted; the coupled stream, channel alias and
    // explicit silent output exercise gain before the existing mapping stage.
    let mapped = ChannelMapping::new(3, 1, &[2, 0, 1, 255, 0]).unwrap();
    let projected = ProjectionMapping::new(
        3,
        1,
        4,
        vec![
            16384, 8192, 0, -16384, -8192, 16384, 0, 0, 0, 4096, 0, -32768, 32767, 0, 0, 8192,
        ],
    )
    .unwrap();
    for actual_fec in [false, true] {
        let packets = if actual_fec {
            let mut encoders = [2, 1, 1].map(|channels| {
                let encoder = Encoder::new(channels, 2048, 1103, 24_000 * channels as i32);
                encoder.set(SIGNAL, 3001);
                encoder.set(FEC, 1);
                encoder.set(LOSS, 30);
                encoder
            });
            (0..8)
                .map(|frame| {
                    let streams: Vec<_> = encoders
                        .iter_mut()
                        .map(|encoder| encoder.encode(&voice(960, encoder.channels, frame * 960)))
                        .collect();
                    let streams: Vec<_> = streams.iter().map(Vec::as_slice).collect();
                    let mut packet = vec![0; 8192];
                    let length = MultistreamPacket::write(&streams, &mut packet).unwrap();
                    packet.truncate(length);
                    packet
                })
                .collect()
        } else {
            multistream_celt_packets(8)
        };
        composed_gain_contract(
            |rate| MultistreamDecoder::with_sample_rate(mapped, rate).unwrap(),
            |rate| NativeDecoder::mapped(rate, mapped),
            &packets,
            actual_fec,
            Some(3),
            Some((1, 4)),
        );
        composed_gain_contract(
            |rate| ProjectionDecoder::with_sample_rate(projected.clone(), rate).unwrap(),
            |rate| NativeDecoder::projected(rate, &projected),
            &packets,
            actual_fec,
            Some(2),
            None,
        );
    }
}

#[test]
fn projected_gain_matches_independent_gained_coded_channel_matrix() {
    let packets = multistream_celt_packets(3);
    for channels in [3, 4, 6] {
        let matrix = (0..4)
            .flat_map(|column| {
                (0..channels).map(move |row| {
                    if row == 1 {
                        0
                    } else {
                        [32767, -32768, 16384, -8192][(column + row) % 4]
                    }
                })
            })
            .collect();
        let mapping = ProjectionMapping::new(3, 1, channels as u8, matrix).unwrap();
        for rate in RATES {
            let samples = rate as usize / 50;
            for gain in [0, i16::MIN, -3072, 3072, i16::MAX] {
                let mut coded =
                    MultistreamDecoder::with_sample_rate(*mapping.coded_layout(), rate).unwrap();
                let mut projected =
                    ProjectionDecoder::with_sample_rate(mapping.clone(), rate).unwrap();
                coded.set_gain_q8(gain);
                projected.set_gain_q8(gain);
                // Native rectangular projection behavior has separate documented
                // limitations. Use its actual projection API only for C == K.
                let mut native = (channels == 4).then(|| {
                    let mut coded = NativeDecoder::mapped(rate, *mapping.coded_layout());
                    let mut projected = NativeDecoder::projected(rate, &mapping);
                    coded.set_gain_q8(gain);
                    projected.set_gain_q8(gain);
                    (coded, projected)
                });
                let mut input = vec![0.0; samples * 4];
                let mut native_input = input.clone();
                let mut output = vec![123.0; samples * channels + 7];
                let mut native_output = output.clone();
                for packet in &packets {
                    assert_eq!(coded.decode(packet, &mut input).unwrap(), samples);
                    assert_eq!(projected.decode(packet, &mut output).unwrap(), samples);
                    assert_eq!(coded.final_range(), projected.final_range());
                    if let Some((coded, projected)) = &mut native {
                        assert_eq!(
                            coded.decode(Some(packet), samples, false, &mut native_input),
                            samples
                        );
                        assert_eq!(
                            projected.decode(Some(packet), samples, false, &mut native_output),
                            samples
                        );
                        assert_eq!(projected.final_range(), coded.final_range());
                    }
                    for (coded_pcm, projected_pcm) in std::iter::once((&input, &output))
                        .chain(native.as_ref().map(|_| (&native_input, &native_output)))
                    {
                        for (frame, actual) in coded_pcm
                            .as_chunks::<4>()
                            .0
                            .iter()
                            .zip(projected_pcm[..samples * channels].chunks_exact(channels))
                        {
                            for (row, &actual) in actual.iter().enumerate() {
                                let terms = (0..4).map(|column| {
                                    f64::from(frame[column])
                                        * f64::from(mapping.matrix()[column * channels + row])
                                        / 32768.0
                                });
                                let expected = terms.clone().sum::<f64>();
                                let magnitude = terms.map(f64::abs).sum::<f64>();
                                assert!(actual.is_finite());
                                assert!(
                                    (f64::from(actual) - expected).abs()
                                        <= magnitude * f64::from(f32::EPSILON) * 8.0 + 1e-30
                                );
                            }
                        }
                        assert!(
                            projected_pcm[samples * channels..]
                                .iter()
                                .all(|&v| v == 123.0)
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn mapped_multistream_aliases_silence_and_xor_range_match_native() {
    let mapping = ChannelMapping::new(3, 1, &[0, 2, 1, 3, 255, 0]).unwrap();
    let packets = multistream_celt_packets(5);
    for rate in RATES {
        let mut owned = MultistreamDecoder::with_sample_rate(mapping, rate).unwrap();
        let mut native = NativeDecoder::mapped(rate, mapping);
        let samples = rate as usize / 50;
        let mut a = vec![0.0; samples * 6];
        let mut b = a.clone();
        for packet in &packets {
            assert_eq!(owned.decode(packet, &mut a).unwrap(), samples);
            assert_eq!(native.decode(Some(packet), samples, false, &mut b), samples);
            assert_eq!(owned.final_range(), native.final_range());
            same_celt(&a, &b, "mapped multistream");
            for sample in a.as_chunks::<6>().0 {
                assert_eq!(sample[0], sample[5]);
                assert_eq!(sample[4], 0.0);
            }
        }
    }
}

#[test]
fn silk_hybrid_duration_loss_fec_and_following_packet_range() {
    let mut modes = [false; 3];
    let mut recovered = 0;
    for frame in [480, 960, 1_920, 2_880] {
        for (bandwidth, bitrate) in [
            (1101, 12_000),
            (1102, 16_000),
            (1103, 24_000),
            (1104, 24_000),
            (1105, 24_000),
        ] {
            let mut encoder = Encoder::new(1, 2048, bandwidth, bitrate);
            encoder.set(SIGNAL, 3001);
            // With a high loss estimate, public libopus selects SILK WB even
            // when SWB/FB was requested. Keep those cases in Hybrid by letting
            // only the three speech bandwidths request in-band redundancy.
            encoder.set(FEC, i32::from(bandwidth <= 1103));
            encoder.set(LOSS, if bandwidth <= 1103 { 30 } else { 0 });
            let packets: Vec<_> = (0..8)
                .map(|index| encoder.encode(&voice(frame, 1, index * frame)))
                .collect();
            for packet in &packets {
                modes[match Packet::parse(packet).unwrap().toc().mode() {
                    Mode::Silk => 0,
                    Mode::Hybrid => 1,
                    Mode::Celt => 2,
                }] = true;
            }
            for rate in RATES {
                let samples = frame * rate as usize / 48_000;
                for channels in [1, 2] {
                    let mut owned = Decoder::with_sample_rate(channels, rate).unwrap();
                    let mut native = NativeDecoder::new(rate, channels);
                    let mut a = vec![0.0; samples * channels];
                    let mut b = a.clone();
                    for (index, packet) in packets.iter().enumerate() {
                        if index == 2 {
                            continue;
                        }
                        if index == 3 {
                            recovered += usize::from(
                                owned.recover(packet, samples, &mut a).unwrap() == Recovery::Fec,
                            );
                            native.decode(Some(packet), samples, true, &mut b);
                            finite(&a, "FEC/missing interval");
                        }
                        let context = format!(
                            "LP rate {rate}, output {channels}, bandwidth {bandwidth}, frame {frame}, index {index}"
                        );
                        assert_eq!(owned.decode(packet, &mut a).unwrap(), samples, "{context}");
                        assert_eq!(native.decode(Some(packet), samples, false, &mut b), samples);
                        assert_eq!(owned.final_range(), native.final_range(), "{context}");
                        finite(&a, &context);
                    }
                }
            }
        }
    }
    assert!(
        modes[0] && modes[1],
        "native public controls did not exercise SILK and Hybrid: {modes:?}"
    );
    assert!(
        recovered > 0,
        "native packets did not exercise actual in-band FEC"
    );
}

#[test]
fn native_dtx_burst_loss_reset_and_refusal_keep_call_state() {
    let mut encoder = Encoder::new(1, 2048, 1103, 16_000);
    encoder.set(SIGNAL, 3001);
    encoder.set(DTX, 1);
    let mut packets: Vec<_> = (0..8)
        .map(|frame| encoder.encode(&voice(960, 1, frame * 960)))
        .collect();
    packets.extend((0..80).map(|_| encoder.encode(&[0.0; 960])));
    assert!(
        packets.iter().any(|p| p.len() == 1),
        "native DTX did not emit a TOC-only packet"
    );
    for rate in RATES {
        for channels in [1, 2] {
            let samples = rate as usize / 50;
            let mut owned = Decoder::with_sample_rate(channels, rate).unwrap();
            let mut native = NativeDecoder::new(rate, channels);
            let mut a = vec![0.0; samples * channels];
            let mut b = a.clone();
            for (index, packet) in packets.iter().enumerate() {
                if index == 6 {
                    for _ in 0..25 {
                        owned.conceal(samples, &mut a).unwrap();
                        native.decode(None, samples, false, &mut b);
                        finite(&a, "native burst loss");
                        assert_eq!(owned.final_range(), 0);
                    }
                }
                owned.decode(packet, &mut a).unwrap();
                native.decode(Some(packet), samples, false, &mut b);
                finite(&a, "native DTX");
                if packet.len() > 2 {
                    assert_eq!(owned.final_range(), native.final_range());
                }
            }
            owned.reset();
            native.reset();
            owned.decode(&packets[0], &mut a).unwrap();
            native.decode(Some(&packets[0]), samples, false, &mut b);
            assert_eq!(owned.final_range(), native.final_range());
            let first = a.clone();
            let range = owned.final_range();
            assert_eq!(
                owned.decode(&packets[0], &mut a[..samples * channels - 1]),
                Err(DecodeError::Output)
            );
            assert!(owned.decode(&[], &mut a).is_err());
            assert_eq!(
                owned.conceal(samples - 1, &mut a),
                Err(DecodeError::Argument)
            );
            assert_eq!(owned.final_range(), range);
            assert_eq!(a, first);
            owned.reset();
            owned.decode(&packets[0], &mut a).unwrap();
            assert_eq!(a, first);
            owned.reset();
            owned.conceal(samples, &mut a).unwrap();
            assert!(a.iter().all(|&v| v == 0.0));
        }
    }
}

#[test]
fn mode_and_coded_channel_changes_preserve_fixed_output_layout() {
    let settings = [
        (1, 2048, 1101, 12_000),
        (2, 2048, 1103, 48_000),
        (1, 2048, 1105, 24_000),
        (2, 2051, 1105, 64_000),
    ];
    let mut packets = Vec::new();
    for (channels, app, bandwidth, bitrate) in
        settings.into_iter().chain(settings.into_iter().rev())
    {
        let mut encoder = Encoder::new(channels, app, bandwidth, bitrate);
        encoder.set(SIGNAL, 3001);
        for frame in 0..3 {
            packets.push(encoder.encode(&voice(960, channels, frame * 960)));
        }
    }
    assert!(
        packets
            .iter()
            .any(|p| Packet::parse(p).unwrap().toc().channels() == 1)
    );
    assert!(
        packets
            .iter()
            .any(|p| Packet::parse(p).unwrap().toc().channels() == 2)
    );
    for rate in RATES {
        for channels in [1, 2] {
            let samples = rate as usize / 50;
            let mut owned = Decoder::with_sample_rate(channels, rate).unwrap();
            let mut native = NativeDecoder::new(rate, channels);
            let mut a = vec![0.0; samples * channels];
            let mut b = a.clone();
            for packet in &packets {
                assert_eq!(owned.decode(packet, &mut a).unwrap(), samples);
                assert_eq!(native.decode(Some(packet), samples, false, &mut b), samples);
                assert_eq!(owned.final_range(), native.final_range());
                finite(&a, "mode/coded channel change");
            }
        }
    }
}

#[test]
fn ambisonics_projection_signed_column_major_matrix_matches_native() {
    let mapping = ProjectionMapping::new(
        3,
        1,
        4,
        vec![
            16384, 8192, 0, -16384, -8192, 16384, 0, 0, 0, 4096, 0, -32768, 32767, 0, 0, 8192,
        ],
    )
    .unwrap();
    let packets = multistream_celt_packets(5);
    for rate in RATES {
        let samples = rate as usize / 50;
        let mut owned = ProjectionDecoder::with_sample_rate(mapping.clone(), rate).unwrap();
        let mut native = NativeDecoder::projected(rate, &mapping);
        let channels = mapping.channels();
        let mut a = vec![0.0; samples * channels + 3];
        let mut b = vec![0.0; samples * channels];
        for packet in &packets {
            a[samples * channels..].fill(123.0);
            assert_eq!(owned.decode(packet, &mut a).unwrap(), samples);
            assert_eq!(native.decode(Some(packet), samples, false, &mut b), samples);
            assert_eq!(owned.final_range(), native.final_range());
            let maximum = a[..samples * channels]
                .iter()
                .zip(&b)
                .map(|(a, b)| (a - b).abs())
                .fold(0.0, f32::max);
            if maximum >= 2e-4 {
                let mut coded = NativeDecoder::mapped(rate, *mapping.coded_layout());
                let mut coded_pcm = vec![0.0; samples * mapping.coded_layout().decoded_channels()];
                coded.decode(Some(packet), samples, false, &mut coded_pcm);
                let directory = std::env::temp_dir().join("snacka-opus-projection-oracle");
                std::fs::create_dir_all(&directory).unwrap();
                for (name, pcm) in [
                    ("native-projection.f32", &b),
                    ("owned-projection.f32", &a),
                    ("native-coded.f32", &coded_pcm),
                ] {
                    std::fs::write(
                        directory.join(name),
                        pcm.iter()
                            .flat_map(|value| value.to_le_bytes())
                            .collect::<Vec<_>>(),
                    )
                    .unwrap();
                }
            }
            same_celt(&a[..samples * channels], &b, "family-3 matrix projection");
            assert_eq!(&a[samples * channels..], &[123.0; 3]);
            assert!(
                a[..samples * channels]
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .all(|row| row[2] == 0.0)
            );
        }
        let range = owned.final_range();
        let previous = a.clone();
        assert!(
            owned
                .decode(&packets[0], &mut a[..samples * channels - 1])
                .is_err()
        );
        assert!(owned.conceal(usize::MAX, &mut a).is_err());
        assert_eq!(owned.final_range(), range);
        assert_eq!(a, previous);
        owned.conceal(samples, &mut a).unwrap();
        finite(&a[..samples * channels], "projected concealment");
        assert_eq!(owned.final_range(), 0);
        assert_eq!(
            owned.recover(&packets[0], samples, &mut a).unwrap(),
            Recovery::Concealed
        );
        owned.reset();
        native.reset();
        owned.decode(&packets[0], &mut a).unwrap();
        native.decode(Some(&packets[0]), samples, false, &mut b);
        same_celt(&a[..samples * channels], &b, "reset family-3 projection");
    }
}

#[test]
fn rectangular_ambisonics_projection_matches_native_coded_channels() {
    // RFC 8486 explicitly permits C != K. libopus 1.6.1's projection entry
    // point rejects C > K and drops coded channels beyond C when C < K. Use
    // its public identity multistream output plus independent f64 matrix math
    // for both rectangular geometries; square projection has its own oracle.
    let packets = multistream_celt_packets(5);
    let matrices = [
        ProjectionMapping::new(
            3,
            1,
            3,
            vec![
                16384, 0, -16384, -8192, 0, 16384, 0, 0, -32768, 32767, 0, 8192,
            ],
        )
        .unwrap(),
        ProjectionMapping::new(
            3,
            1,
            6,
            vec![
                16384, 8192, 0, -16384, 0, 0, -8192, 16384, 0, 0, 8192, 0, 0, 4096, 0, 16384,
                -32768, 0, 32767, 0, 0, 8192, 0, 16384,
            ],
        )
        .unwrap(),
    ];
    for mapping in matrices {
        for rate in RATES {
            let samples = rate as usize / 50;
            let channels = mapping.channels();
            let mut owned = ProjectionDecoder::with_sample_rate(mapping.clone(), rate).unwrap();
            let mut native = NativeDecoder::mapped(rate, *mapping.coded_layout());
            let mut a = vec![0.0; samples * channels];
            let mut coded = vec![0.0; samples * 4];
            for packet in &packets {
                assert_eq!(owned.decode(packet, &mut a).unwrap(), samples);
                assert_eq!(
                    native.decode(Some(packet), samples, false, &mut coded),
                    samples
                );
                assert_eq!(owned.final_range(), native.final_range());
                let mut expected = Vec::with_capacity(samples * channels);
                for frame in coded.as_chunks::<4>().0 {
                    for row in 0..channels {
                        expected.push(
                            (0..4)
                                .map(|column| {
                                    f64::from(frame[column])
                                        * f64::from(mapping.matrix()[column * channels + row])
                                        / 32768.0
                                })
                                .sum::<f64>() as f32,
                        );
                    }
                }
                same_celt(&a, &expected, "rectangular family-3 matrix");
            }
        }
    }
}

fn transition_metadata(packet: &[u8]) -> Vec<Option<(bool, usize)>> {
    use snacka::opus::{Bandwidth, silk};
    let packet = Packet::parse(packet).unwrap();
    if packet.toc().mode() == Mode::Celt {
        return vec![None; packet.frame_count()];
    }
    let samples = packet.frame_samples(48_000).unwrap();
    let frame = silk::Frame {
        bandwidth: match packet.toc().bandwidth() {
            Bandwidth::Narrowband => silk::Bandwidth::Narrow,
            Bandwidth::Mediumband => silk::Bandwidth::Medium,
            _ => silk::Bandwidth::Wide,
        },
        duration: if samples == 480 {
            silk::FrameDuration::Ms10
        } else {
            silk::FrameDuration::Ms20
        },
    };
    let duration = match samples {
        480 => silk::PacketDuration::Ms10,
        960 => silk::PacketDuration::Ms20,
        1920 => silk::PacketDuration::Ms40,
        2880 => silk::PacketDuration::Ms60,
        _ => panic!("invalid LP duration"),
    };
    let mut silk = silk::SilkDecoder::new();
    let mut pcm = [0.0; 1920];
    packet
        .frames()
        .map(|bytes| {
            let mut entropy = snacka::entropy::Decoder::new(bytes);
            silk.decode(
                &mut entropy,
                frame,
                duration,
                packet.toc().channels(),
                &mut pcm,
            )
            .unwrap();
            let remaining = (bytes.len() as u64 * 8).saturating_sub(entropy.tell());
            let present = if packet.toc().mode() == Mode::Hybrid {
                remaining >= 37 && entropy.bit(12).unwrap()
            } else {
                remaining >= 17
            };
            if present {
                let beginning = entropy.bit(1).unwrap();
                let length = if packet.toc().mode() == Mode::Hybrid {
                    entropy.uniform(256).unwrap() as usize + 2
                } else {
                    bytes.len() - entropy.tell().div_ceil(8) as usize
                };
                assert!(length >= 2 && length < bytes.len());
                Some((beginning, length))
            } else {
                None
            }
        })
        .collect()
}

fn transition_directions(packet: &[u8]) -> Vec<Option<bool>> {
    transition_metadata(packet)
        .into_iter()
        .map(|value| value.map(|(direction, _)| direction))
        .collect()
}

#[test]
fn owned_beginning_and_prepared_ending_transitions_decode_natively() {
    use snacka::opus::{
        Application, Bandwidth, Encoder as OwnedEncoder, EncoderConfig, FrameDuration, RateControl,
    };
    let mut checked = 0;
    for channels in [1, 2] {
        for duration in [
            FrameDuration::Ms10,
            FrameDuration::Ms20,
            FrameDuration::Ms40,
            FrameDuration::Ms60,
        ] {
            for bandwidth in [
                Bandwidth::Narrowband,
                Bandwidth::Mediumband,
                Bandwidth::Wideband,
                Bandwidth::Superwideband,
                Bandwidth::Fullband,
            ] {
                for policy in [RateControl::Variable, RateControl::Constant] {
                    let celt = EncoderConfig::new(
                        64_000,
                        channels,
                        FrameDuration::Ms20,
                        Bandwidth::Fullband,
                        Application::LowDelay,
                    )
                    .unwrap()
                    .with_rate_control(policy);
                    let lp = EncoderConfig::new(
                        64_000,
                        channels,
                        duration,
                        bandwidth,
                        Application::Voice,
                    )
                    .unwrap()
                    .with_rate_control(policy);
                    let mut encoder = OwnedEncoder::new(celt).unwrap();
                    let mut buffer = vec![0; lp.max_packet_bytes().max(celt.max_packet_bytes())];
                    let mut packets = Vec::new();
                    let mut start = 0;
                    for _ in 0..3 {
                        let length = encoder
                            .encode(&voice(960, channels, start), &mut buffer)
                            .unwrap();
                        packets.push((buffer[..length].to_vec(), encoder.final_range(), None));
                        start += 960;
                    }
                    encoder.reconfigure(lp).unwrap();
                    for index in 0..3 {
                        let length = encoder
                            .encode(&voice(duration.samples(), channels, start), &mut buffer)
                            .unwrap();
                        let flags = transition_directions(&buffer[..length]);
                        assert_eq!(flags[0], if index == 0 { Some(true) } else { None });
                        assert!(flags[1..].iter().all(Option::is_none));
                        packets.push((
                            buffer[..length].to_vec(),
                            encoder.final_range(),
                            if index == 0 { Some(true) } else { None },
                        ));
                        start += duration.samples();
                    }
                    encoder.prepare_reconfigure(celt).unwrap();
                    assert_eq!(encoder.config(), lp);
                    let length = encoder
                        .encode(&voice(duration.samples(), channels, start), &mut buffer)
                        .unwrap();
                    let flags = transition_directions(&buffer[..length]);
                    assert_eq!(flags.last().copied().unwrap(), Some(false));
                    assert!(flags[..flags.len() - 1].iter().all(Option::is_none));
                    packets.push((
                        buffer[..length].to_vec(),
                        encoder.final_range(),
                        Some(false),
                    ));
                    start += duration.samples();
                    assert_eq!(encoder.config(), celt);
                    for _ in 0..3 {
                        let length = encoder
                            .encode(&voice(960, channels, start), &mut buffer)
                            .unwrap();
                        packets.push((buffer[..length].to_vec(), encoder.final_range(), None));
                        start += 960;
                    }
                    for rate in RATES {
                        let mut owned = Decoder::with_sample_rate(channels, rate).unwrap();
                        let mut native = NativeDecoder::new(rate, channels);
                        let mut a = vec![0.0; rate as usize * 120 / 1000 * channels];
                        let mut b = a.clone();
                        for (index, (packet, interval, position)) in packets.iter().enumerate() {
                            let samples = Packet::parse(packet).unwrap().samples(rate).unwrap();
                            let context = format!(
                                "transition rate{rate} channels{channels} duration{duration:?} bandwidth{bandwidth:?} policy{policy:?} index{index} position{position:?}"
                            );
                            assert_eq!(owned.decode(packet, &mut a).unwrap(), samples, "{context}");
                            assert_eq!(
                                native.decode(Some(packet), samples, false, &mut b),
                                samples
                            );
                            assert_eq!(owned.final_range(), *interval, "owned range {context}");
                            assert_eq!(native.final_range(), *interval, "native range {context}");
                            finite(&a[..samples * channels], &context);
                            finite(&b[..samples * channels], &context);
                            if rate == 48_000 && *position == Some(true) {
                                same_celt(&a[..120 * channels], &b[..120 * channels], &context);
                            }
                            checked += 1;
                        }
                    }
                }
            }
        }
    }
    assert_eq!(checked, 4_000);
}

#[test]
fn queued_silk_bandwidth_transitions_preserve_native_ranges_and_directions() {
    use snacka::opus::{
        Application, Bandwidth, Encoder as OwnedEncoder, EncoderConfig, FrameDuration,
    };
    let cases = [
        (
            Bandwidth::Narrowband,
            Bandwidth::Mediumband,
            Some(false),
            Some(true),
        ),
        (
            Bandwidth::Mediumband,
            Bandwidth::Wideband,
            Some(false),
            Some(true),
        ),
        (
            Bandwidth::Narrowband,
            Bandwidth::Fullband,
            Some(false),
            None,
        ),
        (
            Bandwidth::Fullband,
            Bandwidth::Mediumband,
            Some(false),
            Some(true),
        ),
        (Bandwidth::Wideband, Bandwidth::Fullband, None, None),
        (Bandwidth::Fullband, Bandwidth::Wideband, None, None),
    ];
    for channels in [1, 2] {
        for (old_band, new_band, old_direction, new_direction) in cases {
            let old = EncoderConfig::new(
                64_000,
                channels,
                FrameDuration::Ms20,
                old_band,
                Application::Voice,
            )
            .unwrap();
            let new = EncoderConfig::new(
                64_000,
                channels,
                FrameDuration::Ms20,
                new_band,
                Application::Voice,
            )
            .unwrap();
            let mut encoder = OwnedEncoder::new(old).unwrap();
            let mut buffer = [0; 512];
            let mut packets = Vec::new();
            for index in 0..3 {
                let length = encoder
                    .encode(&voice(960, channels, index * 960), &mut buffer)
                    .unwrap();
                packets.push((buffer[..length].to_vec(), encoder.final_range()));
            }
            encoder.prepare_reconfigure(new).unwrap();
            let length = encoder
                .encode(&voice(960, channels, 3 * 960), &mut buffer)
                .unwrap();
            assert_eq!(transition_directions(&buffer[..length]), [old_direction]);
            packets.push((buffer[..length].to_vec(), encoder.final_range()));
            assert_eq!(encoder.config(), new);
            for index in 4..7 {
                let length = encoder
                    .encode(&voice(960, channels, index * 960), &mut buffer)
                    .unwrap();
                assert_eq!(
                    transition_directions(&buffer[..length]),
                    [if index == 4 { new_direction } else { None }]
                );
                packets.push((buffer[..length].to_vec(), encoder.final_range()));
            }
            for rate in RATES {
                let samples = rate as usize / 50;
                let mut owned = Decoder::with_sample_rate(channels, rate).unwrap();
                let mut native = NativeDecoder::new(rate, channels);
                let mut a = vec![0.0; samples * channels];
                let mut b = a.clone();
                for (index, (packet, interval)) in packets.iter().enumerate() {
                    let context = format!(
                        "queued {old_band:?}->{new_band:?}, rate {rate}, channels {channels}, index {index}"
                    );
                    assert_eq!(owned.decode(packet, &mut a).unwrap(), samples);
                    assert_eq!(native.decode(Some(packet), samples, false, &mut b), samples);
                    assert_eq!(owned.final_range(), *interval, "owned {context}");
                    assert_eq!(native.final_range(), *interval, "native {context}");
                    finite(&a, &context);
                }
            }
        }
    }
}

#[test]
fn all_owned_celt_hybrid_silk_dtx_durations_refresh_and_resume_natively() {
    use snacka::opus::{
        Application, Bandwidth, Encoder as OwnedEncoder, EncoderConfig, FrameDuration,
    };
    let mut configurations = Vec::new();
    for channels in [1, 2] {
        for duration in [
            FrameDuration::Ms2_5,
            FrameDuration::Ms5,
            FrameDuration::Ms10,
            FrameDuration::Ms20,
            FrameDuration::Ms40,
            FrameDuration::Ms60,
        ] {
            configurations.push(
                EncoderConfig::new(
                    64_000,
                    channels,
                    duration,
                    Bandwidth::Fullband,
                    Application::LowDelay,
                )
                .unwrap()
                .with_dtx(true)
                .unwrap(),
            );
            if duration.samples() >= 480 {
                configurations.push(
                    EncoderConfig::new(
                        64_000,
                        channels,
                        duration,
                        Bandwidth::Fullband,
                        Application::Voice,
                    )
                    .unwrap()
                    .with_dtx(true)
                    .unwrap(),
                );
                configurations.push(
                    EncoderConfig::new(
                        64_000,
                        channels,
                        duration,
                        Bandwidth::Wideband,
                        Application::Voice,
                    )
                    .unwrap()
                    .with_dtx(true)
                    .unwrap(),
                );
            }
        }
    }
    assert_eq!(configurations.len(), 28);
    for config in configurations {
        let channels = config.channels();
        let samples = config.duration().samples();
        let mut encoder = OwnedEncoder::new(config).unwrap();
        let mut buffer = vec![0; config.max_packet_bytes()];
        let mut packets = Vec::new();
        for index in 0..4 {
            let length = encoder
                .encode(&voice(samples, channels, index * samples), &mut buffer)
                .unwrap();
            packets.push((
                buffer[..length].to_vec(),
                encoder.final_range(),
                encoder.was_dtx(),
            ));
        }
        let quiet_frames = 57_600usize.div_ceil(samples);
        for _ in 0..quiet_frames {
            let length = encoder
                .encode(&vec![0.0; samples * channels], &mut buffer)
                .unwrap();
            packets.push((
                buffer[..length].to_vec(),
                encoder.final_range(),
                encoder.was_dtx(),
            ));
        }
        let length = encoder
            .encode(
                &voice(samples, channels, (4 + quiet_frames) * samples),
                &mut buffer,
            )
            .unwrap();
        assert!(!encoder.was_dtx());
        packets.push((buffer[..length].to_vec(), encoder.final_range(), false));
        assert!(packets.iter().any(|packet| packet.2));
        let mut suppressed_samples = 0;
        for (packet, interval, suppressed) in &packets {
            if *suppressed {
                assert_eq!(*interval, 0);
                assert!(
                    Packet::parse(packet)
                        .unwrap()
                        .frames()
                        .all(<[u8]>::is_empty)
                );
                suppressed_samples += samples;
                assert!(
                    suppressed_samples <= 19_200,
                    "refresh gap {:?}",
                    config.duration()
                );
            } else {
                suppressed_samples = 0;
            }
        }
        for rate in RATES {
            let count = samples * rate as usize / 48_000;
            let mut owned = Decoder::with_sample_rate(channels, rate).unwrap();
            let mut native = NativeDecoder::new(rate, channels);
            let mut a = vec![0.0; count * channels];
            let mut b = a.clone();
            for (index, (packet, interval, suppressed)) in packets.iter().enumerate() {
                let context = format!(
                    "DTX mode{:?} duration{:?} channels{channels} rate{rate} index{index} suppressed{suppressed}",
                    config.mode(),
                    config.duration()
                );
                assert_eq!(owned.decode(packet, &mut a).unwrap(), count);
                assert_eq!(native.decode(Some(packet), count, false, &mut b), count);
                assert_eq!(owned.final_range(), *interval, "owned {context}");
                assert_eq!(native.final_range(), *interval, "native {context}");
                finite(&a, &context);
                finite(&b, &context);
            }
        }
    }
}

#[test]
fn owned_transition_payload_minimum_and_hybrid_maximum_decode_natively() {
    use snacka::opus::{
        Application, Bandwidth, Encoder as OwnedEncoder, EncoderConfig, FrameDuration,
    };
    for (channels, bitrate, duration, bandwidth, expected_size) in [
        (1, 9_000, FrameDuration::Ms10, Bandwidth::Narrowband, 2),
        (2, 510_000, FrameDuration::Ms20, Bandwidth::Fullband, 257),
    ] {
        let celt = EncoderConfig::new(
            64_000,
            channels,
            FrameDuration::Ms20,
            Bandwidth::Fullband,
            Application::LowDelay,
        )
        .unwrap();
        let lp =
            EncoderConfig::new(bitrate, channels, duration, bandwidth, Application::Voice).unwrap();
        let mut encoder = OwnedEncoder::new(celt).unwrap();
        let mut buffer = vec![0; lp.max_packet_bytes().max(celt.max_packet_bytes())];
        let mut packets = Vec::new();
        for _ in 0..2 {
            let length = encoder
                .encode(&vec![0.0; 960 * channels], &mut buffer)
                .unwrap();
            packets.push((buffer[..length].to_vec(), encoder.final_range()));
        }
        encoder.reconfigure(lp).unwrap();
        let length = encoder
            .encode(&vec![0.0; duration.samples() * channels], &mut buffer)
            .unwrap();
        assert_eq!(
            transition_metadata(&buffer[..length]),
            [Some((true, expected_size))]
        );
        packets.push((buffer[..length].to_vec(), encoder.final_range()));
        for rate in RATES {
            let mut owned = Decoder::with_sample_rate(channels, rate).unwrap();
            let mut native = NativeDecoder::new(rate, channels);
            let mut a = vec![0.0; rate as usize * 120 / 1000 * channels];
            let mut b = a.clone();
            for (packet, interval) in &packets {
                let count = Packet::parse(packet).unwrap().samples(rate).unwrap();
                assert_eq!(owned.decode(packet, &mut a).unwrap(), count);
                assert_eq!(native.decode(Some(packet), count, false, &mut b), count);
                assert_eq!(owned.final_range(), *interval);
                assert_eq!(native.final_range(), *interval);
                finite(&a[..count * channels], "transition payload endpoint");
                finite(&b[..count * channels], "native transition payload endpoint");
            }
        }
    }
}

fn composed_phase_contract<D: ComposedGainDecoder>(
    own: impl Fn(u32) -> D,
    reference: impl Fn(u32) -> NativeDecoder,
    streams: usize,
    coupled: usize,
) {
    let frame = include_bytes!("../src/opus/testdata/stereo-phase.opus");
    let mut packet = vec![0; 256];
    let length = MultistreamPacket::write(&vec![frame.as_slice(); streams], &mut packet).unwrap();
    packet.truncate(length);
    for rate in RATES {
        let mut owned = std::array::from_fn::<_, 3, _>(|_| own(rate));
        let mut native = std::array::from_fn::<_, 3, _>(|_| reference(rate));
        let policies = [None, Some(false), Some(true)];
        for (index, policy) in policies.into_iter().enumerate() {
            if let Some(enabled) = policy {
                owned[index].set_phase_inversion(enabled);
                native[index].set_phase_inversion(enabled);
            }
        }
        let samples = rate as usize / 400;
        let count = samples * native[0].channels;
        let sentinel = f32::from_bits(0x7fc0_2345);
        let mut a = std::array::from_fn::<_, 3, _>(|_| vec![sentinel; count + 7]);
        let mut b = a.clone();
        for step in 0..10 {
            for (index, policy) in policies.into_iter().enumerate() {
                if step == 9 {
                    owned[index].reset();
                    native[index].reset();
                }
                assert_eq!(
                    owned[index].phase_inversion(),
                    policy.unwrap_or(coupled != 0)
                );
                assert_eq!(
                    native[index].phase_inversion(),
                    owned[index].phase_inversion()
                );
                for stream in 0..streams {
                    assert_eq!(
                        native[index].stream_phase_inversion(stream),
                        policy.unwrap_or(stream < coupled)
                    );
                }
                a[index].fill(sentinel);
                b[index].fill(sentinel);
                match step {
                    6 => assert_eq!(
                        owned[index].conceal(samples, &mut a[index]).unwrap(),
                        samples
                    ),
                    7 => assert_eq!(
                        owned[index]
                            .recover(&packet, samples, &mut a[index])
                            .unwrap(),
                        Recovery::Concealed
                    ),
                    _ => assert_eq!(
                        owned[index].decode(&packet, &mut a[index]).unwrap(),
                        samples
                    ),
                }
                assert_eq!(
                    native[index].decode(
                        (step != 6).then_some(packet.as_slice()),
                        samples,
                        step == 7,
                        &mut b[index]
                    ),
                    samples
                );
                assert!(
                    a[index][..count]
                        .iter()
                        .chain(&b[index][..count])
                        .all(|v| v.is_finite())
                );
                assert!(
                    a[index][count..]
                        .iter()
                        .chain(&b[index][count..])
                        .all(|v| v.to_bits() == sentinel.to_bits())
                );
                // Native and owned PLC differ. Compare the existing normal CELT
                // witness only before loss and after a full reset.
                if step < 6 || step == 9 {
                    same_celt(
                        &a[index][..count],
                        &b[index][..count],
                        "composed phase policy",
                    );
                }
            }
            assert!(
                owned
                    .iter()
                    .all(|d| d.final_range() == owned[0].final_range())
            );
            assert!(
                native
                    .iter()
                    .all(|d| d.final_range() == native[0].final_range())
            );
            if rate == 12_000 && step == 0 {
                let difference = |x: &[f32], y: &[f32]| {
                    x.iter()
                        .zip(y)
                        .map(|(x, y)| (x - y).abs())
                        .fold(0.0, f32::max)
                };
                assert!(difference(&a[1][..count], &a[2][..count]) > 0.002);
                assert!(difference(&b[1][..count], &b[2][..count]) > 0.002);
                if coupled != 0 && streams > coupled {
                    // First-stream getter is true initially, yet setting true
                    // changes the initially-disabled mono stream policies.
                    assert!(difference(&a[0][..count], &a[2][..count]) > 0.002);
                    assert!(difference(&b[0][..count], &b[2][..count]) > 0.002);
                }
            }
        }
    }
}

#[test]
fn mapped_and_projected_phase_controls_cover_mixed_defaults_and_real_downmix() {
    let mono = ChannelMapping::mono_stereo(1).unwrap();
    let mixed = ChannelMapping::new(3, 1, &[0, 1, 2, 255, 0]).unwrap();
    composed_phase_contract(
        |rate| MultistreamDecoder::with_sample_rate(mono, rate).unwrap(),
        |rate| NativeDecoder::mapped(rate, mono),
        1,
        0,
    );
    composed_phase_contract(
        |rate| MultistreamDecoder::with_sample_rate(mixed, rate).unwrap(),
        |rate| NativeDecoder::mapped(rate, mixed),
        3,
        1,
    );
    let mapping = ProjectionMapping::new(
        3,
        1,
        4,
        vec![
            16384, 16384, 0, 0, 16384, -16384, 0, 0, 0, 0, 32767, 0, 0, 0, 0, 32767,
        ],
    )
    .unwrap();
    composed_phase_contract(
        |rate| ProjectionDecoder::with_sample_rate(mapping.clone(), rate).unwrap(),
        |rate| NativeDecoder::projected(rate, &mapping),
        3,
        1,
    );
}
