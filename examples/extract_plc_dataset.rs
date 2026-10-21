// SPDX-License-Identifier: MIT
//! Dump causal native PLC predictions and exact refined-MDCT sufficient statistics.
use snacka::{Config, Decoder, Encoder, FRAME_SAMPLES, FrameDuration, MAX_PACKET_BYTES};
use std::{
    collections::VecDeque,
    env,
    error::Error,
    fs,
    io::{self, BufWriter, Write},
};

const HZ: [usize; 17] = [
    0, 200, 400, 600, 800, 1000, 1300, 1600, 2000, 2500, 3200, 4000, 5200, 6800, 9000, 13000, 24000,
];

struct Pending {
    target: usize,
    horizon: u8,
    features: [f32; 80],
    prediction: [f32; FRAME_SAMPLES],
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<_> = env::args().collect();
    if args.len() != 8 {
        return Err(io::Error::other("usage: extract_plc_dataset input.f32 output.bin bitrate frame_ms harmonic stride offset").into());
    }
    let duration = match args[4].as_str() {
        "10" => FrameDuration::Ms10,
        "20" => FrameDuration::Ms20,
        _ => return Err(io::Error::other("invalid duration").into()),
    };
    let samples = duration.samples();
    let stride: usize = args[6].parse()?;
    let offset: usize = args[7].parse()?;
    if stride < 6 {
        return Err(io::Error::other("stride must be at least six frames").into());
    }
    let bytes = fs::read(&args[1])?;
    let (words, remainder) = bytes.as_chunks::<4>();
    if !remainder.is_empty() {
        return Err(io::Error::other("partial f32 sample").into());
    }
    let pcm: Vec<_> = words.iter().map(|&b| f32::from_le_bytes(b)).collect();
    if pcm.iter().any(|x| !x.is_finite() || x.abs() > 1.0) {
        return Err(io::Error::other("invalid PCM").into());
    }
    let config = Config::new(args[3].parse()?)?
        .with_frame_duration(duration)
        .with_harmonic_speech(args[5] == "1");
    let mut encoder = Encoder::new(config);
    let mut decoder = Decoder::with_frame_duration(duration).with_neural_plc(false);
    let mut writer = BufWriter::new(fs::File::create(&args[2])?);
    writer.write_all(b"SPLC")?;
    writer.write_all(&1u32.to_le_bytes())?;
    let mut pending = VecDeque::<Pending>::new();
    let mut packet = [0; MAX_PACKET_BYTES];
    let mut frame = [0.0; FRAME_SAMPLES];
    let mut decoded = [0.0; FRAME_SAMPLES];
    let mut target = [0.0; FRAME_SAMPLES];
    let mut count = 0usize;
    for (index, chunk) in pcm.chunks(samples).enumerate() {
        frame.fill(0.0);
        frame[..chunk.len()].copy_from_slice(chunk);
        let len = encoder.encode(&frame[..samples], &mut packet)?;
        decoder.decode_plc_training_frame(&packet[..len], &mut decoded[..samples], &mut target)?;
        while pending.front().is_some_and(|p| p.target == index) {
            let record = pending.pop_front().unwrap();
            let mut pp = [0.0f32; 16];
            let mut pt = [0.0f32; 16];
            let mut tt = [0.0f32; 16];
            for band in 0..16 {
                let range = HZ[band] * samples / 24000..HZ[band + 1] * samples / 24000;
                for (&p, &t) in record.prediction[range.clone()].iter().zip(&target[range]) {
                    pp[band] += p * p;
                    pt[band] += p * t;
                    tt[band] += t * t;
                }
            }
            if pp.iter().sum::<f32>() < 1e-10 || tt.iter().sum::<f32>() < 1e-10 {
                continue;
            }
            writer.write_all(&(samples as u16).to_le_bytes())?;
            writer.write_all(&[record.horizon, u8::from(args[5] == "1")])?;
            for x in record.features.iter().chain(&pp).chain(&pt).chain(&tt) {
                writer.write_all(&x.to_le_bytes())?;
            }
            count += 1;
        }
        if index >= 3 && (index + offset).is_multiple_of(stride) {
            for horizon in 1..=6 {
                let mut features = [0.0; 80];
                let mut prediction = [0.0; FRAME_SAMPLES];
                decoder.plc_training_prediction(horizon, &mut features, &mut prediction)?;
                if features[76] >= 0.985 {
                    continue;
                }
                pending.push_back(Pending {
                    target: index + horizon,
                    horizon: horizon as u8,
                    features,
                    prediction,
                });
            }
        }
    }
    writer.flush()?;
    println!("records={count}");
    Ok(())
}
