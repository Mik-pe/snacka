// SPDX-License-Identifier: MIT
//! Executed only inside the isolated diagnostic crate made by build_plc_oracle.py.
use snacka::{
    ALGORITHMIC_DELAY, Config, Decoder, Encoder, FRAME_SAMPLES, FrameDuration, MAX_PACKET_BYTES,
    oracle::Kind,
};
use std::{
    collections::BTreeSet,
    env,
    error::Error,
    fs,
    io::{self, Write},
    path::Path,
};

fn main() -> Result<(), Box<dyn Error>> {
    let mut args: Vec<_> = env::args().collect();
    let verify = args.last().is_some_and(|x| x == "--verify-native");
    if verify {
        args.pop();
    }
    if args.len() < 6 {
        return Err(io::Error::other(
            "input.f32 output bitrate frame_ms harmonic [mask ...] [--verify-native]",
        )
        .into());
    }
    let duration = match args[4].as_str() {
        "10" => FrameDuration::Ms10,
        "20" => FrameDuration::Ms20,
        _ => return Err(io::Error::other("duration").into()),
    };
    let config = Config::new(args[3].parse()?)?
        .with_frame_duration(duration)
        .with_harmonic_speech(args[5] == "1");
    let samples = duration.samples();
    let raw = fs::read(&args[1])?;
    let (words, rest) = raw.as_chunks::<4>();
    if !rest.is_empty() || words.is_empty() {
        return Err(io::Error::other("PCM framing").into());
    }
    let input: Vec<_> = words.iter().map(|&x| f32::from_le_bytes(x)).collect();
    let mut encoder = Encoder::new(config);
    let mut packet = [0; MAX_PACKET_BYTES];
    let mut frame = vec![0.; samples];
    let mut packets = Vec::new();
    for chunk in input.chunks(samples) {
        frame.fill(0.);
        frame[..chunk.len()].copy_from_slice(chunk);
        let n = encoder.encode(&frame, &mut packet)?;
        packets.push(packet[..n].to_vec());
    }
    let n = encoder.finish(&mut packet)?;
    packets.push(packet[..n].to_vec());
    let directory = Path::new(&args[2]);
    fs::create_dir_all(directory)?;
    let mut reference = Decoder::with_frame_duration(duration).with_neural_plc(false);
    let mut targets = Vec::new();
    let mut decoded = vec![0.; samples];
    let mut target = [0.; FRAME_SAMPLES];
    for packet in &packets {
        target.fill(0.);
        reference.decode_plc_training_frame(packet, &mut decoded, &mut target)?;
        targets.push(target);
    }
    let mut masks = vec![("uninterrupted".to_owned(), BTreeSet::new())];
    for name in &args[6..] {
        let mask = fs::read_to_string(name)?
            .split_whitespace()
            .map(str::parse::<usize>)
            .collect::<Result<BTreeSet<_>, _>>()?;
        if mask.last().is_some_and(|&x| x >= packets.len() - 1) {
            return Err(io::Error::other("mask drops drain").into());
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
    let mut summary = io::BufWriter::new(fs::File::create(directory.join("frame-errors.csv"))?);
    writeln!(
        summary,
        "pattern,implementation,lost_frames,eligible_frames,mdct_error,normalized_mdct_error_eligible,combined_applied_loss_eligible,training_scale_loss_eligible"
    )?;
    for (label, mask) in masks {
        for kind in Kind::ALL {
            let mut decoder = Decoder::with_frame_duration(duration);
            let mut control = if verify && matches!(kind, Kind::Baseline | Kind::Current) {
                Some(
                    Decoder::with_frame_duration(duration)
                        .with_neural_plc(matches!(kind, Kind::Current)),
                )
            } else {
                None
            };
            let mut output = Vec::new();
            let mut decoded = vec![0.; samples];
            let mut checked = vec![0.; samples];
            let mut stats = if matches!(kind, Kind::Baseline) {
                Some(io::BufWriter::new(fs::File::create(
                    directory.join(format!("{label}.stats.bin")),
                )?))
            } else {
                None
            };
            let (mut lost, mut eligible, mut error, mut normalized, mut combined, mut training) =
                (0, 0, 0., 0., 0., 0.);
            for (index, packet) in packets.iter().enumerate() {
                let dropped = mask.contains(&index);
                let info = if dropped {
                    let (info, record) =
                        decoder.conceal_oracle(&mut decoded, &targets[index][..samples], kind)?;
                    lost += 1;
                    if let Some(out) = &mut stats {
                        record.write(out, index, samples)?;
                    }
                    for b in 0..16 {
                        let g = record.gain[b];
                        error +=
                            (record.pp[b] * g * g - 2. * record.pt[b] * g + record.tt[b]).max(0.);
                    }
                    if record.eligible {
                        eligible += 1;
                        let (a, b, c) = record.normalized_errors();
                        normalized += a;
                        combined += b;
                        training += c;
                    }
                    info
                } else {
                    decoder.decode(packet, &mut decoded)?
                };
                if let Some(control) = &mut control {
                    let check = if dropped {
                        control.conceal(&mut checked)?
                    } else {
                        control.decode(packet, &mut checked)?
                    };
                    assert_eq!(info.valid_samples, check.valid_samples);
                    for (&a, &b) in decoded.iter().zip(&checked) {
                        assert_eq!(
                            a.to_bits(),
                            b.to_bits(),
                            "diagnostic hook changed native PCM"
                        );
                    }
                }
                output.extend_from_slice(&decoded[..info.valid_samples]);
            }
            let aligned = &output[ALGORITHMIC_DELAY..ALGORITHMIC_DELAY + input.len()];
            fs::write(
                directory.join(format!("{label}.{}.f32", kind.name())),
                aligned
                    .iter()
                    .flat_map(|x| x.to_le_bytes())
                    .collect::<Vec<_>>(),
            )?;
            writeln!(
                summary,
                "{label},{},{lost},{eligible},{error},{normalized},{combined},{training}",
                kind.name()
            )?;
        }
    }
    Ok(())
}
