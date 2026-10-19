// SPDX-License-Identifier: MIT
//! Extract frame-level MDCT training and validation pairs for the neural postfilter.

use snacka::{
    ALGORITHMIC_DELAY, Config, Decoder, Encoder, FRAME_SAMPLES, FrameDuration, MAX_PACKET_BYTES,
    transform::{MdctPlan, MdctWorkspace},
};
use std::{
    env,
    error::Error,
    fs::{self, File},
    io::{self, BufWriter, Write},
    path::Path,
};

/// Deterministic Box-Muller Gaussian noise matching `tools/evaluate_speech.py`.
struct Rng {
    state: u64,
}

impl Rng {
    fn new(seed: u64) -> Self {
        Self {
            state: seed ^ 0x9e37_79b9_7f4a_7c15,
        }
    }

    fn next_u64(&mut self) -> u64 {
        let mut x = self.state;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.state = x;
        x
    }

    fn next_f64(&mut self) -> f64 {
        ((self.next_u64() >> 11) as f64) * (1.0 / ((1u64 << 53) as f64))
    }

    fn next_gauss(&mut self) -> f64 {
        let u1 = self.next_f64().max(1e-15);
        let u2 = self.next_f64();
        (-2.0 * u1.ln()).sqrt() * (std::f64::consts::TAU * u2).cos()
    }
}

