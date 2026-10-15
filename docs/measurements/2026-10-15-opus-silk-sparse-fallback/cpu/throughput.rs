// External black-box benchmark; never linked into production Snacka.
use snacka::opus::{
    Application, Bandwidth, Decoder, Encoder, EncoderConfig, FrameDuration, Mode, Packet,
    RateControl, Toc,
};
use std::{
    ffi::{CStr, c_char, c_void},
    hint::black_box,
    time::Instant,
};
#[link(name = "opus")]
unsafe extern "C" {
    fn opus_encoder_create(
        rate: i32,
        channels: i32,
        application: i32,
        error: *mut i32,
    ) -> *mut c_void;
    fn opus_encoder_destroy(state: *mut c_void);
    fn opus_encoder_ctl(state: *mut c_void, request: i32, ...) -> i32;
    fn opus_encode_float(
        state: *mut c_void,
        input: *const f32,
        samples: i32,
        output: *mut u8,
        capacity: i32,
    ) -> i32;
    fn opus_get_version_string() -> *const c_char;
    fn pthread_set_qos_class_self_np(class: u32, priority: i32) -> i32;
    fn pthread_get_qos_class_np(thread: *mut c_void, class: *mut u32, priority: *mut i32) -> i32;
    fn pthread_self() -> *mut c_void;
}
#[derive(Clone, Copy)]
struct Case {
    name: &'static str,
    mode: Mode,
    bw: Bandwidth,
    ms: FrameDuration,
    ch: usize,
    bps: u32,
    fec: bool,
}
fn cases() -> [Case; 10] {
    use Bandwidth::*;
    use FrameDuration::*;
    use Mode::*;
    [
        Case {
            name: "nb10m12",
            mode: Silk,
            bw: Narrowband,
            ms: Ms10,
            ch: 1,
            bps: 12000,
            fec: false,
        },
        Case {
            name: "nb10s24",
            mode: Silk,
            bw: Narrowband,
            ms: Ms10,
            ch: 2,
            bps: 24000,
            fec: false,
        },
        Case {
            name: "mb20m16",
            mode: Silk,
            bw: Mediumband,
            ms: Ms20,
            ch: 1,
            bps: 16000,
            fec: false,
        },
        Case {
            name: "wb10m24",
            mode: Silk,
            bw: Wideband,
            ms: Ms10,
            ch: 1,
            bps: 24000,
            fec: false,
        },
        Case {
            name: "wb20m12",
            mode: Silk,
            bw: Wideband,
            ms: Ms20,
            ch: 1,
            bps: 12000,
            fec: false,
        },
        Case {
            name: "wb20s48",
            mode: Silk,
            bw: Wideband,
            ms: Ms20,
            ch: 2,
            bps: 48000,
            fec: false,
        },
        Case {
            name: "wb20m24fec",
            mode: Silk,
            bw: Wideband,
            ms: Ms20,
            ch: 1,
            bps: 24000,
            fec: true,
        },
        Case {
            name: "wb60s48fec",
            mode: Silk,
            bw: Wideband,
            ms: Ms60,
            ch: 2,
            bps: 48000,
            fec: true,
        },
        Case {
            name: "hybrid20m32fec",
            mode: Hybrid,
            bw: Fullband,
            ms: Ms20,
            ch: 1,
            bps: 32000,
            fec: true,
        },
        Case {
            name: "celt20m32",
            mode: Celt,
            bw: Fullband,
            ms: Ms20,
            ch: 1,
            bps: 32000,
            fec: false,
        },
    ]
}
impl Case {
    fn config(self) -> EncoderConfig {
        EncoderConfig::new(
            self.bps,
            self.ch,
            self.ms,
            self.bw,
            if self.mode == Mode::Celt {
                Application::LowDelay
            } else {
                Application::Voice
            },
        )
        .unwrap()
        .with_mode(self.mode)
        .unwrap()
        .with_rate_control(RateControl::Constant)
        .with_fec(self.fec)
        .unwrap()
    }
    fn bytes(self) -> usize {
        (u64::from(self.bps) * self.ms.samples() as u64 / 384000) as usize
    }
    fn toc_matches(self, t: u8) -> bool {
        let t = Toc::from_byte(t);
        t.mode() == self.mode && t.bandwidth() == self.bw && usize::from(t.channels()) == self.ch
    }
    fn input(self) -> Vec<Vec<f32>> {
        (0..64)
            .map(|frame| {
                (0..self.ms.samples() * self.ch)
                    .map(|p| {
                        let n = frame * self.ms.samples() + p / self.ch;
                        let ch = (p % self.ch) as f64;
                        let t = n as f64 / 48000.;
                        let phase = std::f64::consts::TAU * (137. * t + 4. * t * t)
                            + ch * (0.4 + (t * 2.).sin());
                        (0.32 * phase.sin()
                            + 0.08 * (2. * phase).sin()
                            + 0.006 * (31. * phase + ch).sin()) as f32
                    })
                    .collect()
            })
            .collect()
    }
}
struct Native {
    p: *mut c_void,
    n: usize,
}
impl Native {
    fn new(c: Case) -> Self {
        let mut error = 0;
        let app = match c.mode {
            Mode::Silk => 2052,
            Mode::Hybrid => 2048,
            Mode::Celt => 2051,
        };
        let p = unsafe { opus_encoder_create(48000, c.ch as i32, app, &mut error) };
        assert!(!p.is_null() && error == 0);
        let bw = match c.bw {
            Bandwidth::Narrowband => 1101,
            Bandwidth::Mediumband => 1102,
            Bandwidth::Wideband => 1103,
            Bandwidth::Superwideband => 1104,
            Bandwidth::Fullband => 1105,
        };
        for (request, value) in [
            (4002, c.bps as i32),
            (4006, 0),
            (4010, 10),
            (4008, bw),
            (4022, c.ch as i32),
            (4012, i32::from(c.fec)),
            (4014, if c.fec { 20 } else { 0 }),
            (4024, if c.mode == Mode::Celt { 3002 } else { 3001 }),
        ] {
            assert_eq!(unsafe { opus_encoder_ctl(p, request, value) }, 0);
        }
        Self {
            p,
            n: c.ms.samples(),
        }
    }
    fn encode(&mut self, input: &[f32], out: &mut [u8]) -> usize {
        let n = unsafe {
            opus_encode_float(
                self.p,
                input.as_ptr(),
                self.n as i32,
                out.as_mut_ptr(),
                out.len() as i32,
            )
        };
        assert!(n > 0, "native encode {n}");
        n as usize
    }
}
impl Drop for Native {
    fn drop(&mut self) {
        unsafe { opus_encoder_destroy(self.p) }
    }
}
const BATCHES: usize = 1000;
const WARM: usize = 128;
fn batch(f: &mut impl FnMut(usize), start: usize, n: usize) -> u64 {
    let at = Instant::now();
    for j in 0..n {
        f((start + j) % 64)
    }
    at.elapsed().as_nanos() as u64
}
fn paired(
    mut a: impl FnMut(usize),
    mut b: impl FnMut(usize),
) -> (usize, [f64; 3], [f64; 3], [f64; 3]) {
    let mut start = 0;
    for _ in 0..WARM {
        a(start);
        b(start);
        start = (start + 1) % 64;
    }
    let mut n = 1;
    loop {
        let x = batch(&mut a, start, n);
        let y = batch(&mut b, start, n);
        start = (start + n) % 64;
        let fast = x.min(y).max(1);
        if fast >= 200000 || n >= 1 << 20 {
            break;
        }
        n = ((n as u128 * 220000).div_ceil(fast as u128) as usize).clamp(n + 1, 1 << 20);
    }
    let mut av = Vec::with_capacity(BATCHES);
    let mut bv = Vec::with_capacity(BATCHES);
    let mut rv = Vec::with_capacity(BATCHES);
    for j in 0..BATCHES {
        let (x, y) = if j % 2 == 0 {
            (batch(&mut a, start, n), batch(&mut b, start, n))
        } else {
            let y = batch(&mut b, start, n);
            (batch(&mut a, start, n), y)
        };
        av.push(x as f64 / n as f64);
        bv.push(y as f64 / n as f64);
        rv.push(x as f64 / y as f64);
        start = (start + n) % 64;
    }
    for x in [&mut av, &mut bv, &mut rv] {
        x.sort_unstable_by(f64::total_cmp)
    }
    let q = |x: &[f64]| [x[500], x[950], x[990]];
    (n, q(&av), q(&bv), q(&rv))
}
fn execute(c: Case, measure: bool) {
    let input = c.input();
    let mut input_hash = 0xcbf29ce484222325u64;
    for value in input.iter().flatten() {
        for byte in value.to_bits().to_le_bytes() {
            input_hash = (input_hash ^ u64::from(byte)).wrapping_mul(0x100000001b3);
        }
    }
    if let Some(path) = std::env::var_os("SPARSE_CPU_DUMP_INPUTS") {
        let path = std::path::PathBuf::from(path);
        std::fs::create_dir_all(&path).unwrap();
        let raw: Vec<u8> = input
            .iter()
            .flatten()
            .flat_map(|v| v.to_bits().to_le_bytes())
            .collect();
        std::fs::write(path.join(format!("{}.f32", c.name)), raw).unwrap();
    }

    let mut owned = Encoder::new(c.config()).unwrap();
    let mut native = Native::new(c);
    let mut a = vec![0; c.bytes()];
    let mut b = a.clone();
    let mut decoder = Decoder::new(c.ch).unwrap();
    let mut pcm = vec![0.; c.ms.samples() * c.ch];
    let mut own_toc = [false; 256];
    let mut ref_toc = [false; 256];
    let mut own_fec = 0;
    let mut ref_fec = 0;
    // Untimed actual-wire/state validation extends over16 complete changing sequences.
    for i in 0..1024 {
        let x = &input[i % 64];
        let an = owned.encode(x, &mut a).unwrap();
        let bn = native.encode(x, &mut b);
        assert_eq!(an, c.bytes());
        assert_eq!(bn, c.bytes());
        let pa = Packet::parse(&a).unwrap();
        let pb = Packet::parse(&b).unwrap();
        assert_eq!(pa.samples(48000).unwrap(), c.ms.samples());
        assert_eq!(pb.samples(48000).unwrap(), c.ms.samples());
        assert!(c.toc_matches(a[0]));
        own_toc[a[0] as usize] = true;
        ref_toc[b[0] as usize] = true;
        own_fec += usize::from(owned.has_fec());
        if pb.toc().mode() != Mode::Celt {
            let d = match pb.frame_samples(48000).unwrap() {
                480 => snacka::opus::silk::PacketDuration::Ms10,
                960 => snacka::opus::silk::PacketDuration::Ms20,
                1920 => snacka::opus::silk::PacketDuration::Ms40,
                2880 => snacka::opus::silk::PacketDuration::Ms60,
                _ => unreachable!(),
            };
            let mut r = snacka::entropy::Decoder::new(pb.frames().next().unwrap());
            let h = snacka::opus::silk::Header::decode(&mut r, d, c.ch as u8).unwrap();
            ref_fec += usize::from(h.redundancy(0, 0) == Some(true));
        }
        assert_eq!(decoder.decode(&a, &mut pcm).unwrap(), c.ms.samples());
        assert_eq!(decoder.final_range(), owned.final_range());
    }
    let mut result = None;
    if measure {
        result = Some(paired(
            |i| {
                let n = owned.encode(black_box(&input[i]), &mut a).unwrap();
                assert_eq!(n, c.bytes());
                own_toc[a[0] as usize] = true;
                black_box(&a);
            },
            |i| {
                let n = native.encode(black_box(&input[i]), &mut b);
                assert_eq!(n, c.bytes());
                ref_toc[b[0] as usize] = true;
                black_box(&b);
            },
        ));
    }
    let tocs = |v: [bool; 256]| {
        v.iter()
            .enumerate()
            .filter_map(|(i, x)| x.then_some(i as u8))
            .collect::<Vec<_>>()
    };
    let ot = tocs(own_toc);
    let rt = tocs(ref_toc);
    let wire = rt.iter().all(|&t| c.toc_matches(t));
    let fec_availability = own_fec == ref_fec;
    println!(
        "{{\"case\":\"{}\",\"input_fnv64\":\"{input_hash:016x}\",\"operation\":\"{}\",\"qos\":\"user-initiated\",\"input_frames\":64,\"validation_callbacks\":1024,\"samples\":{},\"channels\":{},\"bitrate\":{},\"packet_bytes\":{},\"fec_enabled\":{},\"owned_fec_packets\":{},\"reference_fec_packets\":{},\"fec_availability_equal\":{},\"owned_tocs\":{:?},\"reference_tocs\":{:?},\"native_wire_match\":\"{}\",\"timed\":{}}}",
        c.name,
        if measure {
            "paired_batch_throughput"
        } else {
            "verify"
        },
        c.ms.samples(),
        c.ch,
        c.bps,
        c.bytes(),
        c.fec,
        own_fec,
        ref_fec,
        fec_availability,
        ot,
        rt,
        if wire { "MATCHED" } else { "UNMATCHED" },
        match result {
            None => "null".to_owned(),
            Some((n, a, b, r)) => format!(
                "{{\"batch_frames\":{n},\"batches\":{BATCHES},\"owned_ns\":{a:?},\"native_ns\":{b:?},\"paired_normalization_ratio\":{r:?},\"paired_owned_native_ratio\":{}}}",
                if wire && fec_availability {
                    format!("{r:?}")
                } else {
                    "null".to_owned()
                }
            ),
        }
    );
}
fn main() {
    let args: Vec<_> = std::env::args().skip(1).collect();
    assert!(
        args.first()
            .is_some_and(|s| s == "--verify" || s == "--measure")
    );
    let measure = args[0] == "--measure";
    assert_eq!(unsafe { pthread_set_qos_class_self_np(0x19, 0) }, 0);
    let (mut q, mut p) = (0, 0);
    assert_eq!(
        unsafe { pthread_get_qos_class_np(pthread_self(), &mut q, &mut p) },
        0
    );
    assert_eq!(q, 0x19);
    assert_eq!(p, 0);
    eprintln!(
        "reference={} qos_class={q} qos_relative_priority={p}",
        unsafe { CStr::from_ptr(opus_get_version_string()) }
            .to_str()
            .unwrap()
    );
    let cases = cases();
    if let Some(name) = args.get(1) {
        execute(
            *cases.iter().find(|c| c.name == name).expect("case"),
            measure,
        )
    } else {
        assert!(!measure, "one measured case per process");
        for c in cases {
            execute(c, false)
        }
    }
}
