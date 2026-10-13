// SPDX-License-Identifier: MIT
//! Optional public-API timing oracle for complete Opus decoding at 48 kHz stereo.
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
