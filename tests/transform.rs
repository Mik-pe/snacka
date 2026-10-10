// SPDX-License-Identifier: MIT
use snacka::transform::{Complex, Error, FftPlan, MdctPlan, MdctWorkspace};

fn noise(length: usize) -> Vec<f32> {
    let mut state = 0x18d4_ab2eu32;
    (0..length)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            (state as f64 / u32::MAX as f64 - 0.5) as f32
        })
        .collect()
}

#[test]
fn mixed_radix_fft_matches_independent_double_precision_dft() {
    for length in [1, 2, 3, 5, 6, 15, 60, 120, 240, 480] {
        let input: Vec<_> = noise(2 * length)
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| Complex {
                real: pair[0],
                imaginary: pair[1],
            })
            .collect();
        let mut output = vec![Complex::ZERO; length];
        let plan = FftPlan::new(length).unwrap();
        plan.forward(&input, &mut output).unwrap();
        let mut energy = 0.0;
        let mut error = 0.0;
        for (frequency, sample) in output.iter().enumerate() {
            let mut real = 0.0;
            let mut imaginary = 0.0;
            for (index, sample) in input.iter().enumerate() {
                let angle = -std::f64::consts::TAU * (index * frequency) as f64 / length as f64;
                real += f64::from(sample.real) * angle.cos()
                    - f64::from(sample.imaginary) * angle.sin();
                imaginary += f64::from(sample.real) * angle.sin()
                    + f64::from(sample.imaginary) * angle.cos();
            }
            energy += real.powi(2) + imaginary.powi(2);
            error += (real - f64::from(sample.real)).powi(2)
                + (imaginary - f64::from(sample.imaginary)).powi(2);
        }
        let snr = 10.0 * (energy / error.max(1e-30)).log10();
        assert!(snr > 115.0, "FFT length {length}: {snr:.1} dB agreement");
        let mut recovered = vec![Complex::ZERO; length];
        plan.inverse(&output, &mut recovered).unwrap();
        for (expected, actual) in input.iter().zip(recovered) {
            assert!((expected.real - actual.real).abs() < 2e-6);
            assert!((expected.imaginary - actual.imaginary).abs() < 2e-6);
        }
    }
}

fn window(index: usize, n: usize, overlap: usize) -> f64 {
    let padding = (n - overlap) / 2;
    let reflected = index.min(2 * n - 1 - index);
    if reflected < padding {
        0.0
    } else if reflected >= padding + overlap {
        1.0
    } else {
        let angle =
            std::f64::consts::FRAC_PI_2 * ((reflected - padding) as f64 + 0.5) / overlap as f64;
        (std::f64::consts::FRAC_PI_2 * angle.sin().powi(2)).sin()
    }
}

#[test]
fn fast_mdct_matches_the_defining_double_precision_cosine_sum() {
    for n in [120, 240, 480, 960] {
        let input = noise(2 * n);
        let plan = MdctPlan::new(n, 120).unwrap();
        let mut workspace = MdctWorkspace::new(&plan);
        let mut output = vec![0.0; n];
        plan.forward(&input, &mut output, &mut workspace).unwrap();
        let mut energy = 0.0;
        let mut error = 0.0;
        for (frequency, &actual) in output.iter().enumerate() {
            let expected: f64 = input
                .iter()
                .enumerate()
                .map(|(index, &sample)| {
                    let phase = std::f64::consts::PI / n as f64
                        * (index as f64 + 0.5 + n as f64 / 2.0)
                        * (frequency as f64 + 0.5);
                    f64::from(sample) * window(index, n, 120) * phase.cos()
                })
                .sum::<f64>()
                * 2.0
                / n as f64;
            energy += expected.powi(2);
            error += (expected - f64::from(actual)).powi(2);
        }
        let snr = 10.0 * (energy / error.max(1e-30)).log10();
        assert!(snr > 110.0, "MDCT {n}: {snr:.1} dB agreement");
    }
}

