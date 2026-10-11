// SPDX-License-Identifier: MIT
const OVERLAP: usize = 120;
const MAX_HARMONICS: usize = 106;
const MOMENTS: usize = 2 * MAX_HARMONICS + 1;
const PARAMETER_LIMIT: f64 = 1024.0;

#[derive(Clone, Copy, Default)]
struct Complex {
    real: f64,
    imaginary: f64,
}

impl Complex {
    const ZERO: Self = Self {
        real: 0.0,
        imaginary: 0.0,
    };

    fn phase(angle: f64) -> Self {
        let (imaginary, real) = angle.sin_cos();
        Self { real, imaginary }
    }

    fn conjugate(self) -> Self {
        Self {
            real: self.real,
            imaginary: -self.imaginary,
        }
    }

    fn multiply(self, other: Self) -> Self {
        Self {
            real: self.real * other.real - self.imaginary * other.imaginary,
            imaginary: self.real * other.imaginary + self.imaginary * other.real,
        }
    }

    fn scale(self, factor: f64) -> Self {
        Self {
            real: self.real * factor,
            imaginary: self.imaginary * factor,
        }
    }

    fn add(self, other: Self) -> Self {
        Self {
            real: self.real + other.real,
            imaginary: self.imaginary + other.imaginary,
        }
    }
}

struct Gram {
    diagonal: [Complex; MOMENTS],
    reflected: [Complex; MOMENTS],
    phase_frame: [Complex; MAX_HARMONICS + 1],
    phase_ramp: [Complex; MAX_HARMONICS + 1],
}

#[cfg(test)]
fn signed(values: &[Complex], index: i32) -> Complex {
    let value = values[index.unsigned_abs() as usize];
    if index < 0 { value.conjugate() } else { value }
}

impl Gram {
    fn new(samples: usize, f0: usize, window: &[f32]) -> Self {
        let harmonics = 8000 / f0;
        let padding = (samples - OVERLAP) / 2;
        let ramp = &window[padding..padding + OVERLAP];
        let omega = std::f64::consts::TAU * f0 as f64 / 48000.0;
        let mut result = Self {
            diagonal: [Complex::ZERO; MOMENTS],
            reflected: [Complex::ZERO; MOMENTS],
            phase_frame: [Complex::ZERO; MAX_HARMONICS + 1],
            phase_ramp: [Complex::ZERO; MAX_HARMONICS + 1],
        };
        for moment in 0..=2 * harmonics {
            let angle = moment as f64 * omega;
            let rotation = Complex::phase(angle);
            let mut phase = Complex {
                real: 1.0,
                imaginary: 0.0,
            };
            let mut ramp_power = Complex::ZERO;
            let mut reflected = Complex::ZERO;
            for t in 0..OVERLAP {
                let value = f64::from(ramp[t]);
                let mirror = f64::from(ramp[OVERLAP - 1 - t]);
                ramp_power = ramp_power.add(phase.scale(value * value));
                reflected = reflected.add(phase.conjugate().scale(value * mirror));
                phase = phase.multiply(rotation);
            }
            let middle = if moment == 0 {
                Complex {
                    real: (samples - OVERLAP) as f64,
                    imaginary: 0.0,
                }
            } else {
                let length = (samples - OVERLAP) as f64;
                Complex::phase(angle * (length - 1.0) * 0.5)
                    .scale((angle * length * 0.5).sin() / (angle * 0.5).sin())
                    .multiply(Complex::phase(angle * OVERLAP as f64))
            };
            result.diagonal[moment] = middle.add(ramp_power).add(
                Complex::phase(angle * (samples + OVERLAP - 1) as f64)
                    .multiply(ramp_power.conjugate()),
            );
            result.reflected[moment] = reflected;
        }
        for harmonic in 0..=harmonics {
            let angle = harmonic as f64 * omega;
            result.phase_frame[harmonic] = Complex::phase(angle * samples as f64);
            result.phase_ramp[harmonic] = Complex::phase(angle * (OVERLAP - 1) as f64);
        }
        result
    }

    // Leading time aliases subtract; trailing time aliases add. The two
    // reflected ramp terms share the same sum-frequency moment.
    #[cfg(test)]
    fn complex_block(&self, m: i32, n: i32) -> Complex {
        let mut phase =
            signed(&self.phase_frame, n).multiply(signed(&self.phase_frame, m).conjugate());
        phase.real -= 1.0;
        signed(&self.diagonal, n - m).add(
            phase
                .multiply(signed(&self.phase_ramp, n))
                .multiply(signed(&self.reflected, m + n)),
        )
    }

