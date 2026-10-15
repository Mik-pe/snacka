// SPDX-License-Identifier: MIT
//! Standalone owned-before/after trace oracle; no reference codec or timing.
//! Build: rustc --edition=2024 -O -C overflow-checks=yes THIS.rs --extern
//! snacka=ROOT/target/release/libsnacka.rlib -L dependency=ROOT/target/release/deps -o PROBE
//! CLI: MODE BW MS CHANNELS SIGNAL [FRAMES=96] [--input RAW.f32]
//!      [--input-channels 1|2] [--bitrate BPS] [--fec off|on|both]
//!      [--reset off|on|both]
//! Signals: voice, noise, transient, silence, clipping, zeros.
//! Raw input is normalized 48 kHz little-endian f32; mono is duplicated for
//! stereo unless --input-channels 2 is supplied. Input wraps when exhausted.
//! Defaults: both FEC configurations for SILK/Hybrid; FEC off for CELT;
//! continuous and reset-half traces. One deterministic JSON object per run.
//! Existing encode refusals are observable trace outcomes: hash the Debug error,
//! public encoder state and complete packet buffer, then retry that logical
//! frame at half amplitude without resetting encoder state. Eight attempts total
//! bound fallback to levels 0..=7. Only successful packets advance the decoders.
//! These fallback traces establish equivalence, not full-amplitude acceptance,
//! encoder throughput or audio quality.

use snacka::opus::{
    Application, Bandwidth, Decoder, Encoder, EncoderConfig, FrameDuration, Mode, Packet,
    RateControl, Recovery,
};
use std::{env, fs};

#[derive(Clone, Copy)]
struct Hash(u64);
impl Hash {
    fn new() -> Self {
        Self(0xcbf2_9ce4_8422_2325)
    }
    fn byte(&mut self, value: u8) {
        self.0 = (self.0 ^ u64::from(value)).wrapping_mul(0x100_0000_01b3);
    }
    fn word(&mut self, value: u64) {
        for byte in value.to_le_bytes() {
            self.byte(byte);
        }
    }
    fn bytes(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            self.byte(byte);
        }
    }
}

struct Trace {
    pcm: Hash,
    ranges: Hash,
    counts: Hash,
    events: usize,
    samples: usize,
    decodes: usize,
    conceals: usize,
    recovery_attempts: usize,
    recovered_fec_packets: usize,
    recovery_fallbacks: usize,
}
impl Trace {
    fn new() -> Self {
        Self {
            pcm: Hash::new(),
            ranges: Hash::new(),
            counts: Hash::new(),
            events: 0,
            samples: 0,
            decodes: 0,
            conceals: 0,
            recovery_attempts: 0,
            recovered_fec_packets: 0,
            recovery_fallbacks: 0,
        }
    }
    fn output(
        &mut self,
        frame: usize,
        action: u64,
        samples: usize,
        range: u32,
        pcm: &[f32],
        channels: usize,
    ) {
        self.events += 1;
        self.samples += samples;
        for hash in [&mut self.pcm, &mut self.ranges, &mut self.counts] {
            hash.word(frame as u64);
            hash.word(action);
        }
        self.ranges.word(u64::from(range));
        self.counts.word(samples as u64);
        for &sample in &pcm[..samples * channels] {
            assert!(
                sample.is_finite(),
                "non-finite or unwritten output at frame {frame}"
            );
            self.pcm.word(u64::from(sample.to_bits()));
        }
        assert!(
            pcm[samples * channels..]
                .iter()
                .all(|v| v.to_bits() == SENTINEL),
            "decoder overwrote trailing output capacity"
        );
    }
    fn reset(&mut self, frame: usize) {
        for hash in [&mut self.pcm, &mut self.ranges, &mut self.counts] {
            hash.word(frame as u64);
            hash.word(9);
        }
    }
    fn json(&self) -> String {
        format!(
            concat!(
                "{{\"pcm_hash\":\"{:016x}\",\"range_hash\":\"{:016x}\",",
                "\"sample_count_hash\":\"{:016x}\",\"events\":{},\"samples_per_channel\":{},",
                "\"decoded_packets\":{},\"concealed_intervals\":{},\"recovery_attempts\":{},",
                "\"recovered_fec_packets\":{},\"recovery_fallbacks\":{}}}"
            ),
            self.pcm.0,
            self.ranges.0,
            self.counts.0,
            self.events,
            self.samples,
            self.decodes,
            self.conceals,
            self.recovery_attempts,
            self.recovered_fec_packets,
            self.recovery_fallbacks
        )
    }
}

