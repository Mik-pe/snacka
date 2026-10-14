// SPDX-License-Identifier: MIT
//! Full CELT throughput at matched raw-packet budgets, including the TOC byte.
//! --quick: 20 ms mono, 32 kb/s voiced. --full: full Cartesian bitrate matrix.
//! --input FILE: normalized little-endian f32 PCM at 48 kHz; --input-channels 1|2.
//! --frames N: changing input/packet sequence length, default 64.
//! --encoder-all: public SILK/Hybrid/CELT encoder geometry; --full adds signals.
//! --decode-only: time the CELT decoder matrix while still preparing identical packets.
//!
//! `reference-tests` links separately installed libopus through its public API.

use snacka::opus::{Bandwidth, CeltConfig, CeltDecoder, CeltEncoder, CeltFrameDuration};
#[cfg(feature = "reference-tests")]
use snacka::opus::{Mode, Packet};
use std::hint::black_box;
use std::time::Instant;

#[path = "opus/encoder.rs"]
mod encoder_bench;

const BATCHES: usize = 1_000;
const WARMUP: usize = 128;
const MINIMUM_BATCH_NS: u128 = 200_000;
const TARGET_BATCH_NS: u128 = 220_000;

#[derive(Clone, Copy)]
enum Signal {
    Voiced,
    Noise,
    Transient,
    File,
}
impl Signal {
    fn name(self) -> &'static str {
        match self {
            Self::Voiced => "voiced",
            Self::Noise => "noise",
            Self::Transient => "transient",
            Self::File => "file",
        }
    }
}

struct Options {
    quick: bool,
    full: bool,
    frames: usize,
    input: Option<Vec<f32>>,
    input_channels: usize,
    bandwidth: Bandwidth,
    encoder_all: bool,
    decode_only: bool,
}
impl Options {
    fn read() -> Self {
        let mut options = Self {
            quick: false,
            full: false,
            frames: 64,
            input: None,
            input_channels: 1,
            bandwidth: Bandwidth::Fullband,
            encoder_all: false,
            decode_only: false,
        };
        let mut input_path = None;
        let mut arguments = std::env::args().skip(1);
        while let Some(argument) = arguments.next() {
            match argument.as_str() {
                "--quick" => options.quick = true,
                "--full" => options.full = true,
                "--encoder-all" => options.encoder_all = true,
                "--decode-only" => options.decode_only = true,
                "--frames" => {
                    options.frames = arguments.next().expect("--frames N").parse().unwrap();
                }
                "--input-channels" => {
                    options.input_channels = arguments
                        .next()
                        .expect("--input-channels 1|2")
                        .parse()
                        .unwrap();
                }
                "--input" => input_path = Some(arguments.next().expect("--input FILE")),
                "--bandwidth" => {
                    options.bandwidth =
                        match arguments.next().expect("--bandwidth NB|WB|SWB|FB").as_str() {
                            "NB" => Bandwidth::Narrowband,
                            "WB" => Bandwidth::Wideband,
                            "SWB" => Bandwidth::Superwideband,
                            "FB" => Bandwidth::Fullband,
                            value => panic!("invalid bandwidth: {value}"),
                        };
                }
                // Cargo may append this marker to a harness-free benchmark.
                "--bench" => {}
                _ => panic!("unknown argument: {argument}"),
            }
        }
        assert!(
            (8..=4_096).contains(&options.frames),
            "--frames must be 8..4096"
        );
        assert!(matches!(options.input_channels, 1 | 2));
        assert!(
            !(options.encoder_all && options.decode_only),
            "--decode-only selects the CELT decoder matrix"
        );
        if let Some(path) = input_path {
            let bytes = std::fs::read(&path).expect("read little-endian f32 PCM");
            assert!(bytes.len().is_multiple_of(4 * options.input_channels));
            let values: Vec<_> = bytes
                .as_chunks::<4>()
                .0
                .iter()
                .map(|&value| f32::from_le_bytes(value))
                .collect();
            assert!(!values.is_empty());
            assert!(
                values
                    .iter()
                    .all(|value| value.is_finite() && value.abs() <= 1.0)
            );
            eprintln!(
                "input={path},input_channels={},rate=48000,format=f32le",
                options.input_channels
            );
            options.input = Some(values);
        }
        options
    }
}