    #[cfg(test)]
    fn generic_block(&self, h: usize, j: usize) -> [[f64; 2]; 2] {
        let positive = self.complex_block(h as i32, j as i32);
        let negative = self.complex_block(h as i32, -(j as i32));
        [
            [
                (positive.real + negative.real) * 0.5,
                (positive.imaginary - negative.imaginary) * 0.5,
            ],
            [
                (-positive.imaginary - negative.imaginary) * 0.5,
                (positive.real - negative.real) * 0.5,
            ],
        ]
    }

    #[inline(always)]
    fn block(&self, h: usize, j: usize) -> [[f64; 2]; 2] {
        #[cfg(test)]
        if GENERIC_BLOCKS.with(|generic| generic.get()) {
            return self.generic_block(h, j);
        }
        let difference = h.abs_diff(j);
        let sum = h + j;
        let phase_h = self.phase_frame[h].conjugate();
        let phase_j = self.phase_frame[j];
        let ramp = self.phase_ramp[j];
        let diagonal = self.diagonal[difference];
        let reflected = self.reflected[difference];
        let positive_diagonal = if j < h {
            diagonal.conjugate()
        } else {
            diagonal
        };
        let negative_reflected = if h < j {
            reflected.conjugate()
        } else {
            reflected
        };
        let mut positive_phase = phase_j.multiply(phase_h);
        positive_phase.real -= 1.0;
        let positive =
            positive_diagonal.add(positive_phase.multiply(ramp).multiply(self.reflected[sum]));
        let mut negative_phase = phase_j.conjugate().multiply(phase_h);
        negative_phase.real -= 1.0;
        let negative = self.diagonal[sum].conjugate().add(
            negative_phase
                .multiply(ramp.conjugate())
                .multiply(negative_reflected),
        );
        [
            [
                (positive.real + negative.real) * 0.5,
                (positive.imaginary - negative.imaginary) * 0.5,
            ],
            [
                (-positive.imaginary - negative.imaginary) * 0.5,
                (positive.real - negative.real) * 0.5,
            ],
        ]
    }
}

fn weighted_sample(input: &[f32], window: &[f32], samples: usize, t: usize) -> f64 {
    let padding = (samples - OVERLAP) / 2;
    if t < OVERLAP || t >= samples {
        let mirrored = if t < OVERLAP {
            OVERLAP - 1 - t
        } else {
            2 * samples + OVERLAP - 1 - t
        };
        let sign = if t < OVERLAP { -1.0 } else { 1.0 };
        let value = f64::from(window[padding + t]);
        let mirror = f64::from(window[padding + mirrored]);
        value * (value * f64::from(input[t]) + sign * mirror * f64::from(input[mirrored]))
    } else {
        f64::from(input[t])
    }
}

fn right_hand_side(
    input: &[f32],
    window: &[f32],
    samples: usize,
    f0: usize,
) -> [f64; 2 * MAX_HARMONICS] {
    let harmonics = 8000 / f0;
    let mut result = [0.0; 2 * MAX_HARMONICS];
    for first in (1..=harmonics).step_by(4) {
        let active = (harmonics - first + 1).min(4);
        let mut angles = [0.0; 4];
        let mut rotations_s = [0.0; 4];
        let mut rotations_c = [0.0; 4];
        let mut twice_c = [0.0; 4];
        for lane in 0..active {
            angles[lane] = std::f64::consts::TAU * ((first + lane) * f0) as f64 / 48000.0;
            (rotations_s[lane], rotations_c[lane]) = angles[lane].sin_cos();
            twice_c[lane] = 2.0 * rotations_c[lane];
        }
        let mut previous = [0.0; 4];
        let mut prior = [0.0; 4];
        for t in 0..input.len() {
            let sample = weighted_sample(input, window, samples, t);
            for lane in 0..4 {
                let current = sample + twice_c[lane] * previous[lane] - prior[lane];
                prior[lane] = previous[lane];
                previous[lane] = current;
            }
        }
        for lane in 0..active {
            let (phase_s, phase_c) = (angles[lane] * (input.len() - 1) as f64).sin_cos();
            let real = previous[lane] - rotations_c[lane] * prior[lane];
            let imaginary = rotations_s[lane] * prior[lane];
            result[2 * (first + lane - 1)] = real * phase_c + imaginary * phase_s;
            result[2 * (first + lane - 1) + 1] = real * phase_s - imaginary * phase_c;
        }
    }
    result
}