#[test]
fn overlapping_inverse_blocks_reconstruct_audio_with_short_and_full_windows() {
    for (n, overlap) in [(120, 120), (240, 120), (480, 120), (960, 120), (240, 240)] {
        let source = noise(8 * n);
        let mut recovered = vec![0.0; source.len()];
        let plan = MdctPlan::new(n, overlap).unwrap();
        let mut workspace = MdctWorkspace::new(&plan);
        let mut coefficients = vec![0.0; n];
        let mut inverse = vec![0.0; 2 * n];
        for frame in 0..7 {
            plan.forward(
                &source[frame * n..frame * n + 2 * n],
                &mut coefficients,
                &mut workspace,
            )
            .unwrap();
            plan.inverse(&coefficients, &mut inverse, &mut workspace)
                .unwrap();
            for (destination, &sample) in recovered[frame * n..frame * n + 2 * n]
                .iter_mut()
                .zip(&inverse)
            {
                *destination += sample;
            }
        }
        for (&expected, &actual) in source[n..7 * n].iter().zip(&recovered[n..7 * n]) {
            assert!(
                (expected - actual).abs() < 3e-6,
                "TDAC {n}/{overlap}: {expected} versus {actual}"
            );
        }
    }
}

#[test]
fn invalid_dimensions_and_samples_leave_output_unchanged() {
    assert!(FftPlan::new(0).is_err());
    assert!(FftPlan::new(7).is_err());
    assert!(FftPlan::new(1_921).is_err());
    assert!(MdctPlan::new(120, 121).is_err());
    assert!(MdctPlan::new(120, 119).is_err());
    let fft = FftPlan::new(60).unwrap();
    let mut complex_output = [Complex {
        real: 9.0,
        imaginary: 4.0,
    }; 60];
    assert_eq!(
        fft.forward(&[Complex::ZERO; 59], &mut complex_output),
        Err(Error::Length)
    );
    for value in [f32::NAN, f32::INFINITY, f32::MAX] {
        let mut input = [Complex::ZERO; 60];
        input[17].real = value;
        assert_eq!(fft.forward(&input, &mut complex_output), Err(Error::Sample));
        assert!(complex_output.iter().all(|sample| *sample
            == Complex {
                real: 9.0,
                imaginary: 4.0
            }));
    }
    let plan = MdctPlan::new(120, 120).unwrap();
    let mut workspace = MdctWorkspace::new(&plan);
    let mut output = [9.0; 120];
    for value in [f32::NAN, f32::INFINITY, f32::MAX] {
        let mut input = [0.0; 240];
        input[7] = value;
        assert_eq!(
            plan.forward(&input, &mut output, &mut workspace),
            Err(Error::Sample)
        );
        assert_eq!(output, [9.0; 120]);
    }
    let wrong_plan = MdctPlan::new(240, 120).unwrap();
    let mut wrong_workspace = MdctWorkspace::new(&wrong_plan);
    assert_eq!(
        plan.forward(&[0.0; 240], &mut output, &mut wrong_workspace),
        Err(Error::Length)
    );
    assert_eq!(output, [9.0; 120]);
}

#[test]
fn streaming_short_overlap_has_exactly_120_samples_of_delay() {
    for n in [480, 960] {
        let overlap = 120;
        let start = (n - overlap) / 2;
        let mut source = noise(5 * n);
        source[0] = 0.8;
        source[n - 1] = -0.8;
        source[5 * n - 1] = 0.8;
        let plan = MdctPlan::new(n, overlap).unwrap();
        let mut workspace = MdctWorkspace::new(&plan);
        let mut previous = vec![0.0; overlap];
        let mut tail = vec![0.0; overlap];
        let mut block = vec![0.0; 2 * n];
        let mut inverse = vec![0.0; 2 * n];
        let mut coefficients = vec![0.0; n];
        let mut recovered = Vec::new();
        for frame in 0..=5 {
            let input = if frame < 5 {
                &source[frame * n..(frame + 1) * n]
            } else {
                &vec![0.0; n]
            };
            block[start..start + overlap].copy_from_slice(&previous);
            block[start + overlap..start + overlap + n].copy_from_slice(input);
            plan.forward(&block, &mut coefficients, &mut workspace)
                .unwrap();
            plan.inverse(&coefficients, &mut inverse, &mut workspace)
                .unwrap();
            let valid = if frame == 5 { overlap } else { n };
            for i in 0..valid {
                recovered.push(inverse[start + i] + if i < overlap { tail[i] } else { 0.0 });
            }
            previous.copy_from_slice(&input[n - overlap..]);
            tail.copy_from_slice(&inverse[start + n..start + n + overlap]);
        }
        assert!(recovered[..overlap].iter().all(|x| x.abs() < 3e-6));
        assert_eq!(recovered.len() - overlap, source.len());
        for (&expected, &actual) in source.iter().zip(&recovered[overlap..]) {
            assert!(
                (expected - actual).abs() < 3e-6,
                "{n}: {expected} vs {actual}"
            );
        }
    }
}
