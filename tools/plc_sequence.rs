// SPDX-License-Identifier: MIT
//! Offline native low-overlap sequences, including the first received recovery.
use snacka::{
    ALGORITHMIC_DELAY, Config, Decoder, Encoder, FRAME_SAMPLES, FrameDuration, MAX_PACKET_BYTES,
    transform::{MdctPlan, MdctWorkspace},
};
use std::{
    env,
    error::Error,
    fs,
    io::{self, Write},
    path::Path,
};

fn floats(out: &mut impl Write, values: &[f32]) -> io::Result<()> {
    for value in values {
        out.write_all(&value.to_le_bytes())?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args().collect();
    if args.get(1).is_some_and(|x| x == "basis") {
        let root = Path::new(&args[2]);
        fs::create_dir_all(root)?;
        for n in [480, 960] {
            let plan = MdctPlan::new(n, ALGORITHMIC_DELAY)?;
            let mut workspace = MdctWorkspace::new(&plan);
            let mut coefficients = vec![0.; n];
            let mut block = vec![0.; 2 * n];
            let mut out =
                io::BufWriter::new(fs::File::create(root.join(format!("basis-{n}.f32")))?);
            let start = (n - ALGORITHMIC_DELAY) / 2;
            for bin in 0..n {
                coefficients.fill(0.);
                coefficients[bin] = 1.;
                plan.inverse(&coefficients, &mut block, &mut workspace)?;
                floats(&mut out, &block[start..start + n + ALGORITHMIC_DELAY])?;
            }
        }
        return Ok(());
    }
    if args.len() != 8 {
        return Err(
            io::Error::other("input output bitrate frame_ms harmonic stride offset").into(),
        );
    }
    let duration = if args[4] == "10" {
        FrameDuration::Ms10
    } else {
        FrameDuration::Ms20
    };
    let n = duration.samples();
    let stride: usize = args[6].parse()?;
    let offset: usize = args[7].parse()?;
    if stride < 16 {
        return Err(io::Error::other("stride >=16 required").into());
    }
    let config = Config::new(args[3].parse()?)?
        .with_frame_duration(duration)
        .with_harmonic_speech(args[5] == "1");
    let raw = fs::read(&args[1])?;
    let (words, tail) = raw.as_chunks::<4>();
    if !tail.is_empty() {
        return Err(io::Error::other("PCM framing").into());
    }
    let input: Vec<_> = words.iter().map(|&x| f32::from_le_bytes(x)).collect();
    let mut encoder = Encoder::new(config);
    let mut packet = [0; MAX_PACKET_BYTES];
    let mut frame = vec![0.; n];
    let mut packets = Vec::new();
    for chunk in input.chunks(n) {
        frame.fill(0.);
        frame[..chunk.len()].copy_from_slice(chunk);
        let len = encoder.encode(&frame, &mut packet)?;
        packets.push(packet[..len].to_vec());
    }
    let len = encoder.finish(&mut packet)?;
    packets.push(packet[..len].to_vec());
    let mut reference = Decoder::with_frame_duration(duration).with_neural_plc(false);
    let mut targets = Vec::new();
    let mut target_pcm = Vec::new();
    let mut coeff = [0.; FRAME_SAMPLES];
    for packet in &packets {
        reference.decode_plc_training_frame(packet, &mut frame, &mut coeff)?;
        targets.push(coeff);
        target_pcm.push(frame.clone());
    }
    let mut decoder = Decoder::with_frame_duration(duration).with_neural_plc(false);
    let mut out = io::BufWriter::new(fs::File::create(&args[2])?);
    out.write_all(b"SPLS\x01\0\0\0")?;
    let mut index = 0;
    let mut events = 0;
    while index < packets.len() {
        if index >= 4 && (index + offset).is_multiple_of(stride) && index + 7 < packets.len() - 1 {
            let h = [1, 2, 4, 6][events % 4];
            out.write_all(&(n as u16).to_le_bytes())?;
            out.write_all(&[h as u8, 0])?;
            let mut xs = Vec::new();
            let mut predictions = Vec::new();
            let mut baseline = Vec::new();
            let mut initial = [0.; ALGORITHMIC_DELAY];
            for step in 0..h {
                let mut x = [0.; 80];
                let mut overlap = [0.; ALGORITHMIC_DELAY];
                decoder.conceal_capture(&mut frame, &mut coeff, &mut x, &mut overlap)?;
                if step == 0 {
                    initial = overlap;
                }
                xs.extend_from_slice(&x);
                predictions.extend_from_slice(&coeff[..n]);
                baseline.extend_from_slice(&frame);
            }
            decoder.decode_plc_training_frame(&packets[index + h], &mut frame, &mut coeff)?;
            baseline.extend_from_slice(&frame);
            floats(&mut out, &initial)?;
            floats(&mut out, &xs)?;
            floats(&mut out, &predictions)?;
            floats(&mut out, &coeff[..n])?;
            for target in &targets[index..index + h] {
                floats(&mut out, &target[..n])?;
            }
            for pcm in &target_pcm[index..=index + h] {
                floats(&mut out, pcm)?;
            }
            floats(&mut out, &baseline)?;
            index += h + 1;
            events += 1;
        } else {
            decoder.decode(&packets[index], &mut frame)?;
            index += 1;
        }
    }
    println!("events={events}");
    Ok(())
}