fn step(
    gram: &Gram,
    inverse: [f64; 3],
    amplitudes: &mut [f64; 2 * MAX_HARMONICS],
    gradient: &mut [f64; 2 * MAX_HARMONICS],
    h: usize,
    harmonics: usize,
) -> bool {
    let [cc, cs, ss] = inverse;
    let real = cc * gradient[2 * h - 2] + cs * gradient[2 * h - 1];
    let imaginary = cs * gradient[2 * h - 2] + ss * gradient[2 * h - 1];
    amplitudes[2 * h - 2] += real;
    amplitudes[2 * h - 1] += imaginary;
    let norm = amplitudes[2 * h - 2].powi(2) + amplitudes[2 * h - 1].powi(2);
    if !norm.is_finite() || norm > PARAMETER_LIMIT * PARAMETER_LIMIT {
        return false;
    }
    for j in 1..=harmonics {
        let block = gram.block(j, h);
        gradient[2 * j - 2] -= block[0][0] * real + block[0][1] * imaginary;
        gradient[2 * j - 1] -= block[1][0] * real + block[1][1] * imaginary;
    }
    true
}

fn inverse_block(block: [[f64; 2]; 2], minimum: f64) -> Option<[f64; 3]> {
    if block.iter().flatten().any(|value| !value.is_finite()) {
        return None;
    }
    let [cc, cs] = block[0];
    let ss = block[1][1];
    let eigenvalue = (cc + ss - ((cc - ss).powi(2) + 4.0 * cs * cs).sqrt()) * 0.5;
    let determinant = cc * ss - cs * cs;
    if !eigenvalue.is_finite()
        || eigenvalue < minimum * 0.999999
        || !determinant.is_finite()
        || determinant <= 0.0
    {
        return None;
    }
    Some([ss / determinant, -cs / determinant, cc / determinant])
}

pub(crate) fn correct(
    input: &[f32],
    f0: usize,
    positions: &[usize; 960],
    window: &[f32],
    values: &mut [f32; 960],
) -> bool {
    if !(75..=400).contains(&f0)
        || !matches!(input.len(), 600 | 1080)
        || window.len() != 2 * (input.len() - OVERLAP)
    {
        return false;
    }
    let samples = input.len() - OVERLAP;
    let harmonics = 8000 / f0;
    let mut amplitudes = [0.0; 2 * MAX_HARMONICS];
    for (&position, &value) in positions.iter().zip(values.iter()) {
        if position != usize::MAX {
            if position >= 2 * harmonics || !value.is_finite() {
                return false;
            }
            amplitudes[position] = f64::from(value);
        }
    }
    if amplitudes[..2 * harmonics]
        .as_chunks::<2>()
        .0
        .iter()
        .any(|pair| pair[0].powi(2) + pair[1].powi(2) > PARAMETER_LIMIT * PARAMETER_LIMIT)
    {
        return false;
    }
    let gram = Gram::new(samples, f0, window);
    let mut gradient = right_hand_side(input, window, samples, f0);
    let mut inverse = [[0.0; 3]; MAX_HARMONICS];
    let minimum =
        ((samples - OVERLAP) as f64 - 1.0 / (std::f64::consts::TAU * 75.0 / 48000.0).sin()) * 0.5;
    for h in 1..=harmonics {
        let block = gram.block(h, h);
        let Some(inverted) = inverse_block(block, minimum) else {
            return false;
        };
        inverse[h - 1] = inverted;
        for j in 1..=harmonics {
            let block = gram.block(h, j);
            let real = amplitudes[2 * j - 2];
            let imaginary = amplitudes[2 * j - 1];
            gradient[2 * h - 2] -= block[0][0] * real + block[0][1] * imaginary;
            gradient[2 * h - 1] -= block[1][0] * real + block[1][1] * imaginary;
        }
    }
    for descending in [false, true] {
        for index in 0..harmonics {
            let h = if descending {
                harmonics - index
            } else {
                index + 1
            };
            if !step(
                &gram,
                inverse[h - 1],
                &mut amplitudes,
                &mut gradient,
                h,
                harmonics,
            ) {
                return false;
            }
        }
    }
    for (value, &position) in values.iter_mut().zip(positions) {
        if position != usize::MAX {
            *value = amplitudes[position] as f32;
        }
    }
    #[cfg(test)]
    CORRECTIONS.with(|count| count.set(count.get() + 1));
    true
}

