// SPDX-License-Identifier: MIT
//! Optional public-API timing oracle for complete Opus decoding.
//!
//! This standalone development tool links Snacka and a separately installed
//! libopus through its public API. No reference implementation source is copied
//! and no foreign codec is linked into the production Rust library.
//!
//! Load official `.bit` records and validate all sample counts, final ranges and
//! finite PCM before timing. Each sample includes a public reset and one complete
//! cached stream; file I/O, allocation, construction and final-range requests are
//! excluded. Eight warmup passes and at least 31 measured passes are required.
//! p50/p95 describe averages across complete stream passes, not frame tail latency.
//! Run three paired repetitions and alternate codec order. This measures CPU
//! throughput; the published opus_compare tool separately checks PCM conformance.
//!
//! Reproduce with a matching installed libopus (adjust its native library path):
//! ```text
//! cargo build --release --features opus --lib --target-dir /tmp/snacka-decode-bench-target
//! rustc --edition=2024 -O -C overflow-checks=yes tools/reference/benchmark_opus_decode.rs \
//!   --extern snacka=/tmp/snacka-decode-bench-target/release/libsnacka.rlib \
//!   -L dependency=/tmp/snacka-decode-bench-target/release/deps \
//!   -L native=/opt/homebrew/opt/opus/lib -o /tmp/snacka-decode-bench
//! /tmp/snacka-decode-bench validate /path/testvector02.bit /path/testvector05.bit /path/testvector06.bit
//! /tmp/snacka-decode-bench time snacka 8 31 /path/testvector02.bit > /tmp/snacka-silk-owned.jsonl
//! /tmp/snacka-decode-bench time libopus 8 31 /path/testvector02.bit > /tmp/snacka-silk-reference.jsonl
//! ```
//! Reports are JSON Lines on stdout. The caller controls export destinations;
//! the examples keep binaries, build caches and reports outside the repository.
//! Multiple vector paths are accepted and each complete stream produces one row.
//! The `matrix` command adds paired mono/stereo measurements at every output
//! rate, ordered packet loss, FEC and return from loss. Its batch percentiles
//! describe continuous stream work, not individual-frame deadline tails.

use snacka::opus::{Decoder, Mode, Packet};
use std::{
    env,
    ffi::{CStr, c_char, c_int, c_void},
    fs,
    hint::black_box,
    path::Path,
    ptr::NonNull,
    time::Instant,
};

const CHANNELS: usize = 2;
const SAMPLE_RATE: usize = 48_000;
const CAPACITY_PER_CHANNEL: usize = 5_760;
const RESET_STATE: c_int = 4_028;
const GET_FINAL_RANGE: c_int = 4_031;

#[link(name = "opus")]
unsafe extern "C" {
    fn opus_decoder_create(rate: c_int, channels: c_int, error: *mut c_int) -> *mut c_void;
    fn opus_decoder_destroy(state: *mut c_void);
    fn opus_decode_float(
        state: *mut c_void,
        data: *const u8,
        length: c_int,
        output: *mut f32,
        capacity: c_int,
        fec: c_int,
    ) -> c_int;
    fn opus_decoder_ctl(state: *mut c_void, request: c_int, ...) -> c_int;
    fn opus_get_version_string() -> *const c_char;
}

struct Reference(NonNull<c_void>);
impl Reference {
    fn new() -> Self {
        let mut error = 0;
        // SAFETY: documented public API; dimensions are valid and error is writable.
        let state =
            unsafe { opus_decoder_create(SAMPLE_RATE as c_int, CHANNELS as c_int, &mut error) };
        assert_eq!(error, 0, "libopus decoder creation");
        Self(NonNull::new(state).expect("libopus returned a null decoder"))
    }
    fn reset(&mut self) {
        // SAFETY: owned live decoder; RESET_STATE takes no variadic argument.
        assert_eq!(unsafe { opus_decoder_ctl(self.0.as_ptr(), RESET_STATE) }, 0);
    }
    fn decode(&mut self, packet: &[u8], output: &mut [f32]) -> usize {
        assert!(output.len() >= CHANNELS * CAPACITY_PER_CHANNEL);
        // SAFETY: live decoder, readable complete packet, and writable stereo
        // capacity for the maximum legal 120 ms packet; ordinary decoding, no FEC.
        let samples = unsafe {
            opus_decode_float(
                self.0.as_ptr(),
                packet.as_ptr(),
                packet.len().try_into().expect("packet length fits int"),
                output.as_mut_ptr(),
                CAPACITY_PER_CHANNEL as c_int,
                0,
            )
        };
        assert!(samples >= 0, "libopus decode returned {samples}");
        samples as usize
    }
    fn final_range(&mut self) -> u32 {
        let mut range = 0u32;
        // SAFETY: live decoder and the public request's required writable u32.
        assert_eq!(
            unsafe { opus_decoder_ctl(self.0.as_ptr(), GET_FINAL_RANGE, &mut range as *mut u32) },
            0
        );
        range
    }
}
impl Drop for Reference {
    fn drop(&mut self) {
        // SAFETY: this wrapper uniquely owns one decoder created by libopus.
        unsafe { opus_decoder_destroy(self.0.as_ptr()) }
    }
}

