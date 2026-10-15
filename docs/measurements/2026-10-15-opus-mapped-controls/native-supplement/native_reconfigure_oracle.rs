//! DRAFT SOURCE ONLY: never compiled or run during the stereo cost window.
//! Native public controls and wire observations only. No Snacka linkage, timers,
//! new speech, foreign implementation bodies, or batch-atomicity claim.
//! Before any later execution, root must pin this source, toolchain, native
//! library, headers and existing numeric-generator source, and authorize it.

use std::{env, ffi::{CStr, c_char, c_int, c_void}, fs::{self, File}, io::Write, path::Path, ptr};

#[link(name = "opus")]
unsafe extern "C" {
    fn opus_get_version_string() -> *const c_char;
    fn opus_multistream_encoder_create(rate: c_int, channels: c_int, streams: c_int, coupled: c_int, mapping: *const u8, application: c_int, error: *mut c_int) -> *mut c_void;
    fn opus_projection_ambisonics_encoder_create(rate: c_int, channels: c_int, family: c_int, streams: *mut c_int, coupled: *mut c_int, application: c_int, error: *mut c_int) -> *mut c_void;
    fn opus_multistream_encoder_ctl(state: *mut c_void, request: c_int, ...) -> c_int;
    fn opus_projection_encoder_ctl(state: *mut c_void, request: c_int, ...) -> c_int;
    fn opus_encoder_ctl(state: *mut c_void, request: c_int, ...) -> c_int;
    fn opus_multistream_encode_float(state: *mut c_void, pcm: *const f32, samples: c_int, bytes: *mut u8, capacity: c_int) -> c_int;
    fn opus_projection_encode_float(state: *mut c_void, pcm: *const f32, samples: c_int, bytes: *mut u8, capacity: c_int) -> c_int;
    fn opus_multistream_encoder_destroy(state: *mut c_void);
    fn opus_projection_encoder_destroy(state: *mut c_void);
    fn opus_multistream_decoder_create(rate: c_int, channels: c_int, streams: c_int, coupled: c_int, mapping: *const u8, error: *mut c_int) -> *mut c_void;
    fn opus_multistream_decode_float(state: *mut c_void, bytes: *const u8, length: c_int, pcm: *mut f32, capacity: c_int, fec: c_int) -> c_int;
    fn opus_multistream_decoder_ctl(state: *mut c_void, request: c_int, ...) -> c_int;
    fn opus_multistream_decoder_destroy(state: *mut c_void);
}

const CHILD: c_int = 5120;
const FINAL_RANGE: c_int = 4031;
const RESET: c_int = 4028;
// Application, bitrate, VBR, bandwidth, FEC, loss, requested DTX, signal,
// sample rate and observed native in-DTX state. None proves actual LBRR.
const GETS: [c_int; 10] = [4001, 4003, 4007, 4009, 4013, 4015, 4017, 4025, 4029, 4049];