#[cfg(test)]
std::thread_local! {
    static ORIGINALS: std::cell::Cell<[(f32, usize); 8]> = const {
        std::cell::Cell::new([(f32::NEG_INFINITY, 0); 8])
    };
    static ATTEMPTS: std::cell::Cell<(usize, usize)> = const { std::cell::Cell::new((0, 0)) };
    static GENERIC_BLOCKS: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    static SCORES: std::cell::Cell<(f32, f32)> = const {
        std::cell::Cell::new((f32::NAN, f32::NAN))
    };
    static CORRECTIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
}

#[cfg(test)]
pub(crate) fn reset_trials() {
    ORIGINALS.with(|trials| trials.set([(f32::NEG_INFINITY, 0); 8]));
    ATTEMPTS.with(|attempts| attempts.set((0, 0)));
}

#[cfg(test)]
pub(crate) fn original_trial(f0: usize, score: f32) {
    ORIGINALS.with(|trials| {
        let mut values = trials.get();
        let index = values.iter().position(|&(_, pitch)| pitch == 0).unwrap();
        values[index] = (score, f0);
        trials.set(values);
    });
}

#[cfg(test)]
pub(crate) fn attempted_trial(f0: usize) {
    ATTEMPTS.with(|attempts| attempts.set((attempts.get().0 + 1, f0)));
}