struct Record {
    packet: Vec<u8>,
    range: u32,
    samples: usize,
}
struct Stream {
    path: String,
    records: Vec<Record>,
    frames: usize,
    samples: usize,
    bytes: usize,
    modes: [usize; 3],
    packet_hash: u64,
}
fn hash(mut current: u64, value: u32) -> u64 {
    current ^= u64::from(value);
    current.wrapping_mul(0x100000001b3)
}
fn load(path: &str) -> Stream {
    let data = fs::read(path).expect("read official .bit stream");
    let mut stream = Stream {
        path: path.into(),
        records: Vec::new(),
        frames: 0,
        samples: 0,
        bytes: 0,
        modes: [0; 3],
        packet_hash: 0xcbf29ce484222325,
    };
    let mut offset = 0usize;
    while offset < data.len() {
        assert!(
            data.len() - offset >= 8,
            "truncated record header at {offset}"
        );
        let length = u32::from_be_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let range = u32::from_be_bytes(data[offset + 4..offset + 8].try_into().unwrap());
        offset += 8;
        assert!(
            length > 0 && length <= data.len() - offset,
            "invalid record length {length} at {offset}"
        );
        let packet = data[offset..offset + length].to_vec();
        offset += length;
        let parsed = Packet::parse(&packet).expect("official packet framing");
        let samples = parsed
            .samples(SAMPLE_RATE as u32)
            .expect("48 kHz packet samples");
        assert!(samples <= CAPACITY_PER_CHANNEL);
        stream.frames += parsed.frame_count();
        stream.samples += samples;
        stream.bytes += length;
        stream.modes[match parsed.toc().mode() {
            Mode::Silk => 0,
            Mode::Hybrid => 1,
            Mode::Celt => 2,
        }] += 1;
        for &byte in &packet {
            stream.packet_hash = hash(stream.packet_hash, u32::from(byte));
        }
        stream.packet_hash = hash(stream.packet_hash, range);
        stream.records.push(Record {
            packet,
            range,
            samples,
        });
    }
    assert!(!stream.records.is_empty(), "empty stream");
    stream
}

