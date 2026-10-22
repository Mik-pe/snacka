// SPDX-License-Identifier: MIT
//! Native candidate PCM and exact lost-frame band diagnostics in an isolated crate.
use snacka::{ALGORITHMIC_DELAY, Config, Decoder, Encoder, FrameDuration, MAX_PACKET_BYTES};
use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fs,
    io::{self, Write},
    path::Path,
    time::Instant,
};

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() < 6 {
        return Err(io::Error::other(
            "usage: plc_compare input.f32 output_dir bitrate frame_ms harmonic [loss-mask.txt ...]",
        )
        .into());
    }
    let duration = match args[4].as_str() {
        "10" => FrameDuration::Ms10,
        "20" => FrameDuration::Ms20,
        _ => return Err(io::Error::other("invalid frame duration").into()),
    };
    let config = Config::new(args[3].parse()?)?
        .with_frame_duration(duration)
        .with_harmonic_speech(args[5] == "1");
    let bytes = fs::read(&args[1])?;
    let (words, remainder) = bytes.as_chunks::<4>();
    if !remainder.is_empty() || words.is_empty() {
        return Err(io::Error::other("invalid f32 input length").into());
    }
    let input: Vec<_> = words.iter().map(|&b| f32::from_le_bytes(b)).collect();
    let samples = duration.samples();
    let mut encoder = Encoder::new(config);
    let mut packet = [0; MAX_PACKET_BYTES];
    let mut frame = vec![0.0; samples];
    let mut packets = Vec::new();
    for chunk in input.chunks(samples) {
        frame.fill(0.0);
        frame[..chunk.len()].copy_from_slice(chunk);
        let len = encoder.encode(&frame, &mut packet)?;
        packets.push(packet[..len].to_vec());
    }
    let len = encoder.finish(&mut packet)?;
    packets.push(packet[..len].to_vec());
    let mut target_decoder = Decoder::with_frame_duration(duration).with_neural_plc(false);
    let mut targets = Vec::new();
    let mut target_coefficients = [0.; snacka::FRAME_SAMPLES];
    for packet in &packets {
        target_decoder.decode_plc_training_frame(packet, &mut frame, &mut target_coefficients)?;
        targets.push(target_coefficients);
    }
    let directory = Path::new(&args[2]);
    fs::create_dir_all(directory)?;
    let mut masks = vec![("uninterrupted".to_owned(), BTreeSet::new())];
    for name in &args[6..] {
        let mask = fs::read_to_string(name)?
            .split_whitespace()
            .map(str::parse::<usize>)
            .collect::<Result<BTreeSet<_>, _>>()?;
        if mask.last().is_some_and(|&i| i >= packets.len() - 1) {
            return Err(io::Error::other("mask drops drain/out-of-range frame").into());
        }
        masks.push((
            Path::new(name)
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
            mask,
        ));
    }
    let mut timing = io::BufWriter::new(fs::File::create(directory.join("timing.csv"))?);
    writeln!(timing, "pattern,implementation,frame,operation,elapsed_ns")?;
    let mut metrics = io::BufWriter::new(fs::File::create(directory.join("band-metrics.csv"))?);
    writeln!(
        metrics,
        "pattern,implementation,band,error_energy,target_energy,log_energy_squared,count"
    )?;
    let hz = [
        0, 200, 400, 600, 800, 1000, 1300, 1600, 2000, 2500, 3200, 4000, 5200, 6800, 9000, 13000,
        24000,
    ];
    let edges = hz.map(|h| h * samples / 24000);
    for (label, mask) in masks {
        for enabled in [false, true] {
            let implementation = if enabled { "current" } else { "baseline" };
            let mut decoder = Decoder::with_frame_duration(duration).with_neural_plc(enabled);
            let mut output = Vec::with_capacity(input.len() + samples * 2);
            let mut decoded = vec![0.0; samples];
            let mut statistics = [[0.0f64; 4]; 16];
            for (index, packet) in packets.iter().enumerate() {
                let lost = mask.contains(&index);
                let started = Instant::now();
                let info = if lost {
                    let mut coefficients = [0.; snacka::FRAME_SAMPLES];
                    let mut features = [0.; 80];
                    let mut overlap = [0.; ALGORITHMIC_DELAY];
                    let info = decoder.conceal_capture(
                        &mut decoded,
                        &mut coefficients,
                        &mut features,
                        &mut overlap,
                    )?;
                    let total: f64 = coefficients[..samples]
                        .iter()
                        .chain(&targets[index][..samples])
                        .map(|&v| (v as f64).powi(2))
                        .sum::<f64>()
                        .max(1e-10);
                    for band in 0..16 {
                        let mut pp = 0.;
                        let mut tt = 0.;
                        let mut error = 0.;
                        for bin in edges[band]..edges[band + 1] {
                            let p = coefficients[bin] as f64;
                            let t = targets[index][bin] as f64;
                            pp += p * p;
                            tt += t * t;
                            error += (p - t).powi(2);
                        }
                        statistics[band][0] += error;
                        statistics[band][1] += tt;
                        if pp + tt > 1e-10 {
                            statistics[band][2] +=
                                ((pp / total + 0.001).ln() - (tt / total + 0.001).ln()).powi(2);
                            statistics[band][3] += 1.;
                        }
                    }
                    info
                } else {
                    decoder.decode(packet, &mut decoded)?
                };
                let elapsed = started.elapsed().as_nanos();
                let operation = if lost {
                    "loss"
                } else if info.finished {
                    "drain"
                } else {
                    "received"
                };
                writeln!(
                    timing,
                    "{label},{implementation},{index},{operation},{elapsed}"
                )?;
                output.extend_from_slice(&decoded[..info.valid_samples]);
            }
            for (band, values) in statistics.iter().enumerate() {
                writeln!(
                    metrics,
                    "{label},{implementation},{band},{},{},{},{}",
                    values[0], values[1], values[2], values[3]
                )?;
            }
            let aligned = &output[ALGORITHMIC_DELAY..ALGORITHMIC_DELAY + input.len()];
            let bytes: Vec<_> = aligned.iter().flat_map(|x| x.to_le_bytes()).collect();
            fs::write(
                directory.join(format!("{label}.{implementation}.f32")),
                bytes,
            )?;
        }
    }
    println!(
        "frames={},packet_bytes={},decoder_bytes={}",
        packets.len() - 1,
        packets.iter().map(Vec::len).sum::<usize>(),
        std::mem::size_of::<Decoder>()
    );
    Ok(())
}
