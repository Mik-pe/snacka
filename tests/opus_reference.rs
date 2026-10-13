// SPDX-License-Identifier: MIT
#![cfg(all(feature = "opus", feature = "reference-tests"))]
//! External libopus interoperability oracle. This test executable alone links
//! libopus; Snacka's `opus` feature has no foreign runtime or codec dependency.
//!
//! Run with `cargo test --release --features opus,reference-tests --test
//! opus_reference`. A system libopus installation must be on the linker path.

use std::{ffi::c_void, ptr::NonNull};

use snacka::opus::{Bandwidth, CeltConfig, CeltDecoder, CeltEncoder, CeltFrameDuration};

const RATE: i32 = 48_000;
const APPLICATION_RESTRICTED_LOWDELAY: i32 = 2051;
const SET_BITRATE: i32 = 4002;
const SET_MAX_BANDWIDTH: i32 = 4004;
const SET_VBR: i32 = 4006;
const SET_BANDWIDTH: i32 = 4008;
const SET_COMPLEXITY: i32 = 4010;
const SET_FORCE_CHANNELS: i32 = 4022;
const GET_FINAL_RANGE: i32 = 4031;
const MAX_PACKET: usize = 1_276;
const MAX_SAMPLES: usize = 960 * 2;

// These declarations follow the installed public opus.h / opus_defines.h API.
// No private structs, internal opcodes, or reference codec sources are used.
#[link(name = "opus")]
unsafe extern "C" {
    fn opus_encoder_create(
        rate: i32,
        channels: i32,
        application: i32,
        error: *mut i32,
    ) -> *mut c_void;
    fn opus_encoder_destroy(encoder: *mut c_void);
    fn opus_encoder_ctl(encoder: *mut c_void, request: i32, ...) -> i32;
    fn opus_encode_float(
        encoder: *mut c_void,
        input: *const f32,
        samples: i32,
        packet: *mut u8,
        capacity: i32,
    ) -> i32;
    fn opus_decoder_create(rate: i32, channels: i32, error: *mut i32) -> *mut c_void;
    fn opus_decoder_destroy(decoder: *mut c_void);
    fn opus_decoder_ctl(decoder: *mut c_void, request: i32, ...) -> i32;
    fn opus_decode_float(
        decoder: *mut c_void,
        packet: *const u8,
        length: i32,
        output: *mut f32,
        samples: i32,
        fec: i32,
    ) -> i32;
}

struct ReferenceEncoder {
    pointer: NonNull<c_void>,
    channels: usize,
}

impl ReferenceEncoder {
    fn new(bitrate: u32, channels: usize, bandwidth: Bandwidth) -> Self {
        let mut error = 0;
        // SAFETY: Parameters specify a supported sample rate/channel count and
        // application. Writable error storage remains live during the call.
        let pointer = unsafe {
            opus_encoder_create(
                RATE,
                channels as i32,
                APPLICATION_RESTRICTED_LOWDELAY,
                &mut error,
            )
        };
        assert_eq!(error, 0, "libopus encoder creation");
        let reference = Self {
            pointer: NonNull::new(pointer).expect("libopus returned an encoder"),
            channels,
        };
        reference.set(SET_BITRATE, bitrate as i32);
        reference.set(SET_VBR, 0);
        reference.set(SET_COMPLEXITY, 10);
        reference.set(SET_FORCE_CHANNELS, channels as i32);
        reference.set(SET_MAX_BANDWIDTH, reference_bandwidth(bandwidth));
        reference.set(SET_BANDWIDTH, reference_bandwidth(bandwidth));
        reference
    }

    fn set(&self, request: i32, value: i32) {
        // SAFETY: This test calls only documented controls whose variadic value
        // type is an int. The owned encoder is live and not concurrently used.
        let status = unsafe { opus_encoder_ctl(self.pointer.as_ptr(), request, value) };
        assert_eq!(status, 0, "libopus encoder control {request}");
    }

