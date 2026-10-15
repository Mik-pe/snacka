// SPDX-License-Identifier: MIT
//! Public encoder throughput at matched PCM, packet budgets and wire modes.

#[cfg(not(feature = "reference-tests"))]
use super::measure;
use super::{BATCHES, Measurement, Options, Signal, WARMUP, black_box, inputs_for};
#[cfg(feature = "reference-tests")]
use super::{measure_pair, reference};
use snacka::opus::{
    Application, Bandwidth, Encoder, EncoderConfig, FrameDuration, Mode, Packet, RateControl,
};

#[derive(Clone, Copy)]
pub(super) struct Case {
    mode: Mode,
    bandwidth: Bandwidth,
    duration: FrameDuration,
    channels: usize,
    signal: Signal,
    requested_override: Option<u32>,
}

pub(super) fn parse_case(arguments: &mut impl Iterator<Item = String>) -> Case {
    let mut next = || {
        arguments
            .next()
            .expect("--encoder-case MODE BW MS CHANNELS BITRATE")
    };
    let mode = match next().as_str() {
        "silk" => Mode::Silk,
        "hybrid" => Mode::Hybrid,
        "celt" => Mode::Celt,
        _ => panic!("encoder mode must be silk, hybrid or celt"),
    };
    let bandwidth = match next().as_str() {
        "NB" => Bandwidth::Narrowband,
        "MB" => Bandwidth::Mediumband,
        "WB" => Bandwidth::Wideband,
        "SWB" => Bandwidth::Superwideband,
        "FB" => Bandwidth::Fullband,
        _ => panic!("invalid encoder bandwidth"),
    };
    let duration = match next().as_str() {
        "2.5" => FrameDuration::Ms2_5,
        "5" => FrameDuration::Ms5,
        "10" => FrameDuration::Ms10,
        "20" => FrameDuration::Ms20,
        "40" => FrameDuration::Ms40,
        "60" => FrameDuration::Ms60,
        "80" => FrameDuration::Ms80,
        "100" => FrameDuration::Ms100,
        "120" => FrameDuration::Ms120,
        _ => panic!("invalid encoder duration"),
    };
    Case {
        mode,
        bandwidth,
        duration,
        channels: next().parse().expect("encoder channels"),
        signal: Signal::Voiced,
        requested_override: Some(next().parse().expect("encoder bitrate")),
    }
}

impl Case {
    fn requested_bitrate(self) -> u32 {
        if let Some(bitrate) = self.requested_override {
            return bitrate;
        }
        let mono = match (self.mode, self.bandwidth) {
            (Mode::Silk, Bandwidth::Narrowband) => 12_000,
            (Mode::Silk, Bandwidth::Mediumband) => 16_000,
            (Mode::Silk, _) => 24_000,
            _ => 32_000,
        };
        mono * self.channels as u32
    }

    fn bytes(self) -> usize {
        let numerator = u64::from(self.requested_bitrate()) * self.duration.samples() as u64;
        ((numerator + 192_000) / 384_000) as usize
    }

    fn bitrate(self) -> u32 {
        (self.bytes() * 384_000 / self.duration.samples()) as u32
    }

    fn config(self) -> EncoderConfig {
        EncoderConfig::new(
            self.bitrate(),
            self.channels,
            self.duration,
            self.bandwidth,
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
    }

    fn mode_name(self) -> &'static str {
        match self.mode {
            Mode::Silk => "silk",
            Mode::Hybrid => "hybrid",
            Mode::Celt => "celt",
        }
    }

    fn bandwidth_name(self) -> &'static str {
        match self.bandwidth {
            Bandwidth::Narrowband => "narrowband",
            Bandwidth::Mediumband => "mediumband",
            Bandwidth::Wideband => "wideband",
            Bandwidth::Superwideband => "superwideband",
            Bandwidth::Fullband => "fullband",
        }
    }

    fn milliseconds(self) -> &'static str {
        match self.duration {
            FrameDuration::Ms2_5 => "2.5",
            FrameDuration::Ms5 => "5",
            FrameDuration::Ms10 => "10",
            FrameDuration::Ms20 => "20",
            FrameDuration::Ms40 => "40",
            FrameDuration::Ms60 => "60",
            FrameDuration::Ms80 => "80",
            FrameDuration::Ms100 => "100",
            FrameDuration::Ms120 => "120",
        }
    }

    fn check_packet(self, bytes: &[u8]) {
        assert_eq!(bytes.len(), self.bytes(), "matched full-packet CBR budget");
        let packet = Packet::parse(bytes).unwrap();
        assert_eq!(
            packet.toc().mode(),
            self.mode,
            "encoder selected another mode"
        );
        assert_eq!(packet.toc().bandwidth(), self.bandwidth);
        assert_eq!(usize::from(packet.toc().channels()), self.channels);
        assert_eq!(packet.samples(48_000).unwrap(), self.duration.samples());
    }
}