#[derive(Clone, Copy)]
struct Case {
    duration: CeltFrameDuration,
    channels: usize,
    requested_bitrate: u32,
    bandwidth: Bandwidth,
    signal: Signal,
}
impl Case {
    fn packet_bytes(self) -> usize {
        // A fixed byte budget is essential at fractional-byte rates such as
        // 24 kb/s at 2.5 ms. Both encoders receive this same effective bitrate.
        let numerator = u64::from(self.requested_bitrate) * self.duration.samples() as u64;
        ((numerator + 192_000) / 384_000) as usize
    }
    fn bitrate(self) -> u32 {
        (self.packet_bytes() * 384_000 / self.duration.samples()) as u32
    }
    fn config(self) -> CeltConfig {
        CeltConfig::new(self.bitrate(), self.duration, self.channels, self.bandwidth).unwrap()
    }
    fn milliseconds(self) -> &'static str {
        match self.duration {
            CeltFrameDuration::Ms2_5 => "2.5",
            CeltFrameDuration::Ms5 => "5",
            CeltFrameDuration::Ms10 => "10",
            CeltFrameDuration::Ms20 => "20",
        }
    }
    fn bandwidth_name(self) -> &'static str {
        match self.bandwidth {
            Bandwidth::Narrowband => "narrowband",
            Bandwidth::Wideband => "wideband",
            Bandwidth::Superwideband => "superwideband",
            Bandwidth::Fullband => "fullband",
            Bandwidth::Mediumband => unreachable!("CELT has no mediumband"),
        }
    }
}

fn inputs(case: Case, options: &Options) -> Vec<Vec<f32>> {
    inputs_for(case.duration.samples(), case.channels, case.signal, options)
}

fn inputs_for(samples: usize, channels: usize, signal: Signal, options: &Options) -> Vec<Vec<f32>> {
    let mut random = 0x6716_48A3u32;
    (0..options.frames)
        .map(|frame| {
            (0..samples * channels)
                .map(|index| {
                    let channel = index % channels;
                    let sample = frame * samples + index / channels;
                    if let Some(input) = &options.input {
                        let source = sample % (input.len() / options.input_channels);
                        return if options.input_channels == 2 && channels == 1 {
                            0.5 * (input[source * 2] + input[source * 2 + 1])
                        } else {
                            input[source * options.input_channels
                                + channel.min(options.input_channels - 1)]
                        };
                    }
                    random ^= random << 13;
                    random ^= random >> 17;
                    random ^= random << 5;
                    let noise = (random as f64 / f64::from(u32::MAX) * 2.0 - 1.0) as f32;
                    let time = sample as f32 / 48_000.0;
                    let phase = std::f32::consts::TAU * (143.0 * time + 4.0 * time * time)
                        + channel as f32 * 0.41;
                    match signal {
                        Signal::Voiced => {
                            let envelope = 0.45 + 0.12 * (time * 17.0).sin();
                            envelope
                                * (0.70 * phase.sin()
                                    + 0.18 * (2.0 * phase).sin()
                                    + 0.08 * (3.0 * phase).sin())
                                + 0.015 * noise
                        }
                        Signal::Noise => 0.36 * noise,
                        Signal::Transient => {
                            let burst = sample % 4_800;
                            let amplitude = if burst < 160 { 0.80 } else { 0.025 };
                            amplitude * noise + 0.05 * phase.sin()
                        }
                        Signal::File => unreachable!("file input handled above"),
                    }
                })
                .collect()
        })
        .collect()
}

struct Measurement {
    batch: usize,
    p50: f64,
    p95: f64,
    p99: f64,
    paired_ratio: Option<[f64; 3]>,
}
fn measure(mut operation: impl FnMut()) -> Measurement {
    // All timing storage and codec plans exist before the measured operations.
    let mut samples = [0u64; BATCHES];
    for _ in 0..WARMUP {
        operation();
    }
    let mut batch = 1;
    loop {
        let started = Instant::now();
        for _ in 0..batch {
            operation();
        }
        let elapsed = started.elapsed().as_nanos().max(1);
        if elapsed >= MINIMUM_BATCH_NS || batch >= 1 << 20 {
            break;
        }
        batch = ((batch as u128 * TARGET_BATCH_NS).div_ceil(elapsed) as usize)
            .clamp(batch + 1, 1 << 20);
    }
    for elapsed in &mut samples {
        let started = Instant::now();
        for _ in 0..batch {
            operation();
        }
        *elapsed = started.elapsed().as_nanos() as u64;
    }
    samples.sort_unstable();
    Measurement {
        batch,
        p50: samples[500] as f64 / batch as f64,
        p95: samples[950] as f64 / batch as f64,
        p99: samples[990] as f64 / batch as f64,
        paired_ratio: None,
    }
}

