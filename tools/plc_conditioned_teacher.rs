// SPDX-License-Identifier: MIT
//! Offline native low-overlap sequences, including the first received recovery.
use snacka::{Config, Decoder, Encoder, FRAME_SAMPLES, FrameDuration, MAX_PACKET_BYTES};
use std::{
    env,
    error::Error,
    fs,
    io::{self, Write},
};

fn floats(out: &mut impl Write, values: &[f32]) -> io::Result<()> {
    for value in values {
        out.write_all(&value.to_le_bytes())?;
    }
    Ok(())
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 9 {
        return Err(
            io::Error::other("input labels audit bitrate frame_ms harmonic stride offset").into(),
        );
    }
    let duration = if args[5] == "10" {
        FrameDuration::Ms10
    } else {
        FrameDuration::Ms20
    };
    let n = duration.samples();
    let stride: usize = args[7].parse()?;
    let offset: usize = args[8].parse()?;
    if stride < 12 {
        return Err(io::Error::other("stride >=12 required").into());
    }
    let config = Config::new(args[4].parse()?)?
        .with_frame_duration(duration)
        .with_harmonic_speech(args[6] == "1");
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
    let mut coeff = [0.; FRAME_SAMPLES];
    for packet in &packets {
        reference.decode_plc_training_frame(packet, &mut frame, &mut coeff)?;
        targets.push(coeff);
    }
    let mut decoder = Decoder::with_frame_duration(duration);
    let mut out = io::BufWriter::new(fs::File::create(&args[2])?);
    out.write_all(b"SPCC\x01\0\0\0")?;
    let mut audit = io::BufWriter::new(fs::File::create(&args[3])?);
    audit.write_all(b"SPLA\x01\0\0\0")?;
    let mut index = 0;
    let mut events = 0;
    while index < packets.len() {
        if index >= 4 && (index + offset).is_multiple_of(stride) && index + 7 < packets.len() - 1 {
            let h = [1, 2, 4, 6][events % 4];
            audit.write_all(&(n as u16).to_le_bytes())?;
            audit.write_all(&[h as u8, 0])?;
            for step in 0..h {
                let mut x = [0.; 227];
                decoder.conceal_conditioned_teacher(
                    &mut frame,
                    &targets[index + step][..n],
                    &mut x,
                )?;
                floats(&mut out, &x)?;

                floats(&mut audit, &frame)?;
            }
            decoder.decode_plc_training_frame(&packets[index + h], &mut frame, &mut coeff)?;
            floats(&mut audit, &frame)?;
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