fn json_string(value: &str) -> String {
    let mut result = String::from("\"");
    for c in value.chars() {
        match c {
            '"' => result.push_str("\\\""),
            '\\' => result.push_str("\\\\"),
            '\n' => result.push_str("\\n"),
            '\r' => result.push_str("\\r"),
            '\t' => result.push_str("\\t"),
            c if c.is_control() => result.push_str(&format!("\\u{:04x}", c as u32)),
            c => result.push(c),
        }
    }
    result.push('"');
    result
}
fn version() -> String {
    // SAFETY: documented static, null-terminated library version string.
    unsafe { CStr::from_ptr(opus_get_version_string()) }
        .to_string_lossy()
        .into_owned()
}
fn metadata(stream: &Stream) -> String {
    format!(
        "\"path\":{},\"packets\":{},\"frames\":{},\"samples_per_channel\":{},\"compressed_bytes\":{},\"silk_packets\":{},\"hybrid_packets\":{},\"celt_packets\":{},\"packet_range_hash\":\"{:016x}\"",
        json_string(&stream.path),
        stream.records.len(),
        stream.frames,
        stream.samples,
        stream.bytes,
        stream.modes[0],
        stream.modes[1],
        stream.modes[2],
        stream.packet_hash
    )
}
fn validate_owned(stream: &Stream, decoder: &mut Decoder, output: &mut [f32]) -> u64 {
    decoder.reset();
    let mut digest = 0xcbf29ce484222325;
    for (index, record) in stream.records.iter().enumerate() {
        let samples = decoder
            .decode(&record.packet, output)
            .unwrap_or_else(|e| panic!("owned {} packet {index}: {e}", stream.path));
        assert_eq!(samples, record.samples, "owned packet {index} samples");
        assert_eq!(
            decoder.final_range(),
            record.range,
            "owned {} packet {index} final range",
            stream.path
        );
        for &sample in &output[..samples * CHANNELS] {
            assert!(sample.is_finite(), "owned packet {index} nonfinite PCM");
            digest = hash(digest, sample.to_bits());
        }
    }
    digest
}
fn validate_reference(stream: &Stream, decoder: &mut Reference, output: &mut [f32]) -> u64 {
    decoder.reset();
    let mut digest = 0xcbf29ce484222325;
    for (index, record) in stream.records.iter().enumerate() {
        let samples = decoder.decode(&record.packet, output);
        assert_eq!(samples, record.samples, "reference packet {index} samples");
        assert_eq!(
            decoder.final_range(),
            record.range,
            "reference {} packet {index} final range",
            stream.path
        );
        for &sample in &output[..samples * CHANNELS] {
            assert!(sample.is_finite(), "reference packet {index} nonfinite PCM");
            digest = hash(digest, sample.to_bits());
        }
    }
    digest
}
fn run_owned(stream: &Stream, decoder: &mut Decoder, output: &mut [f32]) -> usize {
    decoder.reset();
    let mut samples = 0usize;
    for record in &stream.records {
        samples += decoder
            .decode(black_box(&record.packet), black_box(&mut *output))
            .expect("owned timed decode");
    }
    black_box(samples)
}
fn run_reference(stream: &Stream, decoder: &mut Reference, output: &mut [f32]) -> usize {
    decoder.reset();
    let mut samples = 0usize;
    for record in &stream.records {
        samples += decoder.decode(black_box(&record.packet), black_box(&mut *output));
    }
    black_box(samples)
}
fn timed(stream: &Stream, codec: &str, warmup: usize, passes: usize) {
    // Decoder construction, output allocation, validation and all IO are outside
    // every timed interval. Each interval includes one reset plus a full stream.
    let mut output = vec![0.0f32; CHANNELS * CAPACITY_PER_CHANNEL];
    let mut pass_ns = Vec::with_capacity(passes);
    let pcm_hash;
    match codec {
        "snacka" => {
            let mut decoder = Decoder::new(CHANNELS).expect("owned decoder creation");
            pcm_hash = validate_owned(stream, &mut decoder, &mut output);
            for _ in 0..warmup {
                assert_eq!(run_owned(stream, &mut decoder, &mut output), stream.samples);
            }
            for _ in 0..passes {
                let start = Instant::now();
                let samples = run_owned(stream, &mut decoder, &mut output);
                let elapsed = start.elapsed().as_nanos() as u64;
                assert_eq!(samples, stream.samples);
                pass_ns.push(elapsed);
            }
        }
        "libopus" => {
            let mut decoder = Reference::new();
            pcm_hash = validate_reference(stream, &mut decoder, &mut output);
            for _ in 0..warmup {
                assert_eq!(
                    run_reference(stream, &mut decoder, &mut output),
                    stream.samples
                );
            }
            for _ in 0..passes {
                let start = Instant::now();
                let samples = run_reference(stream, &mut decoder, &mut output);
                let elapsed = start.elapsed().as_nanos() as u64;
                assert_eq!(samples, stream.samples);
                pass_ns.push(elapsed);
            }
        }
        _ => panic!("codec must be snacka or libopus"),
    }
    let mut sorted = pass_ns.clone();
    sorted.sort_unstable();
    let p50 = sorted[(passes - 1) / 2] as f64;
    let p95 = sorted[(95 * passes).div_ceil(100) - 1] as f64;
    let mean = pass_ns.iter().map(|&n| n as f64).sum::<f64>() / passes as f64;
    let raw = pass_ns
        .iter()
        .map(u64::to_string)
        .collect::<Vec<_>>()
        .join(",");
    println!(
        "{{\"kind\":\"timing\",\"codec\":{},\"libopus_version\":{},\"channels\":{CHANNELS},\"sample_rate\":{SAMPLE_RATE},\"warmup_stream_passes\":{warmup},\"timed_stream_passes\":{passes},{},\"pcm_hash\":\"{pcm_hash:016x}\",\"scope\":\"one public decoder reset plus all cached packets; no construction, IO or final-range requests\",\"p50_ns_per_packet\":{:.3},\"p95_ns_per_packet\":{:.3},\"mean_ns_per_packet\":{:.3},\"p50_ns_per_frame\":{:.3},\"p95_ns_per_frame\":{:.3},\"p50_realtime_fraction\":{:.6},\"pass_ns\":[{raw}]}}",
        json_string(codec),
        json_string(&version()),
        metadata(stream),
        p50 / stream.records.len() as f64,
        p95 / stream.records.len() as f64,
        mean / stream.records.len() as f64,
        p50 / stream.frames as f64,
        p95 / stream.frames as f64,
        p50 / 1e9 / (stream.samples as f64 / SAMPLE_RATE as f64)
    );
}
fn main() {
    let args: Vec<_> = env::args().collect();
    if args.get(1).map(String::as_str) == Some("matrix") {
        matrix::main(&args[2..]);
        return;
    }
    if args.get(1).map(String::as_str) == Some("profile-plc") {
        matrix::profile(&args[2..]);
        return;
    }
    if args.len() < 3 {
        eprintln!(
            "Usage: {} validate FILE.bit...\n       {} time snacka|libopus WARMUP_PASSES TIMED_PASSES FILE.bit...",
            args[0], args[0]
        );
        std::process::exit(2);
    }
    match args[1].as_str() {
        "validate" => {
            let mut owned = Decoder::new(CHANNELS).unwrap();
            let mut reference = Reference::new();
            let mut output = vec![0.0; CHANNELS * CAPACITY_PER_CHANNEL];
            for path in &args[2..] {
                let stream = load(path);
                let owned_hash = validate_owned(&stream, &mut owned, &mut output);
                let reference_hash = validate_reference(&stream, &mut reference, &mut output);
                println!(
                    "{{\"kind\":\"validation\",\"libopus_version\":{},\"channels\":{CHANNELS},\"sample_rate\":{SAMPLE_RATE},{},\"all_sample_counts_match\":true,\"all_final_ranges_match\":true,\"owned_pcm_hash\":\"{owned_hash:016x}\",\"reference_pcm_hash\":\"{reference_hash:016x}\"}}",
                    json_string(&version()),
                    metadata(&stream)
                );
            }
        }
        "time" => {
            assert!(
                args.len() >= 6,
                "time mode requires codec, warmups, passes and vectors"
            );
            let warmup: usize = args[3].parse().expect("warmup count");
            let passes: usize = args[4].parse().expect("timed pass count");
            assert!(
                warmup >= 8 && passes >= 31,
                "use at least 8 warmup and 31 timed whole-stream passes"
            );
            // All supplied files and their packet arrays are loaded before timers.
            let streams: Vec<_> = args[5..]
                .iter()
                .map(|p| {
                    assert_eq!(
                        Path::new(p).extension().and_then(|e| e.to_str()),
                        Some("bit")
                    );
                    load(p)
                })
                .collect();
            for stream in &streams {
                timed(stream, &args[2], warmup, passes);
            }
        }
        _ => panic!("mode must be validate or time"),
    }
}