const SENTINEL: u32 = 0x7fc0_4200;
fn clear(pcm: &mut [f32]) {
    pcm.fill(f32::from_bits(SENTINEL));
}

fn decode(
    decoder: &mut Decoder,
    trace: &mut Trace,
    packet: &[u8],
    frame: usize,
    expected: usize,
    range: u32,
    pcm: &mut [f32],
    channels: usize,
) {
    clear(pcm);
    let got = decoder.decode(packet, pcm).expect("decode");
    assert_eq!(got, expected, "decode returned another sample count");
    assert_eq!(
        decoder.final_range(),
        range,
        "encoder/decoder range mismatch"
    );
    trace.decodes += 1;
    trace.output(frame, 1, got, decoder.final_range(), pcm, channels);
}

fn conceal(
    decoder: &mut Decoder,
    trace: &mut Trace,
    frame: usize,
    expected: usize,
    pcm: &mut [f32],
    channels: usize,
) {
    clear(pcm);
    let got = decoder.conceal(expected, pcm).expect("conceal");
    assert_eq!(got, expected, "conceal returned another sample count");
    assert_eq!(decoder.final_range(), 0, "PLC entropy interval");
    trace.conceals += 1;
    trace.output(frame, 2, got, decoder.final_range(), pcm, channels);
}