fn row(case: Case, codec: &str, complexity: &str, result: Measurement) {
    let ratio = result
        .paired_ratio
        .map(|values| format!("{:.6},{:.6},{:.6}", values[0], values[1], values[2]))
        .unwrap_or_else(|| "NA,NA,NA".to_owned());
    println!(
        "{codec},encode,48000,{},{},{},{},{},{},{},{},{complexity},{},{BATCHES},{:.2},{:.2},{:.2},{ratio}",
        case.channels,
        case.milliseconds(),
        case.requested_bitrate(),
        case.bitrate(),
        case.bytes(),
        case.mode_name(),
        case.bandwidth_name(),
        case.signal.name(),
        result.batch,
        result.p50,
        result.p95,
        result.p99,
    );
}

fn run_case(case: Case, options: &Options) {
    if options.validate_only {
        #[cfg(feature = "reference-tests")]
        validate_case(case, options);
        return;
    }
    let input = inputs_for(case.duration.samples(), case.channels, case.signal, options);
    let mut native = Encoder::new(case.config()).unwrap();
    let mut native_packet = vec![0; case.bytes()];
    #[cfg(feature = "reference-tests")]
    let mut reference = reference::Encoder::configured(
        case.duration.samples(),
        case.channels,
        case.bitrate(),
        case.bandwidth,
        if case.mode == Mode::Celt { 2051 } else { 2048 },
        case.mode != Mode::Celt,
    );
    #[cfg(feature = "reference-tests")]
    let mut reference_packet = vec![0; case.bytes()];
    #[cfg(feature = "reference-tests")]
    let mut peer = reference::Decoder::configured(case.duration.samples(), case.channels);
    #[cfg(feature = "reference-tests")]
    let mut peer_pcm = vec![0.0; case.duration.samples() * case.channels];
    // A separate validation pass checks actual foreign mode selection and all
    // packet geometry. Both histories advance through identical PCM indexes.
    for pcm in &input {
        let length = native.encode(pcm, &mut native_packet).unwrap();
        case.check_packet(&native_packet[..length]);
        #[cfg(feature = "reference-tests")]
        {
            assert_eq!(
                peer.decode(&native_packet[..length], &mut peer_pcm),
                case.duration.samples()
            );
            assert_eq!(
                peer.final_range(),
                native.final_range(),
                "foreign decode of native packet disagrees on entropy range"
            );
            assert!(peer_pcm.iter().all(|sample| sample.is_finite()));
            let length = reference.encode(pcm, &mut reference_packet);
            case.check_packet(&reference_packet[..length]);
        }
    }
    #[cfg(feature = "reference-tests")]
    {
        let [owned_time, reference_time] = measure_pair(
            |index| {
                let length = native
                    .encode(black_box(&input[index]), &mut native_packet)
                    .unwrap();
                assert_eq!(length, case.bytes());
                black_box(&native_packet[..length]);
            },
            |index| {
                let length = reference.encode(black_box(&input[index]), &mut reference_packet);
                assert_eq!(length, case.bytes());
                black_box(&reference_packet[..length]);
            },
            input.len(),
        );
        row(case, "snacka", "fixed", owned_time);
        row(case, "libopus", "10", reference_time);
    }
    #[cfg(not(feature = "reference-tests"))]
    {
        let mut index = 0;
        let result = measure(|| {
            let length = native
                .encode(black_box(&input[index]), &mut native_packet)
                .unwrap();
            assert_eq!(length, case.bytes());
            black_box(&native_packet[..length]);
            index = (index + 1) % input.len();
        });
        row(case, "snacka", "fixed", result);
    }
}

#[cfg(feature = "reference-tests")]
fn validate_case(case: Case, options: &Options) {
    use snacka::opus::Decoder;

    let samples = case.duration.samples();
    let input = inputs_for(samples, case.channels, case.signal, options);
    let mut owned = Encoder::new(case.config()).unwrap();
    let mut reference = reference::Encoder::configured(
        samples,
        case.channels,
        case.bitrate(),
        case.bandwidth,
        if case.mode == Mode::Celt { 2051 } else { 2048 },
        case.mode != Mode::Celt,
    );
    // Independent histories for each encoder's packets in each decoder.
    let mut owned_decoders = [
        Decoder::new(case.channels).unwrap(),
        Decoder::new(case.channels).unwrap(),
    ];
    let mut reference_decoders = [
        reference::Decoder::configured(samples, case.channels),
        reference::Decoder::configured(samples, case.channels),
    ];
    let mut owned_packet = vec![0; case.bytes()];
    let mut reference_packet = vec![0; case.bytes()];
    let mut owned_pcm = vec![0.0; samples * case.channels];
    let mut reference_pcm = vec![0.0; samples * case.channels];
    for pcm in &input {
        let owned_length = owned.encode(pcm, &mut owned_packet).unwrap();
        let reference_length = reference.encode(pcm, &mut reference_packet);
        for (index, (packet, range)) in [
            (&owned_packet[..owned_length], owned.final_range()),
            (
                &reference_packet[..reference_length],
                reference.final_range(),
            ),
        ]
        .into_iter()
        .enumerate()
        {
            case.check_packet(packet);
            assert_eq!(
                owned_decoders[index]
                    .decode(packet, &mut owned_pcm)
                    .unwrap(),
                samples
            );
            assert_eq!(
                reference_decoders[index].decode(packet, &mut reference_pcm),
                samples
            );
            assert_eq!(
                owned_decoders[index].final_range(),
                range,
                "owned decode entropy range, encoder={index}"
            );
            assert_eq!(
                reference_decoders[index].final_range(),
                range,
                "reference decode entropy range, encoder={index}"
            );
            assert!(owned_pcm.iter().all(|sample| sample.is_finite()));
            assert!(reference_pcm.iter().all(|sample| sample.is_finite()));
        }
    }
    println!(
        "validation,48000,{},{},{},{},{},{},{},{},{},{},PASS",
        case.channels,
        case.milliseconds(),
        case.requested_bitrate(),
        case.bitrate(),
        case.bytes(),
        case.mode_name(),
        case.bandwidth_name(),
        case.signal.name(),
        input.len(),
        input.len() * 4,
    );
}

