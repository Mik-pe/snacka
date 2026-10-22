// SPDX-License-Identifier: MIT
//! Fresh native PCM for all finite-grid representation oracles.
use snacka::{
    ALGORITHMIC_DELAY, Config, Decoder, Encoder, FRAME_SAMPLES, FrameDuration, MAX_PACKET_BYTES,
    representation::Kind,
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
    let args: Vec<_> = env::args().collect();
    if args.len() < 7 {
        return Err(io::Error::other(
            "input output bitrate duration harmonic native_rate [mask ...]",
        )
        .into());
    }
    let duration = match args[4].as_str() {
        "10" => FrameDuration::Ms10,
        "20" => FrameDuration::Ms20,
        _ => return Err(io::Error::other("duration").into()),
    };
    let n = duration.samples();
    let supported = if args[6] == "16000" { 13 } else { 16 };
    let config = Config::new(args[3].parse()?)?
        .with_frame_duration(duration)
        .with_harmonic_speech(args[5] == "1");
    let bytes = fs::read(&args[1])?;
    let (words, tail) = bytes.as_chunks::<4>();
    if !tail.is_empty() || words.is_empty() {
        return Err(io::Error::other("PCM framing").into());
    }
    let input: Vec<_> = words.iter().map(|&x| f32::from_le_bytes(x)).collect();
    let mut encoder = Encoder::new(config);
    let mut packet = [0; MAX_PACKET_BYTES];
    let mut pcm = vec![0.; n];
    let mut packets = Vec::new();
    for chunk in input.chunks(n) {
        pcm.fill(0.);
        pcm[..chunk.len()].copy_from_slice(chunk);
        let len = encoder.encode(&pcm, &mut packet)?;
        packets.push(packet[..len].to_vec());
    }
    let len = encoder.finish(&mut packet)?;
    packets.push(packet[..len].to_vec());
    let mut reference = Decoder::with_frame_duration(duration).with_neural_plc(false);
    let mut targets = Vec::new();
    let mut target = [0.; FRAME_SAMPLES];
    for packet in &packets {
        reference.decode_plc_training_frame(packet, &mut pcm, &mut target)?;
        targets.push(target);
    }
    let root = Path::new(&args[2]);
    fs::create_dir_all(root)?;
    let mut masks = vec![("uninterrupted".to_owned(), BTreeSet::new())];
    for path in &args[7..] {
        let mask = fs::read_to_string(path)?
            .split_whitespace()
            .map(str::parse::<usize>)
            .collect::<Result<BTreeSet<_>, _>>()?;
        if mask.last().is_some_and(|&i| i >= packets.len() - 1) {
            return Err(io::Error::other("drops drain").into());
        }
        masks.push((
            Path::new(path)
                .file_stem()
                .unwrap()
                .to_str()
                .unwrap()
                .to_owned(),
            mask,
        ));
    }
    for (label, mask) in masks {
        for kind in Kind::ALL {
            let mut decoder = Decoder::with_frame_duration(duration);
            let mut control = if matches!(kind, Kind::Baseline | Kind::Current) {
                Some(Decoder::with_frame_duration(duration).with_neural_plc(kind == Kind::Current))
            } else {
                None
            };
            let mut out = Vec::new();
            let mut checked = vec![0.; n];
            let mut records = io::BufWriter::new(fs::File::create(
                root.join(format!("{label}.{}.bin", kind.name())),
            )?);
            for (i, packet) in packets.iter().enumerate() {
                let lost = mask.contains(&i);
                let info = if lost {
                    let (info, r) = decoder.conceal_representation(
                        &mut pcm,
                        &targets[i][..n],
                        kind,
                        supported,
                        20261022u32.wrapping_add(i as u32),
                    )?;
                    records.write_all(&(i as u32).to_le_bytes())?;
                    records.write_all(&[u8::from(r.eligible), r.horizon as u8])?;
                    records.write_all(&(n as u16).to_le_bytes())?;
                    records.write_all(&r.phase.to_le_bytes())?;
                    records.write_all(&r.noise.to_le_bytes())?;
                    for x in r
                        .before
                        .iter()
                        .chain(&r.after)
                        .chain(&r.pp)
                        .chain(&r.pt)
                        .chain(&r.tt)
                        .chain(&r.gains)
                    {
                        records.write_all(&x.to_le_bytes())?;
                    }
                    info
                } else {
                    decoder.decode(packet, &mut pcm)?
                };
                if let Some(c) = &mut control {
                    let ci = if lost {
                        c.conceal(&mut checked)?
                    } else {
                        c.decode(packet, &mut checked)?
                    };
                    assert_eq!(info.valid_samples, ci.valid_samples);
                    for (&a, &b) in pcm.iter().zip(&checked) {
                        assert_eq!(a.to_bits(), b.to_bits(), "native hook PCM parity");
                    }
                }
                out.extend_from_slice(&pcm[..info.valid_samples]);
            }
            let aligned = &out[ALGORITHMIC_DELAY..ALGORITHMIC_DELAY + input.len()];
            fs::write(
                root.join(format!("{label}.{}.f32", kind.name())),
                aligned
                    .iter()
                    .flat_map(|x| x.to_le_bytes())
                    .collect::<Vec<_>>(),
            )?;
        }
    }
    Ok(())
}