    fn encode(&mut self, input: &[f32], output: &mut [u8]) -> usize {
        assert_eq!(input.len() % self.channels, 0);
        assert!(output.len() <= i32::MAX as usize);
        // SAFETY: Input contains the advertised interleaved frame, the output
        // slice is writable up to capacity, and the encoder is exclusively used.
        let length = unsafe {
            opus_encode_float(
                self.pointer.as_ptr(),
                input.as_ptr(),
                (input.len() / self.channels) as i32,
                output.as_mut_ptr(),
                output.len() as i32,
            )
        };
        assert!(length > 0, "libopus encode status {length}");
        length as usize
    }

    fn final_range(&self) -> u32 {
        let mut range = 0;
        // SAFETY: GET_FINAL_RANGE's documented variadic argument is an opus_uint32
        // pointer; the owned encoder and writable output remain live.
        let status = unsafe {
            opus_encoder_ctl(
                self.pointer.as_ptr(),
                GET_FINAL_RANGE,
                &mut range as *mut u32,
            )
        };
        assert_eq!(status, 0);
        range
    }
}

impl Drop for ReferenceEncoder {
    fn drop(&mut self) {
        // SAFETY: The pointer was returned by opus_encoder_create, remains owned
        // solely by this object, and is destroyed exactly once after final use.
        unsafe { opus_encoder_destroy(self.pointer.as_ptr()) };
    }
}

struct ReferenceDecoder {
    pointer: NonNull<c_void>,
    channels: usize,
}

impl ReferenceDecoder {
    fn new(channels: usize) -> Self {
        let mut error = 0;
        // SAFETY: Sample rate/channel count are supported and error is writable.
        let pointer = unsafe { opus_decoder_create(RATE, channels as i32, &mut error) };
        assert_eq!(error, 0, "libopus decoder creation");
        Self {
            pointer: NonNull::new(pointer).expect("libopus returned a decoder"),
            channels,
        }
    }

    fn decode(&mut self, packet: &[u8], output: &mut [f32]) -> usize {
        assert!(!packet.is_empty());
        assert_eq!(output.len() % self.channels, 0);
        // SAFETY: Packet's complete initialized prefix and writable interleaved
        // output have the advertised lengths. Decoder is owned and exclusive.
        let samples = unsafe {
            opus_decode_float(
                self.pointer.as_ptr(),
                packet.as_ptr(),
                packet.len() as i32,
                output.as_mut_ptr(),
                (output.len() / self.channels) as i32,
                0,
            )
        };
        assert!(samples > 0, "libopus decode status {samples}");
        samples as usize
    }

    fn final_range(&self) -> u32 {
        let mut range = 0;
        // SAFETY: The documented request writes one opus_uint32 to the supplied
        // live output pointer; the owned decoder remains valid during the call.
        let status = unsafe {
            opus_decoder_ctl(
                self.pointer.as_ptr(),
                GET_FINAL_RANGE,
                &mut range as *mut u32,
            )
        };
        assert_eq!(status, 0);
        range
    }
}

impl Drop for ReferenceDecoder {
    fn drop(&mut self) {
        // SAFETY: This object owns one live decoder returned by libopus and no
        // other code uses it after this destructor runs.
        unsafe { opus_decoder_destroy(self.pointer.as_ptr()) };
    }
}

fn reference_bandwidth(bandwidth: Bandwidth) -> i32 {
    match bandwidth {
        Bandwidth::Narrowband => 1101,
        Bandwidth::Mediumband => 1102,
        Bandwidth::Wideband => 1103,
        Bandwidth::Superwideband => 1104,
        Bandwidth::Fullband => 1105,
    }
}

fn fill_audio(output: &mut [f32], channels: usize, frame: usize) {
    let mut seed = 0x4345_4c54_6716_0001u64 ^ frame as u64;
    let samples = output.len() / channels;
    for (sample, value) in output.iter_mut().enumerate() {
        let channel = sample % channels;
        let time = (frame * samples + sample / channels) as f32 / RATE as f32;
        *value = match frame {
            0 | 1 | 8 => 0.0,
            2 | 3 | 9 => {
                let frequency = if channel == 0 { 237.0 } else { 523.0 };
                0.2 * (std::f32::consts::TAU * frequency * time).sin()
                    + 0.05 * (std::f32::consts::TAU * (frequency * 3.0 + 11.0) * time).sin()
            }
            4 | 5 => {
                seed ^= seed << 13;
                seed ^= seed >> 7;
                seed ^= seed << 17;
                ((seed >> 32) as i32 as f32 / i32::MAX as f32) * 0.25
            }
            _ => {
                let index = sample / channels;
                if (12..20).contains(&index) {
                    0.25 * (-0.4 * (index - 12) as f32).exp()
                } else {
                    0.0
                }
            }
        };
    }
}