fn run(
    config: EncoderConfig,
    inputs: &[Vec<f32>],
    frames: usize,
    reset_half: bool,
    channels: usize,
    samples: usize,
    budget: usize,
) -> String {
    let mut encoder = Encoder::new(config).expect("encoder config");
    let mut normal = Decoder::new(channels).unwrap();
    let mut plc = Decoder::new(channels).unwrap();
    let mut fec = Decoder::new(channels).unwrap();
    let mut normal_trace = Trace::new();
    let mut plc_trace = Trace::new();
    let mut fec_trace = Trace::new();
    let mut packet_hash = Hash::new();
    let mut encoder_ranges = Hash::new();
    let mut metadata = Hash::new();
    let mut encoder_error_hash = Hash::new();
    let mut packet = vec![0; config.max_packet_bytes()];
    let mut pcm = vec![f32::from_bits(SENTINEL); samples * channels + 8];
    let mut pending: Option<usize> = None;
    let mut epoch = 0;
    let mut fec_packets = 0;
    let mut resets = 0;
    let mut encoder_refusals = 0;
    let mut fallback_levels = [0usize; 8];
    let mut fallback_max_level = 0;
    let mut fallback_level_sum = 0;
    for frame in 0..frames {
        if reset_half && frame == frames / 2 {
            if let Some(lost_frame) = pending.take() {
                conceal(
                    &mut fec,
                    &mut fec_trace,
                    lost_frame,
                    samples,
                    &mut pcm,
                    channels,
                );
            }
            encoder.reset();
            normal.reset();
            plc.reset();
            fec.reset();
            assert_eq!(encoder.final_range(), 0);
            assert!(!encoder.has_fec());
            for decoder in [&normal, &plc, &fec] {
                assert_eq!(decoder.final_range(), 0);
                assert_eq!(decoder.channels(), channels);
                assert_eq!(decoder.sample_rate(), 48_000);
            }
            for trace in [&mut normal_trace, &mut plc_trace, &mut fec_trace] {
                trace.reset(frame);
            }
            packet_hash.word(9);
            packet_hash.word(frame as u64);
            encoder_ranges.word(9);
            encoder_ranges.word(frame as u64);
            metadata.word(9);
            metadata.word(frame as u64);
            epoch = frame;
            resets += 1;
        }
        let mut frame_input = inputs[frame % inputs.len()].clone();
        let mut encoded = None;
        for level in 0..8 {
            match encoder.encode(&frame_input, &mut packet) {
                Ok(length) => {
                    encoded = Some((length, level));
                    break;
                }
                Err(error) => {
                    encoder_refusals += 1;
                    let error_name = format!("{error:?}");
                    for hash in [&mut metadata, &mut encoder_error_hash] {
                        hash.word(10);
                        hash.word(frame as u64);
                        hash.word(level as u64);
                        hash.word(error_name.len() as u64);
                        hash.bytes(error_name.as_bytes());
                        hash.word(u64::from(encoder.final_range()));
                        hash.word(u64::from(encoder.has_fec()));
                        hash.word(u64::from(encoder.was_dtx()));
                        hash.word(u64::from(encoder.has_transition_redundancy()));
                        hash.word(packet.len() as u64);
                        hash.bytes(&packet);
                    }
                    for sample in &mut frame_input {
                        *sample *= 0.5;
                    }
                }
            }
        }
        let (length, fallback_level) = encoded.unwrap_or_else(|| {
            panic!("encode refused all eight amplitude levels for logical frame {frame}")
        });
        fallback_levels[fallback_level] += 1;
        fallback_max_level = fallback_max_level.max(fallback_level);
        fallback_level_sum += fallback_level;
        if fallback_level != 0 {
            metadata.word(11);
            metadata.word(frame as u64);
            metadata.word(fallback_level as u64);
        }
        assert_eq!(length, budget, "constant full-packet budget");
        let bytes = &packet[..length];
        let parsed = Packet::parse(bytes).expect("packet geometry");
        assert_eq!(parsed.samples(48_000).unwrap(), samples);
        assert_eq!(parsed.toc().mode(), config.mode());
        assert_eq!(parsed.toc().bandwidth(), config.bandwidth());
        assert_eq!(usize::from(parsed.toc().channels()), channels);
        let range = encoder.final_range();
        let has_fec = encoder.has_fec();
        if frame == epoch && fallback_level == 0 {
            assert!(!has_fec, "first packet protects no previous packet");
        }
        fec_packets += usize::from(has_fec);
        packet_hash.word(frame as u64);
        packet_hash.word(length as u64);
        packet_hash.bytes(bytes);
        encoder_ranges.word(frame as u64);
        encoder_ranges.word(u64::from(range));
        metadata.word(frame as u64);
        metadata.word(u64::from(has_fec));
        metadata.word(u64::from(encoder.has_transition_redundancy()));
        metadata.word(u64::from(encoder.was_dtx()));
        metadata.word(parsed.frame_samples(48_000).unwrap() as u64);
        metadata.word(parsed.frames().count() as u64);
        decode(
            &mut normal,
            &mut normal_trace,
            bytes,
            frame,
            samples,
            range,
            &mut pcm,
            channels,
        );
        if (frame - epoch) % 7 == 3 {
            conceal(&mut plc, &mut plc_trace, frame, samples, &mut pcm, channels);
        } else {
            decode(
                &mut plc,
                &mut plc_trace,
                bytes,
                frame,
                samples,
                range,
                &mut pcm,
                channels,
            );
        }
        if let Some(lost_frame) = pending.take() {
            clear(&mut pcm);
            let recovery = fec
                .recover(bytes, samples, &mut pcm)
                .expect("recover previous interval");
            let recovered = recovery == Recovery::Fec;
            assert_eq!(
                recovered, has_fec,
                "recovery outcome disagrees with encoder.has_fec"
            );
            fec_trace.recovery_attempts += 1;
            fec_trace.recovered_fec_packets += usize::from(recovered);
            fec_trace.recovery_fallbacks += usize::from(!recovered);
            fec_trace.output(
                lost_frame,
                if recovered { 3 } else { 4 },
                samples,
                fec.final_range(),
                &pcm,
                channels,
            );
        }
        if (frame - epoch) % 11 == 5 {
            pending = Some(frame);
        } else {
            decode(
                &mut fec,
                &mut fec_trace,
                bytes,
                frame,
                samples,
                range,
                &mut pcm,
                channels,
            );
        }
    }
    if let Some(lost_frame) = pending {
        conceal(
            &mut fec,
            &mut fec_trace,
            lost_frame,
            samples,
            &mut pcm,
            channels,
        );
    }
    for trace in [&normal_trace, &plc_trace, &fec_trace] {
        assert_eq!(
            trace.samples,
            frames * samples,
            "schedule lost or duplicated output"
        );
        assert_eq!(
            trace.events, frames,
            "schedule lost or duplicated intervals"
        );
    }
    format!(
        concat!(
            "{{\"reset_half\":{},\"resets\":{},\"packet_hash\":\"{:016x}\",",
            "\"encoder_range_hash\":\"{:016x}\",\"metadata_hash\":\"{:016x}\",",
            "\"packets_with_fec\":{},\"normal\":{},\"plc_return\":{},\"fec_return\":{},",
            "\"encoder_refusals\":{},\"encoder_error_hash\":\"{:016x}\",",
            "\"fallback_levels\":{:?},\"fallback_max_level\":{},\"fallback_level_sum\":{}}}"
        ),
        reset_half,
        resets,
        packet_hash.0,
        encoder_ranges.0,
        metadata.0,
        fec_packets,
        normal_trace.json(),
        plc_trace.json(),
        fec_trace.json(),
        encoder_refusals,
        encoder_error_hash.0,
        fallback_levels,
        fallback_max_level,
        fallback_level_sum
    )
}