#[cfg(feature = "reference-tests")]
fn timed_batch(
    operation: &mut impl FnMut(usize),
    start: usize,
    batch: usize,
    frames: usize,
) -> u64 {
    let started = Instant::now();
    let mut index = start;
    for _ in 0..batch {
        operation(index);
        index += 1;
        if index == frames {
            index = 0;
        }
    }
    started.elapsed().as_nanos() as u64
}

#[cfg(feature = "reference-tests")]
fn measure_pair(
    mut native: impl FnMut(usize),
    mut reference: impl FnMut(usize),
    frames: usize,
) -> [Measurement; 2] {
    let mut native_times = [0u64; BATCHES];
    let mut reference_times = [0u64; BATCHES];
    let mut ratios = [0.0f64; BATCHES];
    let mut start = 0;
    for _ in 0..WARMUP {
        native(start);
        reference(start);
        start = (start + 1) % frames;
    }
    let mut batch = 1;
    loop {
        let first = timed_batch(&mut native, start, batch, frames);
        let second = timed_batch(&mut reference, start, batch, frames);
        start = (start + batch) % frames;
        let fastest = u128::from(first.min(second).max(1));
        if fastest >= MINIMUM_BATCH_NS || batch >= 1 << 20 {
            break;
        }
        batch = ((batch as u128 * TARGET_BATCH_NS).div_ceil(fastest) as usize)
            .clamp(batch + 1, 1 << 20);
    }
    for round in 0..BATCHES {
        // Both histories advance through exactly the same packet indexes;
        // alternating order reduces drift from temperature, frequency and load.
        let (first, second) = if round.is_multiple_of(2) {
            (
                timed_batch(&mut native, start, batch, frames),
                timed_batch(&mut reference, start, batch, frames),
            )
        } else {
            let second = timed_batch(&mut reference, start, batch, frames);
            let first = timed_batch(&mut native, start, batch, frames);
            (first, second)
        };
        native_times[round] = first;
        reference_times[round] = second;
        ratios[round] = first as f64 / second as f64;
        start = (start + batch) % frames;
    }
    native_times.sort_unstable();
    reference_times.sort_unstable();
    ratios.sort_unstable_by(f64::total_cmp);
    let paired_ratio = Some([ratios[500], ratios[950], ratios[990]]);
    [native_times, reference_times].map(|times| Measurement {
        batch,
        p50: times[500] as f64 / batch as f64,
        p95: times[950] as f64 / batch as f64,
        p99: times[990] as f64 / batch as f64,
        paired_ratio,
    })
}

fn row(case: Case, codec: &str, operation: &str, complexity: &str, measured: Measurement) {
    let ratios = measured
        .paired_ratio
        .map(|values| format!("{:.4},{:.4},{:.4}", values[0], values[1], values[2]))
        .unwrap_or_else(|| "NA,NA,NA".to_owned());
    println!(
        "{codec},{operation},48000,{},{},{},{},{},{},{},{complexity},{},{BATCHES},{:.2},{:.2},{:.2},{ratios}",
        case.channels,
        case.milliseconds(),
        case.requested_bitrate,
        case.bitrate(),
        case.packet_bytes(),
        case.bandwidth_name(),
        case.signal.name(),
        measured.batch,
        measured.p50,
        measured.p95,
        measured.p99,
    );
}

fn native(case: Case, input: &[Vec<f32>]) {
    let mut packet = vec![0; case.packet_bytes()];
    let mut encoder = CeltEncoder::new(case.config());
    let mut index = 0;
    let encode = measure(|| {
        let length = encoder
            .encode(black_box(&input[index]), &mut packet)
            .unwrap();
        assert_eq!(length, case.packet_bytes());
        black_box(&packet[..length]);
        index = (index + 1) % input.len();
    });
    row(case, "snacka", "encode", "fixed", encode);
    #[cfg(not(feature = "reference-tests"))]
    native_decode(case, input);
}