#[cfg(test)]
pub(crate) fn selection_score(joint: bool, score: f32) {
    SCORES.with(|scores| {
        scores.set(if joint {
            (scores.get().0, score)
        } else {
            (score, f32::NAN)
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transform::MdctPlan;

    fn folded(values: &[f64], samples: usize, window: &[f32]) -> Vec<f64> {
        let padding = (samples - OVERLAP) / 2;
        let weighted: Vec<_> = values
            .iter()
            .enumerate()
            .map(|(t, &value)| value * f64::from(window[padding + t]))
            .collect();
        let mut result = Vec::with_capacity(samples);
        for t in 0..OVERLAP / 2 {
            result.push(weighted[t] - weighted[OVERLAP - 1 - t]);
        }
        result.extend_from_slice(&weighted[OVERLAP..samples]);
        for t in 0..OVERLAP / 2 {
            result.push(weighted[samples + t] + weighted[samples + OVERLAP - 1 - t]);
        }
        result
    }

    fn direct_columns(samples: usize, f0: usize, window: &[f32]) -> Vec<Vec<f64>> {
        let mut columns = Vec::new();
        for h in 1..=8000 / f0 {
            let angle = std::f64::consts::TAU * (h * f0) as f64 / 48000.0;
            for imaginary in [false, true] {
                let values: Vec<_> = (0..samples + OVERLAP)
                    .map(|t| {
                        if imaginary {
                            (angle * t as f64).sin()
                        } else {
                            (angle * t as f64).cos()
                        }
                    })
                    .collect();
                columns.push(folded(&values, samples, window));
            }
        }
        columns
    }

    fn dot(a: &[f64], b: &[f64]) -> f64 {
        a.iter().zip(b).map(|(&x, &y)| x * y).sum()
    }

    fn close(actual: f64, expected: f64, label: &str) {
        assert!(
            (actual - expected).abs() <= 5e-8,
            "{label}: actual={actual:.12} expected={expected:.12} error={:.12}",
            actual - expected
        );
    }

    fn input(samples: usize) -> Vec<f32> {
        let mut seed = 0x94eaf325u32;
        (0..samples + OVERLAP)
            .map(|t| {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                let noise = f64::from(seed) / f64::from(u32::MAX) - 0.5;
                (0.31 * (std::f64::consts::TAU * 137.5 * t as f64 / 48000.0).sin()
                    + 0.19 * (std::f64::consts::TAU * 820.0 * t as f64 / 48000.0 + 0.7).cos()
                    + 0.05 * noise) as f32
            })
            .collect()
    }

    #[test]
    fn paired_blocks_match_generic_bits_for_every_harmonic_pair() {
        for samples in [480, 960] {
            let plan = MdctPlan::new(samples, OVERLAP).unwrap();
            for f0 in 75..=400 {
                let gram = Gram::new(samples, f0, plan.window());
                for h in 1..=8000 / f0 {
                    for j in 1..=8000 / f0 {
                        assert_eq!(
                            gram.block(h, j).map(|row| row.map(f64::to_bits)),
                            gram.generic_block(h, j).map(|row| row.map(f64::to_bits)),
                            "N={samples} f0={f0} h={h} j={j}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn paired_corrections_match_generic_parameters_and_rollbacks() {
        for samples in [480, 960] {
            let plan = MdctPlan::new(samples, OVERLAP).unwrap();
            let noise = input(samples);
            for f0 in [75, 120, 137, 177, 200, 399, 400] {
                let mut positions = [usize::MAX; 960];
                crate::harmonic::layout(f0, &mut positions);
                for pattern in 0..10 {
                    let source: Vec<_> = (0..samples + OVERLAP)
                        .map(|t| match pattern {
                            0 => 0.0,
                            1 => f32::from(u8::from(t == 0)),
                            2 => f32::from(u8::from(t == OVERLAP - 1)),
                            3 => f32::from(u8::from(t == samples)),
                            4 => f32::from(u8::from(t == samples + OVERLAP - 1)),
                            5 => {
                                f32::from(u8::from(t == 13))
                                    - f32::from(u8::from(t == OVERLAP - 14))
                            }
                            6 => {
                                if t % 2 == 0 {
                                    1.0
                                } else {
                                    -1.0
                                }
                            }
                            7 => f32::from_bits(1) * if t % 2 == 0 { 1.0 } else { -1.0 },
                            8 => {
                                (0.4 * (std::f64::consts::TAU * f0 as f64 * t as f64 / 48000.0)
                                    .sin()) as f32
                            }
                            _ => noise[t],
                        })
                        .collect();
                    let mut initial = [0.0; 960];
                    crate::harmonic::analyze(&source, f0, &positions, &mut initial);
                    for invalid in [None, Some(f32::NAN), Some(f32::INFINITY), Some(1e20)] {
                        let mut paired = initial;
                        if let Some(value) = invalid {
                            paired[0] = value;
                        }
                        let mut generic = paired;
                        let paired_result =
                            correct(&source, f0, &positions, plan.window(), &mut paired);
                        GENERIC_BLOCKS.with(|flag| flag.set(true));
                        let generic_result =
                            correct(&source, f0, &positions, plan.window(), &mut generic);
                        GENERIC_BLOCKS.with(|flag| flag.set(false));
                        assert_eq!(paired_result, generic_result);
                        assert_eq!(
                            paired.map(f32::to_bits),
                            generic.map(f32::to_bits),
                            "N={samples} f0={f0} pattern={pattern}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn analytic_folded_gram_and_rhs_match_direct_stored_window_trigonometry() {
        for samples in [480, 960] {
            let plan = MdctPlan::new(samples, OVERLAP).unwrap();
            let window = plan.window();
            let source = input(samples);
            let target = folded(
                &source.iter().map(|&x| f64::from(x)).collect::<Vec<_>>(),
                samples,
                window,
            );
            for f0 in 75..=400 {
                let harmonics = 8000 / f0;
                let gram = Gram::new(samples, f0, window);
                let columns = direct_columns(samples, f0, window);
                let rhs = right_hand_side(&source, window, samples, f0);
                let minimum = ((samples - OVERLAP) as f64
                    - 1.0 / (std::f64::consts::TAU * 75.0 / 48000.0).sin())
                    * 0.5;
                for h in 1..=harmonics {
                    close(
                        rhs[2 * h - 2],
                        dot(&columns[2 * h - 2], &target),
                        "normal rhs cosine",
                    );
                    close(
                        rhs[2 * h - 1],
                        dot(&columns[2 * h - 1], &target),
                        "normal rhs sine",
                    );
                    let diagonal = gram.block(h, h);
                    let cc = diagonal[0][0];
                    let cs = diagonal[0][1];
                    let ss = diagonal[1][1];
                    let eigenvalue = (cc + ss - ((cc - ss).powi(2) + 4.0 * cs * cs).sqrt()) * 0.5;
                    assert!(
                        eigenvalue >= minimum - 5e-8,
                        "samples={samples} f0={f0} h={h}"
                    );
                    for j in 1..=harmonics {
                        if !matches!(f0, 75 | 76 | 120 | 239 | 400)
                            && j != h
                            && j != 1
                            && j != harmonics
                            && j.abs_diff(h) != 1
                        {
                            continue;
                        }
                        let block = gram.block(h, j);
                        let transpose = gram.block(j, h);
                        for row in 0..2 {
                            for column in 0..2 {
                                close(
                                    block[row][column],
                                    dot(&columns[2 * h - 2 + row], &columns[2 * j - 2 + column]),
                                    "Gram direct dot",
                                );
                                close(block[row][column], transpose[column][row], "Gram transpose");
                            }
                        }
                    }
                }
                let padding = (samples - OVERLAP) / 2;
                let omega = std::f64::consts::TAU * f0 as f64 / 48000.0;
                for moment in 0..=2 * harmonics {
                    let angle = moment as f64 * omega;
                    let mut diagonal = Complex::ZERO;
                    for t in 0..samples + OVERLAP {
                        let weight = f64::from(window[padding + t]);
                        diagonal =
                            diagonal.add(Complex::phase(angle * t as f64).scale(weight * weight));
                    }
                    let mut reflected = Complex::ZERO;
                    for t in 0..OVERLAP {
                        let weight = f64::from(window[padding + t])
                            * f64::from(window[padding + OVERLAP - 1 - t]);
                        reflected = reflected.add(Complex::phase(-angle * t as f64).scale(weight));
                    }
                    close(
                        gram.diagonal[moment].real,
                        diagonal.real,
                        "diagonal kernel real",
                    );
                    close(
                        gram.diagonal[moment].imaginary,
                        diagonal.imaginary,
                        "diagonal kernel imaginary",
                    );
                    close(
                        gram.reflected[moment].real,
                        reflected.real,
                        "reflected kernel real",
                    );
                    close(
                        gram.reflected[moment].imaginary,
                        reflected.imaginary,
                        "reflected kernel imaginary",
                    );
                }
                assert_eq!(gram.diagonal[0].imaginary.to_bits(), 0.0f64.to_bits());
                assert_eq!(
                    gram.reflected[0].imaginary.abs().to_bits(),
                    0.0f64.to_bits()
                );
            }
        }
    }

    #[test]
    fn defining_cosine_energy_matches_independent_fold_for_boundary_and_noise_vectors() {
        for samples in [480, 960] {
            let plan = MdctPlan::new(samples, OVERLAP).unwrap();
            let window = plan.window();
            let padding = (samples - OVERLAP) / 2;
            let source = input(samples);
            for pattern in 0..10 {
                let values: Vec<_> = (0..samples + OVERLAP)
                    .map(|t| match pattern {
                        0 => 1.0,
                        1 => {
                            if t % 2 == 0 {
                                1.0
                            } else {
                                -1.0
                            }
                        }
                        2 => f64::from(u8::from(t == 0)),
                        3 => f64::from(u8::from(t == OVERLAP - 1)),
                        4 => f64::from(u8::from(t == samples)),
                        5 => f64::from(u8::from(t == samples + OVERLAP - 1)),
                        6 => f64::from(u8::from(t == 13)) - f64::from(u8::from(t == OVERLAP - 14)),
                        7 => {
                            f64::from(u8::from(t == samples + 13))
                                + f64::from(u8::from(t == samples + OVERLAP - 14))
                        }
                        8 => f64::from(f32::from_bits(1)) * if t % 2 == 0 { 1.0 } else { -1.0 },
                        _ => f64::from(source[t]),
                    })
                    .collect();
                let folded = folded(&values, samples, window);
                let expected = 2.0 / samples as f64 * dot(&folded, &folded);
                let mut actual = 0.0;
                for k in 0..samples {
                    let coefficient: f64 = values
                        .iter()
                        .enumerate()
                        .map(|(t, &value)| {
                            let index = padding + t;
                            let angle = std::f64::consts::PI / samples as f64
                                * (index as f64 + 0.5 + samples as f64 * 0.5)
                                * (k as f64 + 0.5);
                            value * f64::from(window[index]) * angle.cos()
                        })
                        .sum::<f64>()
                        * (2.0 / samples as f64);
                    actual += coefficient * coefficient;
                }
                close(actual, expected, "defining MDCT energy");
            }
        }
    }

    #[test]
    fn coordinate_steps_reduce_independently_evaluated_folded_residual() {
        for samples in [480, 960] {
            let plan = MdctPlan::new(samples, OVERLAP).unwrap();
            let window = plan.window();
            for f0 in [75, 120, 177, 239, 400] {
                let source = input(samples);
                let target = folded(
                    &source.iter().map(|&x| f64::from(x)).collect::<Vec<_>>(),
                    samples,
                    window,
                );
                let harmonics = 8000 / f0;
                let gram = Gram::new(samples, f0, window);
                let columns = direct_columns(samples, f0, window);
                let mut amplitudes = [0.0; 2 * MAX_HARMONICS];
                let mut gradient = right_hand_side(&source, window, samples, f0);
                let mut residual = target.clone();
                for descending in [false, true] {
                    for index in 0..harmonics {
                        let h = if descending {
                            harmonics - index
                        } else {
                            index + 1
                        };
                        let block = gram.block(h, h);
                        let cc = block[0][0];
                        let cs = block[0][1];
                        let ss = block[1][1];
                        let determinant = cc * ss - cs * cs;
                        let inverse = [ss / determinant, -cs / determinant, cc / determinant];
                        let gc = gradient[2 * h - 2];
                        let gs = gradient[2 * h - 1];
                        close(
                            gc,
                            dot(&columns[2 * h - 2], &residual),
                            "coordinate residual cosine",
                        );
                        close(
                            gs,
                            dot(&columns[2 * h - 1], &residual),
                            "coordinate residual sine",
                        );
                        let predicted = inverse[0] * gc * gc
                            + 2.0 * inverse[1] * gc * gs
                            + inverse[2] * gs * gs;
                        let before = dot(&residual, &residual);
                        assert!(step(
                            &gram,
                            inverse,
                            &mut amplitudes,
                            &mut gradient,
                            h,
                            harmonics
                        ));
                        residual.clone_from(&target);
                        for (column, &amplitude) in columns.iter().zip(&amplitudes) {
                            for (sample, &value) in residual.iter_mut().zip(column) {
                                *sample -= value * amplitude;
                            }
                        }
                        let after = dot(&residual, &residual);
                        close(before - after, predicted, "coordinate quadratic reduction");
                        assert!(after <= before + 5e-8);
                    }
                }
            }
        }
    }

    #[test]
    fn correction_retains_padding_and_rejects_unsafe_values_without_mutation() {
        for samples in [480, 960] {
            let plan = MdctPlan::new(samples, OVERLAP).unwrap();
            let source = input(samples);
            for f0 in 75..=400 {
                let mut positions = [usize::MAX; 960];
                crate::harmonic::layout(f0, &mut positions);
                let mut values = [0.0; 960];
                crate::harmonic::analyze(&source, f0, &positions, &mut values);
                let old = values;
                assert!(correct(&source, f0, &positions, plan.window(), &mut values));
                let columns = direct_columns(samples, f0, plan.window());
                let target = folded(
                    &source.iter().map(|&x| f64::from(x)).collect::<Vec<_>>(),
                    samples,
                    plan.window(),
                );
                let error = |parameters: &[f32; 960]| {
                    let mut residual = target.clone();
                    for (&position, &amplitude) in positions.iter().zip(parameters) {
                        if position != usize::MAX {
                            for (value, &basis) in residual.iter_mut().zip(&columns[position]) {
                                *value -= f64::from(amplitude) * basis;
                            }
                        }
                    }
                    dot(&residual, &residual)
                };
                assert!(error(&values) <= error(&old) + 1e-6, "N={samples} f0={f0}");
                for (&position, &value) in positions.iter().zip(&values) {
                    if position == usize::MAX {
                        assert_eq!(value.to_bits(), 0.0f32.to_bits());
                    }
                }
            }
            let mut positions = [usize::MAX; 960];
            crate::harmonic::layout(120, &mut positions);
            let mut values = [0.0; 960];
            assert!(correct(
                &vec![0.0; source.len()],
                120,
                &positions,
                plan.window(),
                &mut values
            ));
            assert!(values.iter().all(|x| *x == 0.0));
            values[0] = f32::INFINITY;
            let old = values.map(f32::to_bits);
            assert!(!correct(
                &source,
                120,
                &positions,
                plan.window(),
                &mut values
            ));
            assert_eq!(old, values.map(f32::to_bits));
            values[0] = 1e20;
            let old = values.map(f32::to_bits);
            assert!(!correct(
                &source,
                120,
                &positions,
                plan.window(),
                &mut values
            ));
            assert_eq!(old, values.map(f32::to_bits));
            values[0] = 0.2;
            let old = values.map(f32::to_bits);
            for invalid in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let mut bad_input = source.clone();
                bad_input[17] = invalid;
                assert!(!correct(
                    &bad_input,
                    120,
                    &positions,
                    plan.window(),
                    &mut values
                ));
                assert_eq!(old, values.map(f32::to_bits));
                let mut bad_window = plan.window().to_vec();
                bad_window[(samples - OVERLAP) / 2 + 17] = invalid;
                assert!(!correct(&source, 120, &positions, &bad_window, &mut values));
                assert_eq!(old, values.map(f32::to_bits));
            }
        }
    }

    #[test]
    fn block_inversion_refuses_nonfinite_nonpositive_and_overflowing_determinants() {
        for block in [
            [[0.0, 0.0], [0.0, 0.0]],
            [[200.0, 201.0], [201.0, 200.0]],
            [[f64::NAN, 0.0], [0.0, 200.0]],
            [[200.0, f64::INFINITY], [f64::INFINITY, 200.0]],
            [[f64::INFINITY, 0.0], [0.0, f64::INFINITY]],
            [[200.0, 0.0], [f64::NAN, 200.0]],
            [[1e200, 0.0], [0.0, 1e200]],
            [[-200.0, 0.0], [0.0, -200.0]],
        ] {
            assert!(inverse_block(block, 129.0).is_none());
        }
        let inverse = inverse_block([[200.0, 10.0], [10.0, 300.0]], 129.0).unwrap();
        close(inverse[0], 300.0 / 59900.0, "inverse cc");
        close(inverse[1], -10.0 / 59900.0, "inverse cs");
        close(inverse[2], 200.0 / 59900.0, "inverse ss");
    }

    #[test]
    fn no_eligible_original_render_skips_correction() {
        for duration in [crate::FrameDuration::Ms10, crate::FrameDuration::Ms20] {
            let config = crate::Config::new(12_000)
                .unwrap()
                .with_frame_duration(duration)
                .with_harmonic_speech(true);
            let mut encoder = crate::Encoder::new(config);
            let mut packet = [0; crate::MAX_PACKET_BYTES];
            encoder
                .encode(&vec![0.0; config.frame_samples()], &mut packet)
                .unwrap();
            ATTEMPTS.with(|attempts| assert_eq!(attempts.get(), (0, 0)));
            encoder.finish(&mut packet).unwrap();
            ATTEMPTS.with(|attempts| assert_eq!(attempts.get(), (0, 0)));
        }
    }

    #[test]
    fn quantized_trial_selection_preserves_the_original_candidate_floor() {
        CORRECTIONS.with(|count| count.set(0));
        for duration in [crate::FrameDuration::Ms10, crate::FrameDuration::Ms20] {
            for bitrate in [12_000, 12_345, 14_000, 16_000] {
                let config = crate::Config::new(bitrate)
                    .unwrap()
                    .with_frame_duration(duration)
                    .with_harmonic_speech(true);
                let mut encoder = crate::Encoder::new(config);
                let samples = config.frame_samples();
                for pitch in [120, 137, 177, 240, 400] {
                    for frame in 0..3 {
                        let source: Vec<_> = (0..samples)
                            .map(|t| {
                                let phase = std::f64::consts::TAU
                                    * pitch as f64
                                    * (frame * samples + t) as f64
                                    / 48000.0;
                                (0.4 * phase.sin() + 0.11 * (3.0 * phase + 0.7).cos()) as f32
                            })
                            .collect();
                        let mut packet = [0; crate::MAX_PACKET_BYTES];
                        let length = encoder.encode(&source, &mut packet).unwrap();
                        assert!(length <= config.maximum_packet_bytes());
                        let expected = ORIGINALS.with(|trials| {
                            trials
                                .get()
                                .into_iter()
                                .fold((f32::NEG_INFINITY, 0), |best, next| {
                                    if next.0 > best.0 { next } else { best }
                                })
                        });
                        ATTEMPTS.with(|attempts| {
                            assert_eq!(attempts.get(), (usize::from(expected.1 != 0), expected.1));
                        });
                        SCORES.with(|scores| {
                            let (old, new) = scores.get();
                            assert!(old.is_finite() && new.is_finite());
                            assert!(
                                new >= old,
                                "old={old} new={new} duration={duration:?} bitrate={bitrate}"
                            );
                        });
                    }
                }
                let mut packet = [0; crate::MAX_PACKET_BYTES];
                encoder.finish(&mut packet).unwrap();
                SCORES.with(|scores| {
                    let (old, new) = scores.get();
                    assert!(new >= old);
                });
            }
        }
        CORRECTIONS.with(|count| assert!(count.get() > 0));
    }
}
