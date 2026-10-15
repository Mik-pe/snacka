use snacka::opus::{
    Application, Bandwidth, Encoder, EncoderConfig, FrameDuration, Mode, Packet, RateControl,
    multistream::{ChannelMapping, MultistreamEncoder, MultistreamPacket},
    projection::{ProjectionEncoder, ProjectionMapping},
};
use std::{
    error::Error,
    fmt::Debug,
    fs::{self, File},
    io::Write,
    path::{Path, PathBuf},
    time::Instant,
};

const ARTIFACTS: &str = "/tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015";
const QOS_USER_INITIATED: u32 = 0x19;
#[link(name = "System")]
unsafe extern "C" {
    fn pthread_self() -> *mut std::ffi::c_void;
    fn pthread_set_qos_class_self_np(class: u32, relative_priority: i32) -> i32;
    fn pthread_get_qos_class_np(
        thread: *mut std::ffi::c_void,
        class: *mut u32,
        relative_priority: *mut i32,
    ) -> i32;
}
#[derive(Clone, Copy)]
enum Kind {
    Ordinary,
    Mapped,
    Projection,
}
struct Golden {
    bytes: Vec<u8>,
    range: Option<u32>,
}
struct Case {
    name: String,
    kind: Kind,
    configs: Vec<EncoderConfig>,
    input: Vec<Vec<f32>>,
    golden: Vec<Golden>,
    period_ns: u64,
}
struct Sample {
    profile: usize,
    pass: usize,
    packet: usize,
    accepted: bool,
    nanos: Option<u64>,
    bytes: usize,
    range: u32,
    hash: u64,
    error: String,
    reset: bool,
}
struct Options {
    measure: bool,
    repeats: usize,
    role: String,
    output: PathBuf,
}
fn config(
    rate: u32,
    bits: u32,
    channels: usize,
    duration: FrameDuration,
    bw: Bandwidth,
    fec: bool,
    variable: bool,
) -> EncoderConfig {
    EncoderConfig::new(bits, channels, duration, bw, Application::Voice)
        .unwrap()
        .with_sample_rate(rate)
        .unwrap()
        .with_mode(Mode::Silk)
        .unwrap()
        .with_rate_control(if variable {
            RateControl::Variable
        } else {
            RateControl::Constant
        })
        .with_fec(fec)
        .unwrap()
}
fn synthetic(samples: usize, channels: usize, rate: u32, packet: usize, clip: bool) -> Vec<f32> {
    (0..samples * channels)
        .map(|position| {
            let channel = position % channels;
            let sample = position / channels;
            if clip {
                let width = (rate as usize / 2000).max(1);
                match (sample / width + packet + channel * 3 + sample / (width * (5 + channel * 2)))
                    % 8
                {
                    0 | 1 => 1.0,
                    2 | 3 => -1.0,
                    4 => f32::from_bits(0x3f7fffff),
                    5 => -f32::from_bits(0x3f7fffff),
                    6 => 0.0,
                    _ => -0.0,
                }
            } else {
                let t = (packet * samples + sample) as f64 / f64::from(rate);
                (0.03 * (std::f64::consts::TAU * (137.0 + channel as f64 * 43.0) * t).sin()) as f32
            }
        })
        .collect()
}
fn golden(path: &Path, raw: bool) -> Vec<Golden> {
    let data = fs::read(path).unwrap();
    let mut offset = 0;
    let mut records = Vec::new();
    while offset < data.len() {
        let n = u32::from_le_bytes(data[offset..offset + 4].try_into().unwrap()) as usize;
        let range = if raw {
            None
        } else {
            Some(u32::from_le_bytes(
                data[offset + 4..offset + 8].try_into().unwrap(),
            ))
        };
        offset += if raw { 4 } else { 20 };
        records.push(Golden {
            bytes: data[offset..offset + n].to_vec(),
            range,
        });
        offset += n;
    }
    assert_eq!(offset, data.len());
    records
}
// Same10 old-success configurations and64-frame generated voice formula as
// /tmp/snacka-opus-silk-sparse-cpu-20261015/throughput.rs. Each pass starts
// a fresh encoder and plays this same complete input history once.
fn normal_cases() -> Vec<Case> {
    use Bandwidth::*;
    use FrameDuration::*;
    use Mode::*;
    [
        ("nb10m12", Silk, Narrowband, Ms10, 1, 12000, false),
        ("nb10s24", Silk, Narrowband, Ms10, 2, 24000, false),
        ("mb20m16", Silk, Mediumband, Ms20, 1, 16000, false),
        ("wb10m24", Silk, Wideband, Ms10, 1, 24000, false),
        ("wb20m12", Silk, Wideband, Ms20, 1, 12000, false),
        ("wb20s48", Silk, Wideband, Ms20, 2, 48000, false),
        ("wb20m24fec", Silk, Wideband, Ms20, 1, 24000, true),
        ("wb60s48fec", Silk, Wideband, Ms60, 2, 48000, true),
        ("hybrid20m32fec", Hybrid, Fullband, Ms20, 1, 32000, true),
        ("celt20m32", Celt, Fullband, Ms20, 1, 32000, false),
    ]
    .into_iter()
    .map(|(name, mode, bw, duration, channels, bits, fec)| {
        let settings = EncoderConfig::new(
            bits,
            channels,
            duration,
            bw,
            if mode == Celt {
                Application::LowDelay
            } else {
                Application::Voice
            },
        )
        .unwrap()
        .with_mode(mode)
        .unwrap()
        .with_rate_control(RateControl::Constant)
        .with_fec(fec)
        .unwrap();
        let input = (0..64)
            .map(|frame| {
                (0..duration.samples() * channels)
                    .map(|p| {
                        let n = frame * duration.samples() + p / channels;
                        let ch = (p % channels) as f64;
                        let t = n as f64 / 48000.;
                        let phase = std::f64::consts::TAU * (137. * t + 4. * t * t)
                            + ch * (0.4 + (t * 2.).sin());
                        (0.32 * phase.sin()
                            + 0.08 * (2. * phase).sin()
                            + 0.006 * (31. * phase + ch).sin()) as f32
                    })
                    .collect()
            })
            .collect();
        Case {
            name: format!("normal-{name}"),
            kind: Kind::Ordinary,
            configs: vec![settings],
            input,
            golden: Vec::new(),
            period_ns: duration.samples() as u64 * 1_000_000_000 / 48000,
        }
    })
    .collect()
}
fn prepare_normal_goldens() -> Result<(), Box<dyn Error>> {
    // Run only with the baseline-linked adapter. Golden construction is untimed;
    // both frozen roles subsequently consume the same baseline packets/ranges.
    let directory = Path::new(ARTIFACTS).join("controls/callback-latency/normal-golden");
    fs::create_dir_all(&directory)?;
    for case in normal_cases() {
        let mut codec = Encoder::new(case.configs[0])?;
        let mut packet = vec![0; case.configs[0].max_packet_bytes()];
        let mut archive = File::create(directory.join(format!("{}.packets", case.name)))?;
        for input in &case.input {
            let bytes = codec.encode(input, &mut packet)?;
            for field in [bytes as u32, codec.final_range(), 0, 0, 0] {
                archive.write_all(&field.to_le_bytes())?;
            }
            archive.write_all(&packet[..bytes])?;
        }
    }
    println!("shared baseline normal goldens:10 profiles/640 packets, untimed");
    Ok(())
}
fn prepare(role: &str) -> Vec<Case> {
    let root = Path::new(ARTIFACTS);
    let mut cases = Vec::new();
    for (bw, label, native) in [
        (Bandwidth::Narrowband, "nb", 8000),
        (Bandwidth::Wideband, "wb", 16000),
    ] {
        for rate in [48000, native] {
            for channels in [1, 2] {
                for duration in [
                    FrameDuration::Ms10,
                    FrameDuration::Ms20,
                    FrameDuration::Ms40,
                    FrameDuration::Ms60,
                    FrameDuration::Ms80,
                    FrameDuration::Ms100,
                    FrameDuration::Ms120,
                ] {
                    let bits = 12000 * channels as u32;
                    let name = format!(
                        "clip-{label}-{}ms-{channels}ch-input{rate}-{bits}",
                        duration.samples() / 48
                    );
                    let settings = config(rate, bits, channels, duration, bw, false, false);
                    let input = (0..8)
                        .map(|i| synthetic(settings.input_samples(), channels, rate, i, true))
                        .collect();
                    let expected = golden(
                        &root.join(format!("controls/fallback-{role}-data/{name}.packets")),
                        false,
                    );
                    cases.push(Case {
                        name,
                        kind: Kind::Ordinary,
                        configs: vec![settings],
                        input,
                        golden: expected,
                        period_ns: duration.samples() as u64 * 1_000_000_000 / 48000,
                    });
                }
            }
        }
    }
    let name = "fec-carrier-nb10-20k".to_string();
    let settings = config(
        48000,
        20000,
        1,
        FrameDuration::Ms10,
        Bandwidth::Narrowband,
        true,
        false,
    );
    let input = (0..14)
        .map(|i| synthetic(settings.input_samples(), 1, 48000, i, i == 6))
        .collect();
    let expected = golden(
        &root.join(format!("controls/fallback-{role}-data/{name}.packets")),
        false,
    );
    cases.push(Case {
        name,
        kind: Kind::Ordinary,
        configs: vec![settings],
        input,
        golden: expected,
        period_ns: 10_000_000,
    });
    for (label, bits, duration, bw) in [
        ("nb", 12000, FrameDuration::Ms10, Bandwidth::Narrowband),
        ("wb", 6000, FrameDuration::Ms20, Bandwidth::Wideband),
    ] {
        let settings = config(48000, bits, 1, duration, bw, false, false);
        let bytes = fs::read(root.join(format!("refusal-sparse/{label}.input.f32"))).unwrap();
        let values: Vec<f32> = bytes
            .chunks_exact(4)
            .map(|b| f32::from_le_bytes(b.try_into().unwrap()))
            .collect();
        let input = values
            .chunks_exact(settings.input_samples())
            .map(<[f32]>::to_vec)
            .collect();
        let source = if role == "candidate" {
            "refusal-sparse"
        } else {
            "refusal-baseline"
        };
        let mut expected = golden(&root.join(format!("{source}/{label}.packets")), true);
        let ranges_path = root.join(format!("{source}/{label}.ranges"));
        if ranges_path.exists() {
            let ranges = fs::read(ranges_path).unwrap();
            assert_eq!(ranges.len(), expected.len() * 4);
            for (record, b) in expected.iter_mut().zip(ranges.chunks_exact(4)) {
                record.range = Some(u32::from_le_bytes(b.try_into().unwrap()));
            }
        }
        cases.push(Case {
            name: format!("original-{label}-refusal"),
            kind: Kind::Ordinary,
            configs: vec![settings],
            input,
            golden: expected,
            period_ns: duration.samples() as u64 * 1_000_000_000 / 48000,
        });
    }
    for bits in [12000, 16000] {
        for projection in [false, true] {
            let channels = if projection { 4 } else { 5 };
            let name = format!(
                "{}-fallback-{bits}",
                if projection { "projection" } else { "mapped" }
            );
            let configs = if projection {
                vec![
                    config(
                        48000,
                        24000,
                        2,
                        FrameDuration::Ms10,
                        Bandwidth::Narrowband,
                        false,
                        true,
                    ),
                    config(
                        48000,
                        bits,
                        1,
                        FrameDuration::Ms10,
                        Bandwidth::Narrowband,
                        false,
                        true,
                    ),
                    config(
                        48000,
                        bits,
                        1,
                        FrameDuration::Ms10,
                        Bandwidth::Narrowband,
                        false,
                        true,
                    ),
                ]
            } else {
                vec![
                    config(
                        48000,
                        bits,
                        1,
                        FrameDuration::Ms10,
                        Bandwidth::Narrowband,
                        false,
                        true
                    );
                    3
                ]
            };
            let input = (0..16)
                .map(|i| synthetic(480, channels, 48000, i, true))
                .collect();
            let expected = golden(
                &root.join(format!("controls/composition-{role}-data/{name}.packets")),
                false,
            );
            cases.push(Case {
                name,
                kind: if projection {
                    Kind::Projection
                } else {
                    Kind::Mapped
                },
                configs,
                input,
                golden: expected,
                period_ns: 10_000_000,
            });
        }
    }
    assert_eq!(cases.len(), 63);
    for mut case in normal_cases() {
        case.golden = golden(
            &root.join(format!(
                "controls/callback-latency/normal-golden/{}.packets",
                case.name
            )),
            false,
        );
        assert_eq!(case.golden.len(), 64);
        cases.push(case);
    }
    assert_eq!(cases.len(), 73);
    cases
}
#[inline]
fn invoke<E>(
    measure: bool,
    encode: impl FnOnce() -> Result<usize, E>,
) -> (Result<usize, E>, Option<u64>) {
    if measure {
        let start = Instant::now();
        let result = encode();
        let nanos = start.elapsed().as_nanos() as u64;
        (result, Some(nanos))
    } else {
        (encode(), None)
    }
}
fn fingerprint(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x100000001b3)
    })
}
fn context<C, E: Debug>(
    case: &Case,
    profile: usize,
    pass: usize,
    measure: bool,
    codec: &mut C,
    capacity: usize,
    mut encode: impl FnMut(&mut C, &[f32], &mut [u8]) -> Result<usize, E>,
    range: impl Fn(&C) -> u32,
    reset: impl Fn(&mut C),
    records: &mut Vec<Sample>,
) {
    let mut packet = vec![0xa5; capacity + 1];
    let mut accepted = 0;
    for (index, input) in case.input.iter().enumerate() {
        let reset_before_callback = index == 8 && !matches!(case.kind, Kind::Ordinary);
        if reset_before_callback {
            reset(codec);
        }
        // The sole timed interval: one public encode callback. Inputs, codec
        // construction, packet capacity, metadata, golden checks and I/O are outside.
        let (result, nanos) = invoke(measure, || encode(codec, input, &mut packet[..capacity]));
        match result {
            Ok(bytes) => {
                assert_eq!(packet[capacity], 0xa5);
                let interval = range(codec);
                let expected = case
                    .golden
                    .get(index)
                    .expect("success beyond frozen role's expected prefix");
                assert_eq!(&packet[..bytes], expected.bytes);
                if let Some(expected) = expected.range {
                    assert_eq!(interval, expected);
                }
                match case.kind {
                    Kind::Ordinary => {
                        let parsed = Packet::parse(&packet[..bytes]).unwrap();
                        assert_eq!(
                            parsed.samples(48000).unwrap(),
                            case.configs[0].duration().samples()
                        );
                    }
                    _ => {
                        let parsed = MultistreamPacket::parse(&packet[..bytes], 3).unwrap();
                        assert_eq!(parsed.samples(48000).unwrap(), 480);
                    }
                }
                records.push(Sample {
                    profile,
                    pass,
                    packet: index,
                    accepted: true,
                    nanos,
                    bytes,
                    range: interval,
                    hash: fingerprint(&packet[..bytes]),
                    error: String::new(),
                    reset: reset_before_callback,
                });
                accepted += 1;
            }
            Err(error) => {
                let error = format!("{error:?}");
                assert_eq!(
                    error,
                    match case.kind {
                        Kind::Ordinary => "Output",
                        Kind::Mapped | Kind::Projection => "Encode(Output)",
                    },
                    "unexpected refusal class"
                );
                assert_eq!(
                    index,
                    case.golden.len(),
                    "refusal does not match frozen role's boundary"
                );
                records.push(Sample {
                    profile,
                    pass,
                    packet: index,
                    accepted: false,
                    nanos,
                    bytes: 0,
                    range: 0,
                    hash: 0,
                    error,
                    reset: reset_before_callback,
                });
                break;
            }
        }
    }
    assert_eq!(accepted, case.golden.len());
}
fn projection() -> (ProjectionMapping, Vec<f32>) {
    // Same signed permutation as the functional proof, avoiding mix overload.
    let rows = [
        [0.0f64, 1.0, 0.0, 0.0],
        [0.0, 0.0, -1.0, 0.0],
        [1.0, 0.0, 0.0, 0.0],
        [0.0, 0.0, 0.0, -1.0],
    ];
    let demixing = rows
        .iter()
        .flat_map(|row| {
            row.iter()
                .map(|&x| (x * 32768.0).clamp(-32768.0, 32767.0) as i16)
        })
        .collect();
    let mixing = (0..4)
        .flat_map(|source| rows.into_iter().map(move |row| row[source] as f32))
        .collect();
    (ProjectionMapping::new(3, 1, 4, demixing).unwrap(), mixing)
}
fn options() -> Options {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let verify = args.iter().any(|x| x == "--verify");
    let measure = args.iter().any(|x| x == "--measure");
    assert_ne!(
        verify, measure,
        "choose exactly one of --verify or --measure"
    );
    let value = |flag: &str| {
        args.iter()
            .position(|x| x == flag)
            .map(|i| args.get(i + 1).expect("missing value").clone())
    };
    let role = value("--role").expect("--role baseline|candidate required");
    assert!(role == "baseline" || role == "candidate");
    let repeats = value("--repeats")
        .map(|s| s.parse().unwrap())
        .unwrap_or(if verify { 1 } else { 16 });
    assert!(repeats > 0);
    Options {
        measure,
        repeats,
        role,
        output: value("--output").expect("--output required").into(),
    }
}
fn percentile(values: &[u64], p: usize) -> Option<u64> {
    if values.is_empty() {
        None
    } else {
        Some(values[(values.len() * p).div_ceil(100).max(1) - 1])
    }
}
fn number(value: Option<u64>) -> String {
    value.map_or("null".into(), |x| x.to_string())
}
fn run(options: Options) -> Result<(), Box<dyn Error>> {
    // This external test FFI does not enter production code. The same requested
    // QoS and relative priority apply to every baseline/candidate callback.
    let set = unsafe { pthread_set_qos_class_self_np(QOS_USER_INITIATED, 0) };
    let mut qos = 0;
    let mut priority = 0;
    let get = unsafe { pthread_get_qos_class_np(pthread_self(), &mut qos, &mut priority) };
    assert_eq!((set, get, qos, priority), (0, 0, QOS_USER_INITIATED, 0));
    let cases = prepare(&options.role);
    let capacity = cases.iter().map(|c| c.input.len() + 1).sum::<usize>() * options.repeats;
    let mut records = Vec::with_capacity(capacity);
    for (profile, case) in cases.iter().enumerate() {
        for pass in 0..options.repeats {
            match case.kind {
                Kind::Ordinary => {
                    let mut codec = Encoder::new(case.configs[0])?;
                    let capacity = case.configs[0].max_packet_bytes();
                    context(
                        case,
                        profile,
                        pass,
                        options.measure,
                        &mut codec,
                        capacity,
                        Encoder::encode,
                        Encoder::final_range,
                        Encoder::reset,
                        &mut records,
                    );
                }
                Kind::Mapped => {
                    let mut codec = MultistreamEncoder::new(
                        ChannelMapping::new(3, 0, &[2, 0, 1, 0, 255])?,
                        &case.configs,
                    )?;
                    let capacity = codec.max_packet_bytes();
                    context(
                        case,
                        profile,
                        pass,
                        options.measure,
                        &mut codec,
                        capacity,
                        MultistreamEncoder::encode,
                        MultistreamEncoder::final_range,
                        MultistreamEncoder::reset,
                        &mut records,
                    );
                }
                Kind::Projection => {
                    let (mapping, mixing) = projection();
                    let mut codec = ProjectionEncoder::new(mapping, mixing, &case.configs)?;
                    let capacity = codec.max_packet_bytes();
                    context(
                        case,
                        profile,
                        pass,
                        options.measure,
                        &mut codec,
                        capacity,
                        ProjectionEncoder::encode,
                        ProjectionEncoder::final_range,
                        ProjectionEncoder::reset,
                        &mut records,
                    );
                }
            }
        }
    }
    fs::create_dir_all(&options.output)?;
    // Export exact prepared PCM once per profile after all callbacks. Hashing this
    // file plus per-profile byte spans binds every fresh context's input history.
    let mut input_pcm = File::create(options.output.join("inputs.f32"))?;
    let mut input_index = File::create(options.output.join("inputs.csv"))?;
    writeln!(input_index, "profile,packet,byte_offset,samples,period_ns")?;
    let mut byte_offset = 0u64;
    for case in &cases {
        for (packet, input) in case.input.iter().enumerate() {
            writeln!(
                input_index,
                "{},{},{},{},{}",
                case.name,
                packet,
                byte_offset,
                input.len(),
                case.period_ns
            )?;
            for sample in input {
                input_pcm.write_all(&sample.to_le_bytes())?;
            }
            byte_offset += input.len() as u64 * 4;
        }
    }
    let mut samples = File::create(options.output.join("callbacks.csv"))?;
    writeln!(
        samples,
        "profile,pass,packet,first_callback,reset_before_callback,accepted,period_ns,elapsed_ns,overrun,bytes,final_range,packet_fnv1a,error"
    )?;
    for sample in &records {
        let case = &cases[sample.profile];
        writeln!(
            samples,
            "{},{},{},{},{},{},{},{},{},{},{},{:016x},{}",
            case.name,
            sample.pass,
            sample.packet,
            sample.packet == 0,
            sample.reset,
            sample.accepted,
            case.period_ns,
            sample.nanos.map_or(String::new(), |x| x.to_string()),
            sample.nanos.is_some_and(|ns| ns > case.period_ns),
            sample.bytes,
            sample.range,
            sample.hash,
            sample.error
        )?;
    }
    let mut report = File::create(options.output.join("summary.json"))?;
    writeln!(
        report,
        "{{\"role\":\"{}\",\"mode\":\"{}\",\"timing_recorded\":{},\"repeats\":{},\"profiles\":{},\"successful_callbacks\":{},\"refused_callbacks\":{},\"stack_bytes\":1048576,\"qos_requested\":25,\"qos_set_return\":{set},\"qos_get_return\":{get},\"qos_queried\":{qos},\"qos_relative_priority\":{priority},\"compiler_settings\":\"direct Rust1.99; opt3; CGU16; overflow checks; no targetCPU/LTO/incremental\",\"samples\":[",
        options.role,
        if options.measure { "measure" } else { "verify" },
        options.measure,
        options.repeats,
        cases.len(),
        records.iter().filter(|x| x.accepted).count(),
        records.iter().filter(|x| !x.accepted).count()
    )?;
    let mut comma = false;
    for (profile, case) in cases.iter().enumerate() {
        for phase in [
            "first-success",
            "after-reset-success",
            "steady-success",
            "refusal",
        ] {
            let matching: Vec<_> = records
                .iter()
                .filter(|s| {
                    s.profile == profile
                        && match phase {
                            "first-success" => s.accepted && s.packet == 0,
                            "after-reset-success" => s.accepted && s.reset,
                            "steady-success" => s.accepted && s.packet > 0 && !s.reset,
                            _ => !s.accepted,
                        }
                })
                .collect();
            let mut values: Vec<_> = matching.iter().filter_map(|s| s.nanos).collect();
            values.sort_unstable();
            if comma {
                writeln!(report, ",")?;
            }
            comma = true;
            write!(
                report,
                "{{\"profile\":\"{}\",\"phase\":\"{phase}\",\"callbacks\":{},\"timed_callbacks\":{},\"period_ns\":{},\"p50_ns\":{},\"p95_ns\":{},\"p99_ns\":{},\"max_ns\":{},\"overruns\":{}}}",
                case.name,
                matching.len(),
                values.len(),
                case.period_ns,
                number(percentile(&values, 50)),
                number(percentile(&values, 95)),
                number(percentile(&values, 99)),
                number(values.last().copied()),
                values.iter().filter(|&&n| n > case.period_ns).count()
            )?;
        }
    }
    writeln!(report, "]}}")?;
    if !options.measure {
        assert!(records.iter().all(|s| s.nanos.is_none()));
    }
    println!(
        "{{\"role\":\"{}\",\"timing_recorded\":{},\"profiles\":{},\"successful_callbacks\":{},\"refused_callbacks\":{},\"qos\":{qos},\"stack_bytes\":1048576}}",
        options.role,
        options.measure,
        cases.len(),
        records.iter().filter(|x| x.accepted).count(),
        records.iter().filter(|x| !x.accepted).count()
    );
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    if std::env::args().any(|x| x == "--prepare-normal") {
        return std::thread::Builder::new()
            .name("normal-golden-proof".into())
            .stack_size(1 << 20)
            .spawn(|| prepare_normal_goldens().map_err(|e| e.to_string()))?
            .join()
            .unwrap()
            .map_err(Into::into);
    }
    let options = options();
    std::thread::Builder::new()
        .name("callback-latency-proof".into())
        .stack_size(1 << 20)
        .spawn(move || run(options).map_err(|e| e.to_string()))?
        .join()
        .unwrap()
        .map_err(Into::into)
}
