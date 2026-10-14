// SPDX-License-Identifier: MIT
#![cfg(all(feature = "opus", feature = "reference-tests"))]
//! Public libopus mobile transport oracle, isolated from production builds.
//! Run with `cargo test --release --features opus,reference-tests --test opus_calls_reference`.

use std::{ffi::c_void, ptr::NonNull};

use snacka::opus::multistream::{ChannelMapping, MultistreamDecoder};
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