#[cfg(not(feature = "reference-tests"))]
fn native_decode(case: Case, input: &[Vec<f32>]) {
    let mut source = CeltEncoder::new(case.config());
    let mut packet = vec![0; case.packet_bytes()];
    // Prepare a stable changing sequence independently of the timed encoder.
    for index in 0..input.len() * 4 {
        source
            .encode(&input[index % input.len()], &mut packet)
            .unwrap();
    }
    let packets: Vec<_> = input
        .iter()
        .map(|pcm| {
            let length = source.encode(pcm, &mut packet).unwrap();
            assert_eq!(length, case.packet_bytes());
            packet.clone()
        })
        .collect();
    let mut decoder = CeltDecoder::new(case.channels).unwrap();
    let mut pcm = vec![0.0; case.duration.samples() * case.channels];
    let mut index = 0;
    let decode = measure(|| {
        let length = decoder
            .decode(black_box(&packets[index]), &mut pcm)
            .unwrap();
        assert_eq!(length, case.duration.samples());
        black_box(&pcm);
        index = (index + 1) % packets.len();
    });
    row(case, "snacka", "decode", "fixed", decode);
}

#[cfg(feature = "reference-tests")]
mod reference {
    use super::*;
    use std::ffi::{CStr, c_char, c_void};
    use std::ptr::NonNull;

    #[link(name = "opus")]
    unsafe extern "C" {
        fn opus_encoder_create(
            rate: i32,
            channels: i32,
            application: i32,
            error: *mut i32,
        ) -> *mut c_void;
        fn opus_encoder_ctl(encoder: *mut c_void, request: i32, ...) -> i32;
        fn opus_encoder_destroy(encoder: *mut c_void);
        fn opus_decoder_create(rate: i32, channels: i32, error: *mut i32) -> *mut c_void;
        fn opus_decoder_destroy(decoder: *mut c_void);
        fn opus_decoder_ctl(decoder: *mut c_void, request: i32, ...) -> i32;
        fn opus_encode_float(
            encoder: *mut c_void,
            pcm: *const f32,
            samples: i32,
            bytes: *mut u8,
            capacity: i32,
        ) -> i32;
        fn opus_decode_float(
            decoder: *mut c_void,
            bytes: *const u8,
            length: i32,
            pcm: *mut f32,
            samples: i32,
            fec: i32,
        ) -> i32;
        fn opus_get_version_string() -> *const c_char;
    }

    pub(super) struct Encoder {
        state: NonNull<c_void>,
        samples: usize,
        channels: usize,
    }
    impl Encoder {
        fn new(case: Case) -> Self {
            Self::configured(
                case.duration.samples(),
                case.channels,
                case.bitrate(),
                case.bandwidth,
                2051,
                false,
            )
        }
        pub(super) fn configured(
            samples: usize,
            channels: usize,
            bitrate: u32,
            bandwidth: Bandwidth,
            application: i32,
            voice_hint: bool,
        ) -> Self {
            assert!(matches!(channels, 1 | 2));
            assert!(matches!(samples, 120 | 240 | 480 | 960 | 1920 | 2880));
            let mut error = 0;
            // SAFETY: Public libopus API receives valid 48 kHz/channel/application
            // values and a writable error result. A checked pointer is owned here.
            let state =
                unsafe { opus_encoder_create(48_000, channels as i32, application, &mut error) };
            assert_eq!(error, 0);
            let encoder = Self {
                state: NonNull::new(state).expect("libopus encoder"),
                samples,
                channels,
            };
            let bandwidth = match bandwidth {
                Bandwidth::Narrowband => 1101,
                Bandwidth::Wideband => 1103,
                Bandwidth::Superwideband => 1104,
                Bandwidth::Fullband => 1105,
                Bandwidth::Mediumband => 1102,
            };
            for (request, value) in [
                (4002, bitrate as i32),
                (4006, 0),
                (4010, 10),
                (4008, bandwidth),
                (4022, channels as i32),
            ] {
                // SAFETY: These documented SET controls each take one promoted
                // C integer. The state is live and exclusively owned.
                let status = unsafe { opus_encoder_ctl(encoder.state.as_ptr(), request, value) };
                assert_eq!(status, 0);
            }
            if voice_hint {
                assert_eq!(
                    // SAFETY: OPUS_SET_SIGNAL takes one promoted C integer;
                    // OPUS_SIGNAL_VOICE is the documented public value 3001.
                    unsafe { opus_encoder_ctl(encoder.state.as_ptr(), 4024, 3001i32) },
                    0
                );
            }
            encoder
        }
        pub(super) fn encode(&mut self, pcm: &[f32], packet: &mut [u8]) -> usize {
            assert_eq!(pcm.len(), self.samples * self.channels);
            // SAFETY: Input contains one complete configured interleaved frame;
            // writable output capacity is exact, and the encoder is exclusively used.
            let result = unsafe {
                opus_encode_float(
                    self.state.as_ptr(),
                    pcm.as_ptr(),
                    self.samples as i32,
                    packet.as_mut_ptr(),
                    packet.len() as i32,
                )
            };
            assert!(result > 0, "libopus encode: {result}");
            result as usize
        }
    }
    impl Drop for Encoder {
        fn drop(&mut self) {
            // SAFETY: The pointer was created and is owned solely by this wrapper.
            unsafe { opus_encoder_destroy(self.state.as_ptr()) };
        }
    }