#[derive(Clone, Copy)]
enum Kind { Mapped, Projected }
impl Kind { fn name(self) -> &'static str { match self { Self::Mapped => "mapped", Self::Projected => "projected" } } }

struct Native { state: *mut c_void, kind: Kind, rate: i32, channels: usize, streams: usize, coupled: usize }
impl Native {
    fn new(kind: Kind, rate: i32, out: &mut File) -> Option<Self> {
        let mut error = 0;
        let (mut streams, mut coupled) = (3, 1);
        // Supplemental mapped input has every coded channel, an alias and a silent slot.
        // The original omitted-stream constructor failures remain retained.
        let mapping = [0, 1, 2, 3, 255, 0];
        let channels = if matches!(kind, Kind::Mapped) { 6 } else { 4 };
        // SAFETY: Complete live constructor arguments and writable scalar slots.
        let state = unsafe { match kind {
            Kind::Mapped => opus_multistream_encoder_create(rate, channels, streams, coupled, mapping.as_ptr(), 2048, &mut error),
            Kind::Projected => opus_projection_ambisonics_encoder_create(rate, channels, 3, &mut streams, &mut coupled, 2048, &mut error),
        }};
        writeln!(out, "{{\"kind\":\"create\",\"family\":{:?},\"rate\":{rate},\"channels\":{channels},\"streams\":{streams},\"coupled\":{coupled},\"code\":{error},\"nonnull\":{}}}", kind.name(), !state.is_null()).unwrap();
        if state.is_null() { return None; }
        let result = Self { state, kind, rate, channels: channels as usize, streams: streams.max(0) as usize, coupled: coupled.max(0) as usize };
        if error != 0 || streams <= 0 || coupled < 0 || coupled > streams || streams + coupled > 255 { drop(result); return None; }
        Some(result)
    }
    fn child(&self, stream: usize, stage: &str, out: &mut File) -> Option<*mut c_void> {
        let mut child = ptr::null_mut::<c_void>();
        // SAFETY: In-range index and live writable borrowed-pointer slot. The
        // parent owns the result; it is never separately destroyed.
        let code = unsafe { match self.kind {
            Kind::Mapped => opus_multistream_encoder_ctl(self.state, CHILD, stream as c_int, &mut child as *mut *mut c_void),
            Kind::Projected => opus_projection_encoder_ctl(self.state, CHILD, stream as c_int, &mut child as *mut *mut c_void),
        }};
        writeln!(out, "{{\"kind\":\"child\",\"family\":{:?},\"rate\":{},\"stage\":{stage:?},\"stream\":{stream},\"code\":{code},\"nonnull\":{}}}", self.kind.name(), self.rate, !child.is_null()).unwrap();
        (code == 0 && !child.is_null()).then_some(child)
    }
    fn snapshot(&self, stage: &str, out: &mut File) {
        for request in GETS {
            let mut value = -999;
            // SAFETY: These public GETs take a writable int slot on both owners.
            let code = unsafe { match self.kind {
                Kind::Mapped => opus_multistream_encoder_ctl(self.state, request, &mut value as *mut c_int),
                Kind::Projected => opus_projection_encoder_ctl(self.state, request, &mut value as *mut c_int),
            }};
            writeln!(out, "{{\"kind\":\"aggregate_get\",\"family\":{:?},\"rate\":{},\"stage\":{stage:?},\"request\":{request},\"code\":{code},\"value\":{value}}}", self.kind.name(), self.rate).unwrap();
        }
        for stream in 0..self.streams {
            let Some(child) = self.child(stream, stage, out) else { continue; };
            for request in GETS {
                let mut value = -999;
                // SAFETY: Borrowed child remains live; each request writes int.
                let code = unsafe { opus_encoder_ctl(child, request, &mut value as *mut c_int) };
                writeln!(out, "{{\"kind\":\"child_get\",\"family\":{:?},\"rate\":{},\"stage\":{stage:?},\"stream\":{stream},\"request\":{request},\"code\":{code},\"value\":{value}}}", self.kind.name(), self.rate).unwrap();
            }
        }
    }
    fn configure(&self, stage: &Stage, out: &mut File) {
        for stream in 0..self.streams {
            let Some(child) = self.child(stream, stage.name, out) else { continue; };
            let channels = if stream < self.coupled { 2 } else { 1 };
            for (request, value) in [(4000, stage.application), (4002, stage.mono_bitrate * channels), (4006, stage.vbr), (4008, stage.bandwidth), (4012, stage.fec), (4014, stage.loss), (4016, stage.dtx), (4024, 3001)] {
                // SAFETY: Each named public SET uses one promoted int. This is
                // native-only sequential control, NOT an atomic group operation.
                let code = unsafe { opus_encoder_ctl(child, request, value) };
                writeln!(out, "{{\"kind\":\"child_set\",\"family\":{:?},\"rate\":{},\"stage\":{:?},\"stream\":{stream},\"request\":{request},\"requested\":{value},\"code\":{code}}}", self.kind.name(), self.rate, stage.name).unwrap();
            }
        }
        self.snapshot(stage.name, out);
    }
    fn reset(&self, out: &mut File) {
        // SAFETY: Public reset has no variadic arguments; owner is live.
        let code = unsafe { match self.kind { Kind::Mapped => opus_multistream_encoder_ctl(self.state, RESET), Kind::Projected => opus_projection_encoder_ctl(self.state, RESET) } };
        writeln!(out, "{{\"kind\":\"reset\",\"family\":{:?},\"rate\":{},\"code\":{code}}}", self.kind.name(), self.rate).unwrap();
        self.snapshot("after-reset", out);
    }
}
impl Drop for Native { fn drop(&mut self) { /* SAFETY: Owned parent destroyed once. */ unsafe { match self.kind { Kind::Mapped => opus_multistream_encoder_destroy(self.state), Kind::Projected => opus_projection_encoder_destroy(self.state) } } } }

struct Decoder { state: *mut c_void, channels: usize }
impl Decoder {
    fn new(encoder: &Native, out: &mut File) -> Option<Self> {
        let channels = encoder.streams + encoder.coupled;
        let identity: Vec<u8> = (0..channels).map(|x| x as u8).collect();
        let mut error = 0;
        // SAFETY: Valid copied coded-channel identity layout; decoder observes
        // native projected coded PCM, not the automatic demixing matrix.
        let state = unsafe { opus_multistream_decoder_create(encoder.rate, channels as c_int, encoder.streams as c_int, encoder.coupled as c_int, identity.as_ptr(), &mut error) };
        let classification = if error == 0 && !state.is_null() { "usable" }
            else if error == -5 { "unsupported" }
            else if error == 0 { "invalid-success-null" }
            else { "error" };
        writeln!(out, "{{\"kind\":\"decoder_create\",\"family\":{:?},\"rate\":{},\"channels\":{channels},\"code\":{error},\"nonnull\":{},\"classification\":{classification:?}}}", encoder.kind.name(), encoder.rate, !state.is_null()).unwrap();
        if state.is_null() { return None; }
        if error != 0 {
            // SAFETY: Any non-null constructor allocation remains parent-owned
            // even when rejected here; never use it as a successful decoder.
            unsafe { opus_multistream_decoder_destroy(state) };
            return None;
        }
        Some(Self { state, channels })
    }
    fn decode(&mut self, encoder: &Native, packet: &[u8], seq: usize, root: &Path, out: &mut File) {
        let samples = encoder.rate as usize / 50;
        let mut pcm = vec![12345.25; samples * self.channels + 7];
        // SAFETY: Full live packet, sufficient output capacity, ordinary decode.
        let code = unsafe { opus_multistream_decode_float(self.state, packet.as_ptr(), packet.len() as c_int, pcm.as_mut_ptr(), samples as c_int, 0) };
        let count = code.max(0).min(samples as c_int) as usize * self.channels;
        // Save physical float bits, including the entire advertised output and
        // seven sentinel samples, even on error, so diagnostics are auditable.
        let filename = format!("{}-{}-{seq:03}.pcm.f32le", encoder.kind.name(), encoder.rate);
        let raw_pcm: Vec<u8> = pcm.iter().flat_map(|sample| sample.to_bits().to_le_bytes()).collect();
        fs::write(root.join(&filename), raw_pcm).unwrap();
        let mut range = 0u32;
        // SAFETY: Public unsigned-range GET writes the live u32 slot.
        let range_code = unsafe { opus_multistream_decoder_ctl(self.state, FINAL_RANGE, &mut range as *mut u32) };
        writeln!(out, "{{\"kind\":\"decode\",\"family\":{:?},\"rate\":{},\"sequence\":{seq},\"code\":{code},\"expected_samples\":{samples},\"channels\":{},\"physical_floats\":{},\"guard_floats\":7,\"pcm\":{filename:?},\"finite\":{},\"suffix_unchanged\":{},\"range_code\":{range_code},\"range\":{range}}}", encoder.kind.name(), encoder.rate, self.channels, pcm.len(), pcm[..count].iter().all(|x| x.is_finite()), pcm[count..].iter().all(|&x| x == 12345.25)).unwrap();
    }
}
impl Drop for Decoder { fn drop(&mut self) { /* SAFETY: Owned decoder destroyed once. */ unsafe { opus_multistream_decoder_destroy(self.state) } } }

struct Stage { name: &'static str, mono_bitrate: i32, bandwidth: i32, vbr: i32, fec: i32, loss: i32, dtx: i32, application: i32, quiet: bool, frames: usize }
// Existing bounded formula, copied from owned tests/opus_input_rates.rs::pcm.
// The sum is bounded by0.215; no recorded speech/quality corpus or fitting.
fn pcm(samples: usize, channels: usize, rate: u32, start: usize) -> Vec<f32> {
    (0..samples * channels).map(|index| {
        let t = (start + index / channels) as f32 / rate as f32;
        0.18 * (std::f32::consts::TAU * (137.0 + (index % channels) as f32 * 61.0) * t).sin()
            + 0.035 * (std::f32::consts::TAU * 1297.0 * t).sin()
    }).collect()
}

fn main() {
    let args: Vec<_> = env::args().collect();
    assert!(args.len() == 3 && args[1] == "--run-native-after-freeze", "DRAFT: inspect/pin/authorize first; use --run-native-after-freeze NEW_OUTPUT_DIR only after root releases CPU");
    let root = Path::new(&args[2]);
    fs::create_dir(root).expect("new output directory, preserving previous runs");
    let mut out = File::create(root.join("events.jsonl")).unwrap();
    // SAFETY: Public immutable NUL-terminated native version string.
    let version = unsafe { CStr::from_ptr(opus_get_version_string()) }.to_string_lossy();
    writeln!(out, "{{\"kind\":\"version\",\"version\":{version:?},\"scope\":\"native controls/wire only; no timers, quality or batch atomicity claim\"}}").unwrap();
    let stages = [
        Stage { name: "base", mono_bitrate: 48_000, bandwidth: 1103, vbr: 0, fec: 0, loss: 0, dtx: 0, application: 2048, quiet: false, frames: 6 },
        Stage { name: "downshift-fec", mono_bitrate: 24_000, bandwidth: 1103, vbr: 0, fec: 1, loss: 30, dtx: 0, application: 2048, quiet: false, frames: 8 },
        Stage { name: "quiet-dtx", mono_bitrate: 16_000, bandwidth: 1103, vbr: 1, fec: 0, loss: 0, dtx: 1, application: 2048, quiet: true, frames: 30 },
        Stage { name: "upshift-resume", mono_bitrate: 48_000, bandwidth: 1105, vbr: 1, fec: 0, loss: 0, dtx: 0, application: 2049, quiet: false, frames: 6 },
        Stage { name: "return-cbr", mono_bitrate: 48_000, bandwidth: 1103, vbr: 0, fec: 0, loss: 0, dtx: 0, application: 2048, quiet: false, frames: 4 },
    ];
    for rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
        for kind in [Kind::Mapped] {
            let Some(encoder) = Native::new(kind, rate, &mut out) else { continue; };
            let mut decoder = Decoder::new(&encoder, &mut out);
            encoder.snapshot("fresh", &mut out);
            let samples = rate as usize / 50;
            let mut seq = 0;
            for stage in &stages {
                encoder.configure(stage, &mut out);
                for _ in 0..stage.frames {
                    let input = if stage.quiet { vec![0.0; samples * encoder.channels] } else { pcm(samples, encoder.channels, rate as u32, seq * samples) };
                    let mut packet = vec![0xa5u8; 16_384];
                    // SAFETY: Exact current20ms interleaved input and complete
                    // initialized physical output allocation for advertised size.
                    let code = unsafe { match kind {
                        Kind::Mapped => opus_multistream_encode_float(encoder.state, input.as_ptr(), samples as c_int, packet.as_mut_ptr(), packet.len() as c_int),
                        Kind::Projected => opus_projection_encode_float(encoder.state, input.as_ptr(), samples as c_int, packet.as_mut_ptr(), packet.len() as c_int),
                    }};
                    let length = code.max(0).min(packet.len() as c_int) as usize;
                    let valid_payload = code > 0 && code <= packet.len() as c_int;
                    let suffix = packet[length..].iter().all(|&x| x == 0xa5);
                    let filename = format!("{}-{rate}-{seq:03}.opus", kind.name());
                    // Save returned payload and, for every failure or dirty
                    // suffix, the complete original physical packet buffer.
                    fs::write(root.join(&filename), &packet[..length]).unwrap();
                    let full_buffer_json = if !valid_payload || !suffix {
                        let raw = format!("{}-{rate}-{seq:03}.packet-buffer.bin", kind.name());
                        fs::write(root.join(&raw), &packet).unwrap();
                        format!("{raw:?}")
                    } else { "null".to_owned() };
                    let toc_json = if valid_payload { packet[0].to_string() } else { "null".to_owned() };
                    let mut range = 0u32;
                    // SAFETY: Public unsigned-range GET on the live parent.
                    let range_code = unsafe { match kind {
                        Kind::Mapped => opus_multistream_encoder_ctl(encoder.state, FINAL_RANGE, &mut range as *mut u32),
                        Kind::Projected => opus_projection_encoder_ctl(encoder.state, FINAL_RANGE, &mut range as *mut u32),
                    }};
                    writeln!(out, "{{\"kind\":\"encode\",\"family\":{:?},\"rate\":{rate},\"stage\":{:?},\"sequence\":{seq},\"code\":{code},\"packet\":{filename:?},\"physical_bytes\":{},\"full_buffer\":{full_buffer_json},\"toc\":{toc_json},\"suffix_unchanged_observed\":{suffix},\"range_code\":{range_code},\"range\":{range}}}", kind.name(), stage.name, packet.len()).unwrap();
                    if valid_payload {
                        if let Some(decoder) = &mut decoder { decoder.decode(&encoder, &packet[..length], seq, root, &mut out); }
                    }
                    seq += 1;
                }
            }
            encoder.reset(&mut out);
        }
    }
    // No assertion infers actualLBRR, nativebatchatomicity, integer clipping,
    // automatic bitrate-allocation equivalence, or waveform/quality parity.
}