mod matrix {
    use super::*;
    use snacka::opus::Recovery;

    struct Native {
        state: NonNull<c_void>,
        channels: usize,
        capacity: usize,
    }
    impl Native {
        fn new(rate: u32, channels: usize) -> Self {
            let mut error = 0;
            // SAFETY: valid public dimensions and writable error destination.
            let state =
                unsafe { opus_decoder_create(rate as c_int, channels as c_int, &mut error) };
            assert_eq!(error, 0);
            Self {
                state: NonNull::new(state).expect("reference decoder"),
                channels,
                capacity: rate as usize * 120 / 1000,
            }
        }
        fn reset(&mut self) {
            // SAFETY: uniquely owned live decoder, no argument for this request.
            assert_eq!(
                unsafe { opus_decoder_ctl(self.state.as_ptr(), RESET_STATE) },
                0
            );
        }
        fn run(
            &mut self,
            packet: Option<&[u8]>,
            samples: usize,
            fec: bool,
            out: &mut [f32],
        ) -> usize {
            assert!(samples <= self.capacity && out.len() >= samples * self.channels);
            let (data, length) =
                packet.map_or((std::ptr::null(), 0), |p| (p.as_ptr(), p.len() as c_int));
            // SAFETY: complete readable packet or documented NULL PLC; output
            // covers the requested per-channel capacity for this live decoder.
            let count = unsafe {
                opus_decode_float(
                    self.state.as_ptr(),
                    data,
                    length,
                    out.as_mut_ptr(),
                    samples as c_int,
                    c_int::from(fec),
                )
            };
            assert!(count >= 0, "reference decoding: {count}");
            count as usize
        }
        fn range(&mut self) -> u32 {
            let mut range = 0;
            // SAFETY: live decoder and writable public request u32 destination.
            assert_eq!(
                unsafe {
                    opus_decoder_ctl(self.state.as_ptr(), GET_FINAL_RANGE, &mut range as *mut u32)
                },
                0
            );
            range
        }
    }
    impl Drop for Native {
        fn drop(&mut self) {
            // SAFETY: this wrapper uniquely owns a decoder created by libopus.
            unsafe { opus_decoder_destroy(self.state.as_ptr()) }
        }
    }