fn bool_modes(value: &str) -> Vec<bool> {
    match value {
        "off" => vec![false],
        "on" => vec![true],
        "both" => vec![false, true],
        _ => panic!("expected off, on or both"),
    }
}

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    assert!(
        args.len() >= 5,
        "MODE BW MS CHANNELS SIGNAL [FRAMES=96] [--input RAW.f32] [--input-channels 1|2] [--bitrate BPS] [--fec off|on|both] [--reset off|on|both]"
    );
    let mode = match args[0].as_str() {
        "silk" => Mode::Silk,
        "hybrid" => Mode::Hybrid,
        "celt" => Mode::Celt,
        _ => panic!("mode"),
    };
    let bandwidth = match args[1].as_str() {
        "NB" => Bandwidth::Narrowband,
        "MB" => Bandwidth::Mediumband,
        "WB" => Bandwidth::Wideband,
        "SWB" => Bandwidth::Superwideband,
        "FB" => Bandwidth::Fullband,
        _ => panic!("bandwidth"),
    };
    let duration = match args[2].as_str() {
        "2.5" => FrameDuration::Ms2_5,
        "5" => FrameDuration::Ms5,
        "10" => FrameDuration::Ms10,
        "20" => FrameDuration::Ms20,
        "40" => FrameDuration::Ms40,
        "60" => FrameDuration::Ms60,
        "80" => FrameDuration::Ms80,
        "100" => FrameDuration::Ms100,
        "120" => FrameDuration::Ms120,
        _ => panic!("duration"),
    };
    let channels: usize = args[3].parse().unwrap();
    assert!((1..=2).contains(&channels));
    let signal = args[4].as_str();
    assert!(
        [
            "voice",
            "voiced",
            "noise",
            "transient",
            "silence",
            "clipping",
            "zeros",
            "file"
        ]
        .contains(&signal)
    );
    let mut frames = 96usize;
    let mut index = 5;
    if let Some(value) = args.get(index).filter(|s| !s.starts_with("--")) {
        frames = value.parse().unwrap();
        index += 1;
    }
    assert!(frames > 1);
    let mut input_path = None;
    let mut input_channels = 1usize;
    let mut requested_bitrate = None;
    let mut fec_modes = if mode == Mode::Celt {
        vec![false]
    } else {
        vec![false, true]
    };
    let mut reset_modes = vec![false, true];
    while index < args.len() {
        let value = args.get(index + 1).expect("option requires value");
        match args[index].as_str() {
            "--input" => input_path = Some(value.clone()),
            "--input-channels" => input_channels = value.parse().unwrap(),
            "--bitrate" => requested_bitrate = Some(value.parse::<u32>().unwrap()),
            "--fec" => fec_modes = bool_modes(value),
            "--reset" => reset_modes = bool_modes(value),
            _ => panic!("unknown option {}", args[index]),
        }
        index += 2;
    }
    assert!((1..=2).contains(&input_channels));
    let raw = input_path.as_ref().map(|path| {
        let bytes = fs::read(path).expect("read rawf32 corpus");
        assert!(!bytes.is_empty() && bytes.len().is_multiple_of(4 * input_channels));
        let samples: Vec<_> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        assert!(
            samples
                .iter()
                .all(|v| v.is_finite() && (-1.0..=1.0).contains(v))
        );
        samples
    });
    assert!(
        signal != "file" || raw.is_some(),
        "file signal requires --input"
    );
    let samples = duration.samples();
    let mono_rate = match (mode, bandwidth) {
        (Mode::Silk, Bandwidth::Narrowband) => 12_000,
        (Mode::Silk, Bandwidth::Mediumband) => 16_000,
        (Mode::Silk, _) => 24_000,
        _ => 32_000,
    };
    let requested = requested_bitrate.unwrap_or(mono_rate * channels as u32);
    let budget = (u64::from(requested) * samples as u64 + 192_000) / 384_000;
    let bitrate = (budget * 384_000 / samples as u64) as u32;
    let mut random = 730_193u32;
    let mut input_hash = Hash::new();
    let inputs: Vec<Vec<f32>> = (0..frames)
        .map(|frame| {
            (0..samples * channels)
                .map(|offset| {
                    let channel = offset % channels;
                    let n = frame * samples + offset / channels;
                    random ^= random << 13;
                    random ^= random >> 17;
                    random ^= random << 5;
                    let noise = (f64::from(random) / f64::from(u32::MAX) * 2.0 - 1.0) as f32;
                    let value = if let Some(raw) = &raw {
                        let source = n % (raw.len() / input_channels);
                        if input_channels == 2 && channels == 1 {
                            (raw[source * 2] + raw[source * 2 + 1]) * 0.5
                        } else {
                            raw[source * input_channels + channel.min(input_channels - 1)]
                        }
                    } else {
                        let time = n as f64 / 48_000.0;
                        let phase = std::f64::consts::TAU * (137.0 * time + 4.0 * time * time)
                            + channel as f64 * (0.4 + (time * 2.0).sin());
                        match signal {
                            "voice" | "voiced" => {
                                (0.32 * phase.sin() + 0.08 * (2.0 * phase).sin()) as f32
                                    + 0.01 * noise
                            }
                            "noise" => noise * 0.7,
                            "transient" => {
                                if n % 4_800 < 120 {
                                    0.9 * noise
                                } else {
                                    0.0
                                }
                            }
                            "silence" => 0.0,
                            "zeros" => f32::from_bits(if (n + channel + frame) % 2 == 0 {
                                0
                            } else {
                                0x8000_0000
                            }),
                            "clipping" => match (n / 24 + channel + frame) % 8 {
                                0 | 1 => 1.0,
                                2 | 3 => -1.0,
                                4 => f32::from_bits(0x3f7f_ffff),
                                5 => -f32::from_bits(0x3f7f_ffff),
                                6 => 0.0,
                                _ => -0.0,
                            },
                            _ => unreachable!(),
                        }
                    };
                    input_hash.word(u64::from(value.to_bits()));
                    value
                })
                .collect()
        })
        .collect();
    let mut traces = Vec::new();
    for fec_enabled in fec_modes {
        assert!(
            !fec_enabled || mode != Mode::Celt,
            "CELT has no FEC encoder option"
        );
        let config = EncoderConfig::new(
            bitrate,
            channels,
            duration,
            bandwidth,
            if mode == Mode::Celt {
                Application::LowDelay
            } else {
                Application::Voice
            },
        )
        .unwrap()
        .with_mode(mode)
        .unwrap()
        .with_rate_control(RateControl::Constant)
        .with_fec(fec_enabled)
        .unwrap();
        for &reset_half in &reset_modes {
            let trace = run(
                config,
                &inputs,
                frames,
                reset_half,
                channels,
                samples,
                budget as usize,
            );
            traces.push(format!(
                "{{\"fec_enabled\":{fec_enabled},\"trace\":{trace}}}"
            ));
        }
    }
    println!(
        concat!(
            "{{\"schema\":1,\"mode\":\"{}\",\"bandwidth\":\"{}\",\"frame_ms\":\"{}\",",
            "\"channels\":{},\"signal\":\"{}\",\"frames\":{},\"packet_bytes\":{},\"bitrate\":{},",
            "\"raw_input\":{},\"input_hash\":\"{:016x}\",\"traces\":[{}]}}"
        ),
        args[0],
        args[1],
        args[2],
        channels,
        signal,
        frames,
        budget,
        bitrate,
        raw.is_some(),
        input_hash.0,
        traces.join(",")
    );
}