    pub(super) struct Decoder {
        state: NonNull<c_void>,
        samples: usize,
        channels: usize,
    }
    impl Decoder {
        fn new(case: Case) -> Self {
            Self::configured(case.duration.samples(), case.channels)
        }
        pub(super) fn configured(samples: usize, channels: usize) -> Self {
            assert!(matches!(channels, 1 | 2));
            assert!(matches!(samples, 120 | 240 | 480 | 960 | 1920 | 2880));
            let mut error = 0;
            // SAFETY: Valid fixed rate/channel values and writable error storage.
            let state = unsafe { opus_decoder_create(48_000, channels as i32, &mut error) };
            assert_eq!(error, 0);
            Self {
                state: NonNull::new(state).expect("libopus decoder"),
                samples,
                channels,
            }
        }
        pub(super) fn decode(&mut self, packet: &[u8], pcm: &mut [f32]) -> usize {
            assert_eq!(pcm.len(), self.samples * self.channels);
            // SAFETY: Packet bytes are initialized and output holds the configured
            // complete frame. The live decoder is exclusively owned; FEC is disabled.
            let result = unsafe {
                opus_decode_float(
                    self.state.as_ptr(),
                    packet.as_ptr(),
                    packet.len() as i32,
                    pcm.as_mut_ptr(),
                    self.samples as i32,
                    0,
                )
            };
            assert!(result > 0, "libopus decode: {result}");
            result as usize
        }
        pub(super) fn final_range(&mut self) -> u32 {
            let mut value = 0u32;
            assert_eq!(
                // SAFETY: OPUS_GET_FINAL_RANGE takes a writable u32 pointer. The
                // live decoder is exclusively owned and the result outlives this call.
                unsafe { opus_decoder_ctl(self.state.as_ptr(), 4031, &mut value as *mut u32) },
                0
            );
            value
        }
    }
    impl Drop for Decoder {
        fn drop(&mut self) {
            // SAFETY: The pointer was created and is owned solely by this wrapper.
            unsafe { opus_decoder_destroy(self.state.as_ptr()) };
        }
    }