fn assert_pcm_matches(native: &[f32], reference: &[f32], context: &str) {
    assert_eq!(native.len(), reference.len());
    assert!(
        native.iter().all(|sample| sample.is_finite()),
        "nonfinite native PCM: {context}"
    );
    assert!(
        reference.iter().all(|sample| sample.is_finite()),
        "nonfinite libopus PCM: {context}"
    );
    let power: f64 = reference
        .iter()
        .map(|&value| f64::from(value).powi(2))
        .sum();
    let error: f64 = native
        .iter()
        .zip(reference)
        .map(|(&left, &right)| (f64::from(left) - f64::from(right)).powi(2))
        .sum();
    let reference_rms = (power / reference.len() as f64).sqrt();
    let error_rms = (error / native.len() as f64).sqrt();
    let tolerance = (reference_rms * 1e-4).max(1e-5);
    assert!(
        error_rms <= tolerance,
        "PCM mismatch: {context}: reference_rms={reference_rms:.9}, error_rms={error_rms:.9}, tolerance={tolerance:.9}"
    );
}

const DURATIONS: [CeltFrameDuration; 4] = [
    CeltFrameDuration::Ms2_5,
    CeltFrameDuration::Ms5,
    CeltFrameDuration::Ms10,
    CeltFrameDuration::Ms20,
];
const BANDWIDTHS: [Bandwidth; 4] = [
    Bandwidth::Narrowband,
    Bandwidth::Wideband,
    Bandwidth::Superwideband,
    Bandwidth::Fullband,
];
const BITRATES: [u32; 5] = [12_000, 24_000, 32_000, 64_000, 96_000];