pub(super) fn run(options: &Options) {
    if options.validate_only {
        eprintln!(
            "48 kHz public encoder validation; {} changing PCM frames; no timers, warmups, calibration or batches",
            options.frames
        );
    } else {
        eprintln!(
            "48 kHz public encoder; {} changing PCM frames; {WARMUP} warmups; {BATCHES} calibrated >=200 us batches; ratios use paired batch-average throughput, not single-frame tail latency",
            options.frames
        );
    }
    eprintln!(
        "architecture={},cpu_label={}; CBR/equal raw bytes; reference complexity10, VOIP/voice hint for SILK+Hybrid, RestrictedLowDelay for CELT; actual wire modes verified; equal bytes does not establish equal perceptual quality",
        std::env::consts::ARCH,
        std::env::var("SNACKA_BENCH_CPU_LABEL").unwrap_or_else(|_| "unspecified".to_owned())
    );
    #[cfg(feature = "reference-tests")]
    eprintln!("reference_version={}", reference::version());
    if options.validate_only {
        println!(
            "operation,sample_rate,channels,frame_ms,requested_bitrate,packet_bitrate,packet_bytes,wire_mode,bandwidth,signal,packets_per_encoder,decode_checks,result"
        );
    } else {
        println!(
            "codec,operation,sample_rate,channels,frame_ms,requested_bitrate,packet_bitrate,packet_bytes,wire_mode,bandwidth,signal,complexity,batch_frames,batches,p50_batch_ns_per_frame,p95_batch_ns_per_frame,p99_batch_ns_per_frame,paired_encode_ratio_p50,paired_encode_ratio_p95,paired_encode_ratio_p99"
        );
    }
    let signals: &[Signal] = if options.input.is_some() {
        &[Signal::File]
    } else if options.full {
        &[Signal::Voiced, Signal::Noise, Signal::Transient]
    } else {
        &[Signal::Voiced]
    };
    if let Some(case) = options.encoder_case {
        for &signal in signals {
            run_case(Case { signal, ..case }, options);
        }
        return;
    }
    let all = [
        FrameDuration::Ms2_5,
        FrameDuration::Ms5,
        FrameDuration::Ms10,
        FrameDuration::Ms20,
        FrameDuration::Ms40,
        FrameDuration::Ms60,
        FrameDuration::Ms80,
        FrameDuration::Ms100,
        FrameDuration::Ms120,
    ];
    for (mode, bands) in [
        (
            Mode::Silk,
            &[
                Bandwidth::Narrowband,
                Bandwidth::Mediumband,
                Bandwidth::Wideband,
            ][..],
        ),
        (
            Mode::Hybrid,
            &[Bandwidth::Superwideband, Bandwidth::Fullband][..],
        ),
        (
            Mode::Celt,
            &[
                Bandwidth::Narrowband,
                Bandwidth::Wideband,
                Bandwidth::Superwideband,
                Bandwidth::Fullband,
            ][..],
        ),
    ] {
        for &duration in &all {
            if mode != Mode::Celt && duration.samples() < 480 {
                continue;
            }
            for &bandwidth in bands {
                for channels in [1, 2] {
                    if options.quick
                        && (duration != FrameDuration::Ms20
                            || channels != 1
                            || bandwidth
                                != if mode == Mode::Silk {
                                    Bandwidth::Narrowband
                                } else {
                                    Bandwidth::Fullband
                                })
                    {
                        continue;
                    }
                    for &signal in signals {
                        run_case(
                            Case {
                                mode,
                                bandwidth,
                                duration,
                                channels,
                                signal,
                                requested_override: None,
                            },
                            options,
                        );
                    }
                }
            }
        }
    }
}