    pub(super) fn version() -> &'static str {
        // SAFETY: libopus returns a static, non-null, NUL-terminated version string.
        unsafe { CStr::from_ptr(opus_get_version_string()) }
            .to_str()
            .unwrap()
    }

    pub(super) fn run(case: Case, input: &[Vec<f32>], decode_only: bool) {
        let mut source = Encoder::new(case);
        let mut packet = vec![0; case.packet_bytes()];
        for index in 0..input.len() * 4 {
            source.encode(&input[index % input.len()], &mut packet);
        }
        let packets: Vec<_> = input
            .iter()
            .map(|pcm| {
                let length = source.encode(pcm, &mut packet);
                assert_eq!(length, case.packet_bytes(), "equal CBR raw-packet budget");
                let framed = Packet::parse(&packet).unwrap();
                assert_eq!(framed.toc().mode(), Mode::Celt);
                assert_eq!(usize::from(framed.toc().channels()), case.channels);
                assert_eq!(framed.toc().bandwidth(), case.bandwidth);
                packet.clone()
            })
            .collect();
        if !decode_only {
            let mut encoder = Encoder::new(case);
            let mut index = 0;
            let encode = measure(|| {
                let length = encoder.encode(black_box(&input[index]), &mut packet);
                assert_eq!(length, case.packet_bytes());
                black_box(&packet[..length]);
                index = (index + 1) % input.len();
            });
            row(case, "libopus", "encode", "10", encode);
        }
        let mut native = CeltDecoder::new(case.channels).unwrap();
        let mut reference = Decoder::new(case);
        let mut native_pcm = vec![0.0; case.duration.samples() * case.channels];
        let mut reference_pcm = vec![0.0; case.duration.samples() * case.channels];
        let [native_time, reference_time] = measure_pair(
            |index| {
                let length = native
                    .decode(black_box(&packets[index]), &mut native_pcm)
                    .unwrap();
                assert_eq!(length, case.duration.samples());
                black_box(&native_pcm);
            },
            |index| {
                let length = reference.decode(black_box(&packets[index]), &mut reference_pcm);
                assert_eq!(length, case.duration.samples());
                black_box(&reference_pcm);
            },
            packets.len(),
        );
        row(case, "snacka", "decode", "fixed", native_time);
        row(case, "libopus", "decode", "10", reference_time);
    }
}

fn main() {
    let options = Options::read();
    if options.encoder_all {
        encoder_bench::run(&options);
        return;
    }
    eprintln!(
        "48 kHz CELT; {} frames/sequence; {WARMUP} warmups; {BATCHES} calibrated >=200 us batches; p50/p95/p99 describe batch-average throughput, not single-frame tail latency",
        options.frames
    );
    eprintln!(
        "architecture={},cpu_label={}; reference RestrictedLowDelay/CBR/complexity10; Snacka fixed coding choices; equal bytes does not establish equal perceptual quality",
        std::env::consts::ARCH,
        std::env::var("SNACKA_BENCH_CPU_LABEL").unwrap_or_else(|_| "unspecified".to_owned())
    );
    #[cfg(feature = "reference-tests")]
    eprintln!("reference_version={}", reference::version());
    #[cfg(feature = "reference-tests")]
    eprintln!(
        "decoder pairs consume identical libopus packets and packet indexes; paired_decode_ratio percentiles describe native/reference ratios from each alternating timing pair"
    );
    println!(
        "codec,operation,sample_rate,channels,frame_ms,requested_bitrate,packet_bitrate,packet_bytes,bandwidth,signal,complexity,batch_frames,batches,p50_batch_ns_per_frame,p95_batch_ns_per_frame,p99_batch_ns_per_frame,paired_decode_ratio_p50,paired_decode_ratio_p95,paired_decode_ratio_p99"
    );
    let durations = [
        CeltFrameDuration::Ms2_5,
        CeltFrameDuration::Ms5,
        CeltFrameDuration::Ms10,
        CeltFrameDuration::Ms20,
    ];
    let signals = if options.input.is_some() {
        vec![Signal::File]
    } else {
        vec![Signal::Voiced, Signal::Noise, Signal::Transient]
    };
    let mut cases = Vec::new();
    if options.quick {
        cases.push(Case {
            duration: CeltFrameDuration::Ms20,
            channels: 1,
            requested_bitrate: 32_000,
            bandwidth: options.bandwidth,
            signal: signals[0],
        });
    } else {
        for (duration_index, duration) in durations.into_iter().enumerate() {
            for channels in [1, 2] {
                for (signal_index, &signal) in signals.iter().enumerate() {
                    let rates = if options.full {
                        vec![24_000, 64_000, 96_000]
                    } else {
                        vec![
                            [24_000, 64_000, 96_000]
                                [(duration_index + signal_index + channels - 1) % 3],
                        ]
                    };
                    for requested_bitrate in rates {
                        cases.push(Case {
                            duration,
                            channels,
                            requested_bitrate,
                            bandwidth: options.bandwidth,
                            signal,
                        });
                    }
                }
            }
        }
    }
    for case in cases {
        let input = inputs(case, &options);
        if !options.decode_only {
            native(case, &input);
        }
        #[cfg(not(feature = "reference-tests"))]
        if options.decode_only {
            native_decode(case, &input);
        }
        #[cfg(feature = "reference-tests")]
        reference::run(case, &input, options.decode_only);
    }
}