#[test]
fn native_celt_packets_match_libopus_range_and_pcm() {
    for duration in DURATIONS {
        for bandwidth in BANDWIDTHS {
            for channels in [1, 2] {
                for bitrate in BITRATES {
                    let config = CeltConfig::new(bitrate, duration, channels, bandwidth).unwrap();
                    let samples = duration.samples();
                    let length = samples * channels;
                    let mut encoder = CeltEncoder::new(config);
                    let mut native = CeltDecoder::new(channels).unwrap();
                    let mut reference = ReferenceDecoder::new(channels);
                    let mut input = [0.0; MAX_SAMPLES];
                    let mut native_pcm = [0.0; MAX_SAMPLES];
                    let mut reference_pcm = [0.0; MAX_SAMPLES];
                    let mut packet = [0; MAX_PACKET];
                    for frame in 0..10 {
                        fill_audio(&mut input[..length], channels, frame);
                        let context = format!(
                            "native encoder, {duration:?}, {bandwidth:?}, channels={channels}, bitrate={bitrate}, frame={frame}"
                        );
                        let bytes = encoder
                            .encode(&input[..length], &mut packet)
                            .expect(&context);
                        assert!(
                            packet[0] >> 3 >= 16,
                            "encoder emitted a non-CELT packet: {context}"
                        );
                        assert_eq!(
                            native
                                .decode(&packet[..bytes], &mut native_pcm[..length])
                                .expect(&context),
                            samples,
                            "{context}"
                        );
                        assert_eq!(
                            reference.decode(&packet[..bytes], &mut reference_pcm[..length]),
                            samples,
                            "{context}"
                        );
                        assert_eq!(
                            native.final_range(),
                            encoder.final_range(),
                            "native entropy mismatch: {context}"
                        );
                        assert_eq!(
                            reference.final_range(),
                            encoder.final_range(),
                            "libopus entropy mismatch: {context}"
                        );
                        assert_pcm_matches(
                            &native_pcm[..length],
                            &reference_pcm[..length],
                            &context,
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn libopus_celt_packets_match_native_range_and_pcm() {
    for duration in DURATIONS {
        for bandwidth in BANDWIDTHS {
            for channels in [1, 2] {
                for bitrate in BITRATES {
                    let samples = duration.samples();
                    let length = samples * channels;
                    let mut encoder = ReferenceEncoder::new(bitrate, channels, bandwidth);
                    let mut native = CeltDecoder::new(channels).unwrap();
                    let mut reference = ReferenceDecoder::new(channels);
                    let mut input = [0.0; MAX_SAMPLES];
                    let mut native_pcm = [0.0; MAX_SAMPLES];
                    let mut reference_pcm = [0.0; MAX_SAMPLES];
                    let mut packet = [0; MAX_PACKET];
                    for frame in 0..10 {
                        fill_audio(&mut input[..length], channels, frame);
                        let context = format!(
                            "libopus encoder, {duration:?}, {bandwidth:?}, channels={channels}, bitrate={bitrate}, frame={frame}"
                        );
                        let bytes = encoder.encode(&input[..length], &mut packet);
                        assert!(
                            packet[0] >> 3 >= 16,
                            "oracle emitted a non-CELT packet: {context}"
                        );
                        assert_eq!(
                            native
                                .decode(&packet[..bytes], &mut native_pcm[..length])
                                .expect(&context),
                            samples,
                            "{context}"
                        );
                        assert_eq!(
                            reference.decode(&packet[..bytes], &mut reference_pcm[..length]),
                            samples,
                            "{context}"
                        );
                        assert_eq!(
                            native.final_range(),
                            encoder.final_range(),
                            "native entropy mismatch: {context}"
                        );
                        assert_eq!(
                            reference.final_range(),
                            encoder.final_range(),
                            "libopus entropy mismatch: {context}"
                        );
                        assert_pcm_matches(
                            &native_pcm[..length],
                            &reference_pcm[..length],
                            &context,
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn libopus_celt_duration_changes_preserve_decoder_state() {
    let sequence = [
        CeltFrameDuration::Ms2_5,
        CeltFrameDuration::Ms5,
        CeltFrameDuration::Ms10,
        CeltFrameDuration::Ms20,
        CeltFrameDuration::Ms10,
        CeltFrameDuration::Ms5,
        CeltFrameDuration::Ms2_5,
        CeltFrameDuration::Ms20,
        CeltFrameDuration::Ms2_5,
        CeltFrameDuration::Ms10,
    ];
    for bandwidth in BANDWIDTHS {
        for channels in [1, 2] {
            let mut encoder = ReferenceEncoder::new(64_000, channels, bandwidth);
            let mut native = CeltDecoder::new(channels).unwrap();
            let mut reference = ReferenceDecoder::new(channels);
            let mut input = [0.0; MAX_SAMPLES];
            let mut native_pcm = [0.0; MAX_SAMPLES];
            let mut reference_pcm = [0.0; MAX_SAMPLES];
            let mut packet = [0; MAX_PACKET];
            for (frame, duration) in sequence.into_iter().cycle().take(20).enumerate() {
                let samples = duration.samples();
                let length = samples * channels;
                fill_audio(&mut input[..length], channels, frame % 10);
                let context = format!(
                    "duration change, {duration:?}, {bandwidth:?}, channels={channels}, frame={frame}"
                );
                let bytes = encoder.encode(&input[..length], &mut packet);
                assert_eq!(
                    native
                        .decode(&packet[..bytes], &mut native_pcm[..length])
                        .expect(&context),
                    samples,
                    "{context}"
                );
                assert_eq!(
                    reference.decode(&packet[..bytes], &mut reference_pcm[..length]),
                    samples,
                    "{context}"
                );
                assert_eq!(
                    native.final_range(),
                    encoder.final_range(),
                    "native entropy mismatch: {context}"
                );
                assert_eq!(
                    reference.final_range(),
                    encoder.final_range(),
                    "libopus entropy mismatch: {context}"
                );
                assert_pcm_matches(&native_pcm[..length], &reference_pcm[..length], &context);
            }
        }
    }
}