    #[derive(Clone, Copy)]
    enum Action {
        Decode(usize),
        Conceal(usize),
        Recover(usize, usize),
    }

    fn plan(stream: &Stream, rate: u32, schedule: &str) -> Vec<Action> {
        assert!(
            stream.records.len() >= 12,
            "matrix stream needs at least 12 packets"
        );
        let mut actions = Vec::with_capacity(stream.records.len());
        for (index, record) in stream.records.iter().enumerate() {
            let samples = record.samples * rate as usize / SAMPLE_RATE;
            actions.push(match (schedule, index) {
                ("plc-return" | "fec-return", 8) => Action::Conceal(samples),
                ("plc-return", 9) => Action::Conceal(samples),
                ("fec-return", 9) => Action::Recover(index + 1, samples),
                _ => Action::Decode(index),
            });
        }
        actions
    }
    fn owned_action(
        decoder: &mut Decoder,
        stream: &Stream,
        action: Action,
        out: &mut [f32],
    ) -> (usize, bool) {
        match action {
            Action::Decode(i) => (
                decoder
                    .decode(black_box(&stream.records[i].packet), black_box(out))
                    .expect("owned decode"),
                false,
            ),
            Action::Conceal(n) => (
                decoder.conceal(n, black_box(out)).expect("owned PLC"),
                false,
            ),
            Action::Recover(i, n) => (
                n,
                decoder
                    .recover(black_box(&stream.records[i].packet), n, black_box(out))
                    .expect("owned recovery")
                    == Recovery::Fec,
            ),
        }
    }
    fn native_action(
        decoder: &mut Native,
        stream: &Stream,
        rate: u32,
        action: Action,
        out: &mut [f32],
    ) -> usize {
        let (packet, samples, fec) = match action {
            Action::Decode(i) => (
                Some(&stream.records[i].packet[..]),
                stream.records[i].samples * rate as usize / SAMPLE_RATE,
                false,
            ),
            Action::Conceal(n) => (None, n, false),
            Action::Recover(i, n) => (Some(&stream.records[i].packet[..]), n, true),
        };
        decoder.run(black_box(packet), samples, fec, black_box(out))
    }
    fn owned_pass(
        decoder: &mut Decoder,
        stream: &Stream,
        _rate: u32,
        actions: &[Action],
        out: &mut [f32],
        repeats: usize,
    ) {
        for _ in 0..repeats {
            for &action in actions {
                black_box(owned_action(decoder, stream, action, out));
            }
        }
    }
    fn native_pass(
        decoder: &mut Native,
        stream: &Stream,
        rate: u32,
        actions: &[Action],
        out: &mut [f32],
        repeats: usize,
    ) {
        for _ in 0..repeats {
            for &action in actions {
                black_box(native_action(decoder, stream, rate, action, out));
            }
        }
    }
    fn percentile(values: &[u64], pct: usize) -> f64 {
        let mut ordered = values.to_vec();
        ordered.sort_unstable();
        ordered[(values.len() * pct).div_ceil(100) - 1] as f64
    }
    fn array(values: &[u64]) -> String {
        values
            .iter()
            .map(u64::to_string)
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Sampling workload only: every healthy packet resets the loss streak,
    /// so the subsequent public conceal call includes first-loss analysis.
    pub(super) fn profile(args: &[String]) {
        assert_eq!(
            args.len(),
            5,
            "profile-plc snacka|libopus CHANNELS LOSS_SAMPLES SECONDS FILE.bit"
        );
        let codec = &args[0];
        let channels: usize = args[1].parse().expect("channels");
        let loss_samples: usize = args[2].parse().expect("loss samples");
        let seconds: u64 = args[3].parse().expect("profile seconds");
        assert!((1..=2).contains(&channels));
        assert!([120, 240, 480, 960].contains(&loss_samples));
        assert!((1..=60).contains(&seconds));
        let stream = load(&args[4]);
        assert!(
            stream
                .records
                .iter()
                .all(|record| Packet::parse(&record.packet).unwrap().toc().mode() == Mode::Celt)
        );
        let mut output = vec![0.0; channels * CAPACITY_PER_CHANNEL];
        println!(
            "{{\"kind\":\"profile_start\",\"codec\":{},\"pid\":{},\"channels\":{channels},\"loss_samples\":{loss_samples},\"duration_seconds\":{seconds},{}}}",
            json_string(codec),
            std::process::id(),
            metadata(&stream)
        );
        std::io::Write::flush(&mut std::io::stdout()).unwrap();
        let start = Instant::now();
        let mut completed = 0usize;
        match codec.as_str() {
            "snacka" => {
                let mut decoder = Decoder::new(channels).unwrap();
                while start.elapsed().as_secs() < seconds {
                    let record = &stream.records[completed % stream.records.len()];
                    assert_eq!(
                        decoder
                            .decode(black_box(&record.packet), black_box(&mut output))
                            .unwrap(),
                        record.samples
                    );
                    assert_eq!(
                        decoder
                            .conceal(loss_samples, black_box(&mut output))
                            .unwrap(),
                        loss_samples
                    );
                    black_box(&output[..loss_samples * channels]);
                    completed += 1;
                }
            }
            "libopus" => {
                let mut decoder = Native::new(48_000, channels);
                while start.elapsed().as_secs() < seconds {
                    let record = &stream.records[completed % stream.records.len()];
                    assert_eq!(
                        decoder.run(
                            Some(black_box(&record.packet)),
                            record.samples,
                            false,
                            black_box(&mut output)
                        ),
                        record.samples
                    );
                    assert_eq!(
                        decoder.run(None, loss_samples, false, black_box(&mut output)),
                        loss_samples
                    );
                    black_box(&output[..loss_samples * channels]);
                    completed += 1;
                }
            }
            _ => panic!("profile codec must be snacka or libopus"),
        }
        println!(
            "{{\"kind\":\"profile_complete\",\"codec\":{},\"healthy_plus_first_loss_pairs\":{completed},\"elapsed_ns\":{},\"scope\":\"sampling workload includes healthy decode and first-loss conceal; not a paired latency acceptance benchmark\"}}",
            json_string(codec),
            start.elapsed().as_nanos()
        );
    }

    pub(super) fn main(args: &[String]) {
        assert_eq!(
            args.len(),
            6,
            "matrix RATE CHANNELS decode|plc-return|fec-return BATCHES MIN_NS FILE.bit"
        );
        let rate: u32 = args[0].parse().expect("output rate");
        let channels: usize = args[1].parse().expect("output channels");
        let schedule = &args[2];
        let batches: usize = args[3].parse().expect("batch count");
        let minimum: u64 = args[4].parse().expect("minimum batch ns");
        assert!([8000, 12000, 16000, 24000, 48000].contains(&rate));
        assert!((1..=2).contains(&channels));
        assert!(["decode", "plc-return", "fec-return"].contains(&schedule.as_str()));
        assert!(batches == 0 || batches >= 100);
        assert!(minimum >= 200_000);
        let stream = load(&args[5]);
        let actions = plan(&stream, rate, schedule);
        let mut owned = Decoder::with_sample_rate(channels, rate).expect("owned decoder");
        let mut native = Native::new(rate, channels);
        let mut left = vec![0.0; channels * rate as usize * 120 / 1000];
        let mut right = left.clone();
        let mut fec_events = 0;
        let mut recovery_ranges = Vec::new();
        let mut error_energy = 0.0f64;
        let mut reference_energy = 0.0f64;
        let mut compared = 0;
        let mut max_error = 0.0f32;
        let mut operation_counts = [0usize; 4];
        let mut operation_samples = [0usize; 4];
        let mut operation_error_energy = [0.0f64; 4];
        let mut operation_reference_energy = [0.0f64; 4];
        let mut bands = [0usize; 5];
        let mut durations = [0usize; 6];
        let mut coded_channels = [0usize; 2];
        for record in &stream.records {
            let packet = Packet::parse(&record.packet).unwrap();
            bands[match packet.toc().bandwidth() {
                snacka::opus::Bandwidth::Narrowband => 0,
                snacka::opus::Bandwidth::Mediumband => 1,
                snacka::opus::Bandwidth::Wideband => 2,
                snacka::opus::Bandwidth::Superwideband => 3,
                snacka::opus::Bandwidth::Fullband => 4,
            }] += 1;
            durations[match record.samples {
                120 => 0,
                240 => 1,
                480 => 2,
                960 => 3,
                1920 => 4,
                2880 => 5,
                _ => panic!("matrix supports 2.5 through 60 ms packets"),
            }] += 1;
            coded_channels[usize::from(packet.toc().channels()) - 1] += 1;
        }
        for (index, &action) in actions.iter().enumerate() {
            let operation_kind = match action {
                Action::Decode(i) => usize::from(
                    Packet::parse(&stream.records[i].packet)
                        .unwrap()
                        .frames()
                        .any(|frame| frame.len() <= 1),
                ),
                Action::Conceal(_) => 2,
                Action::Recover(..) => 3,
            };
            operation_counts[operation_kind] += 1;
            let (count, fec) = owned_action(&mut owned, &stream, action, &mut left);
            let expected = native_action(&mut native, &stream, rate, action, &mut right);
            assert_eq!(count, expected, "packet {index} sample count");
            let reference_range = native.range();
            if matches!(action, Action::Recover(..)) {
                recovery_ranges.push(format!("{{\"packet_index\":{index},\"owned_entropy_interval\":{},\"reference_debug_interval\":{reference_range},\"decoded_lbrr\":{fec}}}", owned.final_range()));
            } else {
                assert_eq!(
                    owned.final_range(),
                    reference_range,
                    "packet {index} final range"
                );
            }
            if matches!(action, Action::Decode(_)) {
                assert_eq!(
                    owned.final_range(),
                    stream.records[index].range,
                    "packet {index} recorded final range"
                );
            }
            fec_events += usize::from(fec);
            for (&a, &b) in left[..count * channels].iter().zip(&right) {
                assert!(a.is_finite() && b.is_finite(), "packet {index} finite PCM");
                let error = a - b;
                max_error = max_error.max(error.abs());
                error_energy += f64::from(error) * f64::from(error);
                reference_energy += f64::from(b) * f64::from(b);
                compared += 1;
                operation_error_energy[operation_kind] += f64::from(error) * f64::from(error);
                operation_reference_energy[operation_kind] += f64::from(b) * f64::from(b);
                operation_samples[operation_kind] += 1;
            }
        }
        let mut repeats = 1usize;
        let mut own_ns = Vec::with_capacity(batches);
        let mut native_ns = Vec::with_capacity(batches);
        if batches != 0 {
            // Calibration is outside measurements. Both sides receive the same
            // number of repeated cached streams within each paired batch.
            loop {
                let start = Instant::now();
                owned_pass(&mut owned, &stream, rate, &actions, &mut left, repeats);
                let a = start.elapsed().as_nanos() as u64;
                let start = Instant::now();
                native_pass(&mut native, &stream, rate, &actions, &mut right, repeats);
                let b = start.elapsed().as_nanos() as u64;
                if a.min(b) >= minimum * 3 {
                    break;
                }
                repeats *= 2;
                assert!(repeats <= 65_536);
            }
            owned.reset();
            native.reset();
            for _ in 0..8 {
                owned_pass(&mut owned, &stream, rate, &actions, &mut left, repeats);
                native_pass(&mut native, &stream, rate, &actions, &mut right, repeats);
            }
            for batch in 0..batches {
                let (a, b);
                if batch % 2 == 0 {
                    let start = Instant::now();
                    owned_pass(&mut owned, &stream, rate, &actions, &mut left, repeats);
                    a = start.elapsed().as_nanos() as u64;
                    let start = Instant::now();
                    native_pass(&mut native, &stream, rate, &actions, &mut right, repeats);
                    b = start.elapsed().as_nanos() as u64;
                } else {
                    let start = Instant::now();
                    native_pass(&mut native, &stream, rate, &actions, &mut right, repeats);
                    b = start.elapsed().as_nanos() as u64;
                    let start = Instant::now();
                    owned_pass(&mut owned, &stream, rate, &actions, &mut left, repeats);
                    a = start.elapsed().as_nanos() as u64;
                }
                own_ns.push(a);
                native_ns.push(b);
            }
        }
        let timing = if batches == 0 {
            String::new()
        } else {
            let operations = repeats * actions.len();
            let metrics = [50, 95, 99]
                .into_iter()
                .map(|pct| {
                    let a = percentile(&own_ns, pct);
                    let b = percentile(&native_ns, pct);
                    format!(
                        "\"p{pct}_ns_per_operation\":{{\"snacka\":{},\"libopus\":{},\"ratio\":{}}}",
                        a / operations as f64,
                        b / operations as f64,
                        a / b
                    )
                })
                .collect::<Vec<_>>()
                .join(",");
            format!(
                ",\"repeated_streams_per_batch\":{repeats},\"operations_per_batch\":{operations},\"minimum_observed_batch_ns\":{},\"all_batches_meet_minimum\":{},\"owned_batch_ns\":[{}],\"reference_batch_ns\":[{}],{metrics}",
                own_ns.iter().chain(&native_ns).min().unwrap(),
                own_ns.iter().chain(&native_ns).all(|&n| n >= minimum),
                array(&own_ns),
                array(&native_ns)
            )
        };
        let recovery_ranges = recovery_ranges.join(",");
        let pcm_by_operation = ["coded_packet", "implicit_plc_packet", "explicit_plc", "fec_recovery"]
            .iter()
            .enumerate()
            .map(|(index, name)| {
                let denominator = operation_samples[index].max(1) as f64;
                format!(
                    "\"{name}\":{{\"operations\":{},\"interleaved_samples\":{},\"error_rms\":{},\"reference_rms\":{}}}",
                    operation_counts[index],
                    operation_samples[index],
                    (operation_error_energy[index] / denominator).sqrt(),
                    (operation_reference_energy[index] / denominator).sqrt(),
                )
            })
            .collect::<Vec<_>>()
            .join(",");
        println!(
            "{{\"kind\":\"paired_decoder_matrix\",\"libopus_version\":{},\"sample_rate\":{rate},\"channels\":{channels},\"schedule\":{},\"timed_batches\":{batches},\"requested_minimum_batch_ns\":{minimum},\"fec_events\":{fec_events},\"all_sample_counts_match\":true,\"ordinary_and_plc_final_ranges_match\":true,\"recovery_intervals\":[{recovery_ranges}],\"recovery_interval_scope\":\"owned consumed Header/LBRR prefix; reference FEC debug state may include Hybrid PLC RNG; comparison is reported, ordinary following packet remains exact\",\"bandwidth_packet_counts\":{bands:?},\"duration_packet_counts_2_5_5_10_20_40_60\":{durations:?},\"coded_channel_packet_counts\":{coded_channels:?},\"reference_pcm_error_rms\":{},\"reference_pcm_rms\":{},\"reference_pcm_max_error\":{max_error},\"pcm_by_operation_kind\":{{{pcm_by_operation}}},\"pcm_diagnostic_scope\":\"unshifted PCM by current operation; coded packets can follow differing PLC histories; error metrics are not quality acceptance\",\"scope\":\"continuous ordered cached streams; no constructor, reset, IO, validation or final-range calls in timers; alternating codec order\",{}{timing}}}",
            json_string(&version()),
            json_string(schedule),
            (error_energy / compared as f64).sqrt(),
            (reference_energy / compared as f64).sqrt(),
            metadata(&stream)
        );
    }
}