fn make_noisy(clean: &[f32], seed_str: &str, snr_db: i32) -> Vec<f32> {
    let mut seed = 0xcbf2_9ce4_8422_2325u64;
    for b in seed_str.bytes() {
        seed ^= u64::from(b);
        seed = seed.wrapping_mul(0x0100_0000_01b3);
    }
    seed ^= snr_db as u64;
    let mut rng = Rng::new(seed);
    let signal_power: f64 = clean
        .iter()
        .map(|&x| f64::from(x) * f64::from(x))
        .sum::<f64>()
        / (clean.len().max(1) as f64);
    if signal_power <= 1e-12 {
        return clean.to_vec();
    }
    let noise: Vec<f64> = (0..clean.len()).map(|_| rng.next_gauss()).collect();
    let noise_power: f64 = noise.iter().map(|&n| n * n).sum::<f64>() / (noise.len() as f64);
    let scale = (signal_power / (noise_power * 10.0f64.powf(f64::from(snr_db) / 10.0))).sqrt();
    clean
        .iter()
        .zip(&noise)
        .map(|(&x, &n)| ((f64::from(x) + n * scale) as f32).clamp(-1.0, 1.0))
        .collect()
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = env::args().collect();
    if args.len() < 3 || args.len() > 4 {
        return Err(io::Error::other(
            "usage: extract_neural_dataset <f32_dir> <output.bin> [frame_stride]",
        )
        .into());
    }
    let input_dir = Path::new(&args[1]);
    let output_path = Path::new(&args[2]);
    let frame_stride: usize = args.get(3).map_or(Ok(2), |s| s.parse())?;

    let mut entries: Vec<_> = fs::read_dir(input_dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "f32"))
        .collect();
    entries.sort();
    if entries.is_empty() {
        return Err(io::Error::other("no .f32 files found in input_dir").into());
    }

    let mut writer = BufWriter::with_capacity(8 * 1024 * 1024, File::create(output_path)?);
    // Magic header: "SNK1" + version 1u32
    writer.write_all(b"SNK1")?;
    writer.write_all(&1u32.to_le_bytes())?;

    let mut total_records = 0usize;

    for (clip_idx, path) in entries.iter().enumerate() {
        let stem = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("clip")
            .to_string();
        let bytes = fs::read(path)?;
        let (chunks, _) = bytes.as_chunks::<4>();
        if chunks.len() < 48_000 {
            continue;
        }
        let clean_pcm: Vec<f32> = chunks
            .iter()
            .map(|c| f32::from_le_bytes(*c).clamp(-1.0, 1.0))
            .collect();

        for (cond_id, cond_snr) in [(0u8, None), (1u8, Some(20)), (2u8, Some(10))] {
            let input_pcm = match cond_snr {
                None => clean_pcm.clone(),
                Some(snr) => make_noisy(&clean_pcm, &stem, snr),
            };

            for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
                let samples = duration.samples();
                let start = (samples - ALGORITHMIC_DELAY) / 2;
                let plan = MdctPlan::new(samples, ALGORITHMIC_DELAY)?;
                let mut clean_ws = MdctWorkspace::new(&plan);
                let mut input_ws = MdctWorkspace::new(&plan);

                for bitrate in [12_000u32, 16_000, 24_000, 32_000] {
                    for harmonic in [false, true] {
                        let config = Config::new(bitrate)?
                            .with_frame_duration(duration)
                            .with_harmonic_speech(harmonic);
                        let mut encoder = Encoder::new(config);
                        let mut decoder =
                            Decoder::with_frame_duration(duration).with_neural_postfilter(false);

                        let mut prev_clean = [0.0f32; ALGORITHMIC_DELAY];
                        let mut prev_input = [0.0f32; ALGORITHMIC_DELAY];
                        let mut clean_win = [0.0f32; 2 * FRAME_SAMPLES];
                        let mut input_win = [0.0f32; 2 * FRAME_SAMPLES];
                        let mut clean_coeffs = [0.0f32; FRAME_SAMPLES];
                        let mut input_coeffs = [0.0f32; FRAME_SAMPLES];
                        let mut decoded_coeffs = [0.0f32; FRAME_SAMPLES];
                        let mut global_features = [0.0f32; 64];
                        let mut group_sigma = [0.0f32; 4];
                        let mut edges = [0usize; 5];
                        let mut codes = [0u8; 4];
                        let mut is_harmonic = false;
                        let mut packet = [0u8; MAX_PACKET_BYTES];

                        let num_frames = input_pcm.len() / samples;
                        for frame_idx in 0..num_frames {
                            let f_start = frame_idx * samples;
                            let f_end = f_start + samples;
                            let clean_slice = &clean_pcm[f_start..f_end];
                            let input_slice = &input_pcm[f_start..f_end];

                            clean_win.fill(0.0);
                            clean_win[start..start + ALGORITHMIC_DELAY]
                                .copy_from_slice(&prev_clean);
                            clean_win
                                [start + ALGORITHMIC_DELAY..start + ALGORITHMIC_DELAY + samples]
                                .copy_from_slice(clean_slice);
                            plan.forward(
                                &clean_win[..2 * samples],
                                &mut clean_coeffs[..samples],
                                &mut clean_ws,
                            )?;
                            prev_clean.copy_from_slice(&clean_slice[samples - ALGORITHMIC_DELAY..]);

                            input_win.fill(0.0);
                            input_win[start..start + ALGORITHMIC_DELAY]
                                .copy_from_slice(&prev_input);
                            input_win
                                [start + ALGORITHMIC_DELAY..start + ALGORITHMIC_DELAY + samples]
                                .copy_from_slice(input_slice);
                            plan.forward(
                                &input_win[..2 * samples],
                                &mut input_coeffs[..samples],
                                &mut input_ws,
                            )?;
                            prev_input.copy_from_slice(&input_slice[samples - ALGORITHMIC_DELAY..]);

                            let pkt_len = encoder.encode(input_slice, &mut packet)?;
                            let coded_bins = decoder.decode_unrefined_training_frame(
                                &packet[..pkt_len],
                                &mut decoded_coeffs,
                                &mut global_features,
                                &mut group_sigma,
                                &mut edges,
                                &mut codes,
                                &mut is_harmonic,
                            )?;

                            if coded_bins == 0 || frame_idx == 0 {
                                continue;
                            }
                            if (frame_idx + clip_idx) % frame_stride != 0 {
                                continue;
                            }

                            let occupied = decoded_coeffs[..coded_bins]
                                .iter()
                                .filter(|&&x| x.abs() > 1e-6)
                                .count();
                            if occupied < 2 {
                                continue;
                            }

                            let clean_energy: f32 =
                                clean_coeffs[..coded_bins].iter().map(|&x| x * x).sum();
                            if clean_energy < 1e-7 {
                                continue;
                            }

                            // Write binary record:
                            // samples (u16), coded_bins (u16), is_harmonic (u8), cond_id (u8), codes ([u8; 4]), edges ([u16; 5]) = 20 bytes
                            writer.write_all(&(samples as u16).to_le_bytes())?;
                            writer.write_all(&(coded_bins as u16).to_le_bytes())?;
                            writer.write_all(&[u8::from(is_harmonic), cond_id])?;
                            writer.write_all(&codes)?;
                            for &e in &edges {
                                writer.write_all(&(e as u16).to_le_bytes())?;
                            }
                            for &s in &group_sigma {
                                writer.write_all(&s.to_le_bytes())?;
                            }
                            for &gf in &global_features {
                                writer.write_all(&gf.to_le_bytes())?;
                            }
                            for &dc in &decoded_coeffs[..coded_bins] {
                                writer.write_all(&dc.to_le_bytes())?;
                            }
                            for &ic in &input_coeffs[..coded_bins] {
                                writer.write_all(&ic.to_le_bytes())?;
                            }
                            for &cc in &clean_coeffs[..coded_bins] {
                                writer.write_all(&cc.to_le_bytes())?;
                            }
                            total_records += 1;
                        }
                    }
                }
            }
        }
    }
    writer.flush()?;
    eprintln!(
        "Extracted {} frame records to {}",
        total_records,
        output_path.display()
    );
    Ok(())
}
