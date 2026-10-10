// SPDX-License-Identifier: MIT
pub(crate) const MIN_F0: usize = 75;
pub(crate) const MAX_F0: usize = 400;
pub(crate) fn layout(f0: usize, positions: &mut [usize; 960]) -> ([usize; 4], [usize; 5]) {
    positions.fill(usize::MAX);
    let mut dimensions = [0; 4];
    let mut edges = [0; 5];
    let frequency_edges: [usize; 5] = [0, 1000, 2000, 4000, 8001];
    for group in 0..4 {
        let first = frequency_edges[group].div_ceil(f0);
        let first = first.max(1);
        let last = (frequency_edges[group + 1] - 1) / f0;
        let count = if last >= first {
            2 * (last - first + 1)
        } else {
            0
        };
        let dimension = [4, 8, 16, 20, 40, 80, 160]
            .into_iter()
            .find(|&d| d >= count)
            .expect("bounded harmonic parameters");
        dimensions[group] = dimension;
        edges[group + 1] = edges[group] + dimension;
        let mut index = edges[group];
        for harmonic in first..=last {
            positions[index] = 2 * (harmonic - 1);
            positions[index + 1] = 2 * (harmonic - 1) + 1;
            index += 2;
        }
    }
    (dimensions, edges)
}

pub(crate) fn analyze(input: &[f32], f0: usize, positions: &[usize; 960], values: &mut [f32; 960]) {
    values.fill(0.0);
    let harmonics = 8000 / f0;
    let mut amplitudes = [0.0f32; 214];
    for first in (1..=harmonics).step_by(8) {
        let active = (harmonics - first + 1).min(8);
        let mut angles = [0.0f64; 8];
        let mut rotations_s = [0.0f64; 8];
        let mut rotations_c = [0.0f64; 8];
        let mut twice_c = [0.0f64; 8];
        for lane in 0..active {
            angles[lane] = std::f64::consts::TAU * ((first + lane) * f0) as f64 / 48000.0;
            (rotations_s[lane], rotations_c[lane]) = angles[lane].sin_cos();
            twice_c[lane] = 2.0 * rotations_c[lane];
        }
        let mut previous = [0.0f64; 8];
        let mut prior = [0.0f64; 8];
        for &sample in input {
            let sample = f64::from(sample);
            for lane in 0..8 {
                let current = sample + twice_c[lane] * previous[lane] - prior[lane];
                prior[lane] = previous[lane];
                previous[lane] = current;
            }
        }
        for lane in 0..active {
            let angle = angles[lane];
            let rotation_c = rotations_c[lane];
            let rotation_s = rotations_s[lane];
            let (phase_s, phase_c) = (angle * (input.len() - 1) as f64).sin_cos();
            let terminal_real = previous[lane] - rotation_c * prior[lane];
            let terminal_imaginary = rotation_s * prior[lane];
            let xc = terminal_real * phase_c + terminal_imaginary * phase_s;
            let xs = terminal_real * phase_s - terminal_imaginary * phase_c;
            let factor = (angle * input.len() as f64).sin() / rotation_s;
            let cosine_sum = factor * phase_c;
            let sine_sum = factor * phase_s;
            let cc = (input.len() as f64 + cosine_sum) * 0.5;
            let ss = (input.len() as f64 - cosine_sum) * 0.5;
            let cs = sine_sum * 0.5;
            let determinant = cc * ss - cs * cs;
            if determinant > 1e-12 {
                amplitudes[2 * (first + lane - 1)] = ((xc * ss - xs * cs) / determinant) as f32;
                amplitudes[2 * (first + lane - 1) + 1] = ((xs * cc - xc * cs) / determinant) as f32;
            }
        }
    }
    for (value, &position) in values.iter_mut().zip(positions) {
        if position != usize::MAX {
            *value = amplitudes[position];
        }
    }
}

pub(crate) fn synthesize(
    input: &mut [f32],
    samples: usize,
    f0: usize,
    positions: &[usize; 960],
    values: &[f32; 960],
) {
    input.fill(0.0);
    let mut amplitudes = [0.0f32; 214];
    for (&value, &position) in values.iter().zip(positions) {
        if position != usize::MAX {
            amplitudes[position] = value;
        }
    }
    let start = (samples - 120) / 2;
    fn render<const LANES: usize>(
        output: &mut [f32],
        cosine: &[f64; 4],
        sine: &[f64; 4],
        rotation_s: &[f64; 4],
        rotation_c: &[f64; 4],
    ) {
        let mut sin = [0.0f64; 4];
        let mut cos = [1.0f64; 4];
        for sample in output {
            let mut terms = [0.0f32; 4];
            for lane in 0..LANES {
                terms[lane] = (cosine[lane] * cos[lane] + sine[lane] * sin[lane]) as f32;
                (sin[lane], cos[lane]) = (
                    sin[lane] * rotation_c[lane] + cos[lane] * rotation_s[lane],
                    cos[lane] * rotation_c[lane] - sin[lane] * rotation_s[lane],
                );
            }
            // Retain the scalar harmonic order and its single-precision rounding.
            for term in &terms[..LANES] {
                *sample += term;
            }
        }
    }
    let mut count = 0;
    let mut cosine = [0.0f64; 4];
    let mut sine = [0.0f64; 4];
    let mut rotation_s = [0.0f64; 4];
    let mut rotation_c = [0.0f64; 4];
    for harmonic in 1..=8000 / f0 {
        let real = f64::from(amplitudes[2 * (harmonic - 1)]);
        let imaginary = f64::from(amplitudes[2 * (harmonic - 1) + 1]);
        if real == 0.0 && imaginary == 0.0 {
            continue;
        }
        cosine[count] = real;
        sine[count] = imaginary;
        let angle = std::f64::consts::TAU * (harmonic * f0) as f64 / 48000.0;
        (rotation_s[count], rotation_c[count]) = angle.sin_cos();
        count += 1;
        if count == 4 {
            render::<4>(
                &mut input[start..start + samples + 120],
                &cosine,
                &sine,
                &rotation_s,
                &rotation_c,
            );
            count = 0;
        }
    }
    let output = &mut input[start..start + samples + 120];
    match count {
        1 => render::<1>(output, &cosine, &sine, &rotation_s, &rotation_c),
        2 => render::<2>(output, &cosine, &sine, &rotation_s, &rotation_c),
        3 => render::<3>(output, &cosine, &sine, &rotation_s, &rotation_c),
        _ => {}
    }
}
pub(crate) fn cost_table(dimensions: usize) -> &'static [u16; 388] {
    let index = match dimensions {
        4 => 0,
        8 => 1,
        16 => 2,
        _ => unreachable!("small harmonic dimensions"),
    };
    &SMALL_COST[index]
}
const SMALL_COST: [[u16; 388]; 3] = [
    [
        0, 3, 6, 8, 10, 10, 11, 11, 12, 12, 13, 13, 13, 14, 14, 14, 14, 15, 15, 15, 15, 15, 16, 16,
        16, 16, 16, 16, 17, 17, 17, 17, 17, 17, 17, 18, 18, 18, 18, 18, 18, 18, 18, 18, 18, 19, 19,
        19, 19, 19, 19, 19, 19, 19, 19, 19, 19, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20, 20,
        20, 20, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 21, 22, 22,
        22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 22, 23,
        23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23, 23,
        23, 23, 23, 23, 23, 23, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24,
        24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 24, 25, 25,
        25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25,
        25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25, 25,
        26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26,
        26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26,
        26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 26, 27, 27, 27, 27, 27, 27, 27, 27,
        27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27,
        27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27,
        27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 27, 28,
        28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28, 28,
    ],
    [
        0, 4, 8, 10, 13, 15, 17, 19, 21, 22, 23, 23, 24, 25, 25, 26, 26, 27, 27, 28, 28, 29, 29,
        29, 30, 30, 31, 31, 31, 31, 32, 32, 32, 33, 33, 33, 33, 34, 34, 34, 34, 35, 35, 35, 35, 35,
        36, 36, 36, 36, 36, 37, 37, 37, 37, 37, 38, 38, 38, 38, 38, 38, 39, 39, 39, 39, 39, 39, 39,
        40, 40, 40, 40, 40, 40, 40, 40, 41, 41, 41, 41, 41, 41, 41, 41, 42, 42, 42, 42, 42, 42, 42,
        42, 42, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 43, 44, 44, 44, 44, 44, 44, 44, 44, 44, 44,
        44, 45, 45, 45, 45, 45, 45, 45, 45, 45, 45, 45, 45, 46, 46, 46, 46, 46, 46, 46, 46, 46, 46,
        46, 46, 46, 46, 47, 47, 47, 47, 47, 47, 47, 47, 47, 47, 47, 47, 47, 47, 47, 48, 48, 48, 48,
        48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 48, 49, 49, 49, 49, 49, 49, 49, 49, 49, 49,
        49, 49, 49, 49, 49, 49, 49, 49, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50, 50,
        50, 50, 50, 50, 50, 50, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51, 51,
        51, 51, 51, 51, 51, 52, 52, 52, 52, 52, 52, 52, 52, 52, 52, 52, 52, 52, 52, 52, 52, 52, 52,
        52, 52, 52, 52, 52, 52, 52, 53, 53, 53, 53, 53, 53, 53, 53, 53, 53, 53, 53, 53, 53, 53, 53,
        53, 53, 53, 53, 53, 53, 53, 53, 53, 53, 53, 53, 54, 54, 54, 54, 54, 54, 54, 54, 54, 54, 54,
        54, 54, 54, 54, 54, 54, 54, 54, 54, 54, 54, 54, 54, 54, 54, 54, 54, 54, 54, 55, 55, 55, 55,
        55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55, 55,
        55, 55, 55, 55, 55, 55, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56,
        56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56, 56,
    ],
    [
        0, 5, 10, 13, 16, 19, 22, 25, 27, 30, 32, 34, 37, 39, 41, 43, 45, 46, 46, 47, 48, 49, 50,
        50, 51, 52, 52, 53, 54, 54, 55, 55, 56, 56, 57, 58, 58, 59, 59, 59, 60, 60, 61, 61, 62, 62,
        63, 63, 63, 64, 64, 64, 65, 65, 66, 66, 66, 67, 67, 67, 68, 68, 68, 68, 69, 69, 69, 70, 70,
        70, 70, 71, 71, 71, 72, 72, 72, 72, 73, 73, 73, 73, 74, 74, 74, 74, 75, 75, 75, 75, 75, 76,
        76, 76, 76, 77, 77, 77, 77, 77, 78, 78, 78, 78, 78, 79, 79, 79, 79, 79, 79, 80, 80, 80, 80,
        80, 81, 81, 81, 81, 81, 81, 82, 82, 82, 82, 82, 82, 83, 83, 83, 83, 83, 83, 83, 84, 84, 84,
        84, 84, 84, 85, 85, 85, 85, 85, 85, 85, 86, 86, 86, 86, 86, 86, 86, 86, 87, 87, 87, 87, 87,
        87, 87, 88, 88, 88, 88, 88, 88, 88, 88, 89, 89, 89, 89, 89, 89, 89, 89, 89, 90, 90, 90, 90,
        90, 90, 90, 90, 90, 91, 91, 91, 91, 91, 91, 91, 91, 91, 92, 92, 92, 92, 92, 92, 92, 92, 92,
        92, 93, 93, 93, 93, 93, 93, 93, 93, 93, 93, 94, 94, 94, 94, 94, 94, 94, 94, 94, 94, 94, 95,
        95, 95, 95, 95, 95, 95, 95, 95, 95, 95, 96, 96, 96, 96, 96, 96, 96, 96, 96, 96, 96, 96, 97,
        97, 97, 97, 97, 97, 97, 97, 97, 97, 97, 97, 98, 98, 98, 98, 98, 98, 98, 98, 98, 98, 98, 98,
        98, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 99, 100, 100, 100, 100, 100, 100, 100,
        100, 100, 100, 100, 100, 100, 100, 101, 101, 101, 101, 101, 101, 101, 101, 101, 101, 101,
        101, 101, 101, 101, 102, 102, 102, 102, 102, 102, 102, 102, 102, 102, 102, 102, 102, 102,
        102, 103, 103, 103, 103, 103, 103, 103, 103, 103, 103, 103, 103, 103, 103, 103, 103, 104,
        104, 104, 104, 104, 104, 104, 104, 104, 104, 104, 104, 104, 104, 104, 104, 104, 105, 105,
        105, 105, 105, 105, 105, 105, 105, 105, 105, 105, 105, 105, 105, 105, 105, 105, 106, 106,
        106,
    ],
];

pub(crate) fn candidates(input: &[f32]) -> [(f32, usize); 8] {
    let mut finalists = [(f32::NEG_INFINITY, 0usize); 8];
    let mut prefix = (0usize, 0.0f64);
    let mut first = MIN_F0;
    while first <= MAX_F0 {
        let (end, lags, scores, count) = periodicity_batch(input, first, &mut prefix);
        let mut lane = 0;
        for f0 in first..end {
            let lag = (48000 + f0 / 2) / f0;
            while lane + 1 < count && lag < lags[lane] {
                lane += 1;
            }
            let score = if count > 0 && lag == lags[lane] {
                scores[lane]
            } else {
                0.0
            };
            if score < 0.55 {
                continue;
            }
            if score > finalists[7].0 {
                finalists[7] = (score, f0);
                finalists.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
            }
        }
        first = end;
    }
    finalists
}

fn periodicity_batch(
    input: &[f32],
    first: usize,
    prefix: &mut (usize, f64),
) -> (usize, [usize; 4], [f32; 4], usize) {
    let mut lags = [0usize; 4];
    let mut count = 0;
    let mut end = first;
    while end <= MAX_F0 {
        let lag = (48000 + end / 2) / end;
        if lag < input.len()
            && input.len() - lag >= input.len() / 3
            && (count == 0 || lag != lags[count - 1])
        {
            if count == 4 {
                break;
            }
            lags[count] = lag;
            count += 1;
        }
        end += 1;
    }
    let mut scores = [0.0f32; 4];
    if count == 0 {
        return (end, lags, scores, count);
    }
    let mut energy_b = [0.0f64; 4];
    for lane in 0..count {
        let pairs = input.len() - lags[lane];
        // Ascending pitches extend the same chronological prefix, without subtraction.
        for &sample in &input[prefix.0..pairs] {
            prefix.1 += f64::from(sample) * f64::from(sample);
        }
        prefix.0 = pairs;
        energy_b[lane] = prefix.1;
    }
    let (cross, energy_a) = match count {
        1 => correlate::<1>(input, &lags),
        2 => correlate::<2>(input, &lags),
        3 => correlate::<3>(input, &lags),
        4 => correlate::<4>(input, &lags),
        _ => unreachable!("bounded correlation lanes"),
    };
    for lane in 0..count {
        scores[lane] = if energy_a[lane] * energy_b[lane] < 1e-18 {
            0.0
        } else {
            (cross[lane] / (energy_a[lane] * energy_b[lane]).sqrt()) as f32
        };
    }
    (end, lags, scores, count)
}

fn correlate<const LANES: usize>(input: &[f32], lags: &[usize; 4]) -> ([f64; 4], [f64; 4]) {
    let mut cross = [0.0f64; 4];
    let mut energy_a = [0.0f64; 4];
    let common = input.len() - lags[0];
    for index in 0..common {
        let b = f64::from(input[index]);
        for lane in 0..LANES {
            let a = f64::from(input[index + lags[lane]]);
            cross[lane] += a * b;
            energy_a[lane] += a * a;
        }
    }
    for lane in 0..LANES {
        for index in common..input.len() - lags[lane] {
            let a = f64::from(input[index + lags[lane]]);
            let b = f64::from(input[index]);
            cross[lane] += a * b;
            energy_a[lane] += a * a;
        }
    }
    (cross, energy_a)
}

#[cfg(test)]
fn cached_periodicity(input: &[f32], f0: usize, previous: &mut (usize, f32)) -> f32 {
    let lag = (48000 + f0 / 2) / f0;
    if lag != previous.0 {
        *previous = (lag, periodicity(input, f0));
    }
    previous.1
}

#[cfg(test)]
fn periodicity(input: &[f32], f0: usize) -> f32 {
    let lag = (48000 + f0 / 2) / f0;
    if lag >= input.len() || input.len() - lag < input.len() / 3 {
        return 0.0;
    }
    let mut cross = 0.0f64;
    let mut energy_a = 0.0f64;
    let mut energy_b = 0.0f64;
    for (&a, &b) in input[lag..].iter().zip(input[..input.len() - lag].iter()) {
        cross += f64::from(a) * f64::from(b);
        energy_a += f64::from(a) * f64::from(a);
        energy_b += f64::from(b) * f64::from(b);
    }
    if energy_a * energy_b < 1e-18 {
        0.0
    } else {
        (cross / (energy_a * energy_b).sqrt()) as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sinusoidal_phase_and_amplitude_are_explicit_roundtrip_parameters() {
        for samples in [480, 960] {
            for f0 in [75, 137, 200, 400] {
                let mut positions = [usize::MAX; 960];
                let (_, edges) = layout(f0, &mut positions);
                let mut input = [0.0f32; 1080];
                for (index, value) in input[..samples + 120].iter_mut().enumerate() {
                    let angle = std::f64::consts::TAU * f0 as f64 * index as f64 / 48000.0;
                    *value = (0.3 * angle.cos() - 0.2 * angle.sin()) as f32;
                }
                let mut values = [0.0f32; 960];
                analyze(&input[..samples + 120], f0, &positions, &mut values);
                for index in 0..edges[4] {
                    match positions[index] {
                        0 => assert!((values[index] - 0.3).abs() < 1e-6),
                        1 => assert!((values[index] + 0.2).abs() < 1e-6),
                        _ => values[index] = 0.0,
                    }
                }
                let mut waveform = [0.0f32; 1920];
                synthesize(&mut waveform, samples, f0, &positions, &values);
                let start = (samples - 120) / 2;
                for (&actual, &expected) in waveform[start..start + samples + 120]
                    .iter()
                    .zip(&input[..samples + 120])
                {
                    assert!((actual - expected).abs() < 1e-6);
                }
            }
        }
    }

    #[test]
    fn accelerated_analysis_matches_direct_dot_products_for_every_pitch() {
        for samples in [480, 960] {
            for f0 in MIN_F0..=MAX_F0 {
                let mut positions = [usize::MAX; 960];
                let (_, edges) = layout(f0, &mut positions);
                for pattern in 0..5 {
                    let input: Vec<_> = (0..samples + 120)
                        .map(|index| match pattern {
                            0 => 1.0f32,
                            1 => {
                                if index % 2 == 0 {
                                    1.0
                                } else {
                                    -1.0
                                }
                            }
                            2 => {
                                if index == 0 || index == samples + 119 {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            3 => {
                                let mixed = (index as u64 + 0x1234_5678)
                                    .wrapping_mul(0x9e37_79b9_7f4a_7c15);
                                ((mixed >> 40) as f64 / 8388608.0 - 1.0) as f32
                            }
                            _ => {
                                let angle =
                                    std::f64::consts::TAU * f0 as f64 * index as f64 / 48000.0;
                                (0.3 * angle.cos() + 0.2 * (7.0 * angle).sin()) as f32
                            }
                        })
                        .collect();
                    let mut actual = [0.0f32; 960];
                    analyze(&input, f0, &positions, &mut actual);
                    let mut expected = [0.0f64; 214];
                    let mut scalar = [0.0f32; 214];
                    for harmonic in 1..=8000 / f0 {
                        let angle = std::f64::consts::TAU * (harmonic * f0) as f64 / 48000.0;
                        let (rotation_s, rotation_c) = angle.sin_cos();
                        let (mut previous, mut prior) = (0.0f64, 0.0f64);
                        for &sample in &input {
                            let current = f64::from(sample) + 2.0 * rotation_c * previous - prior;
                            prior = previous;
                            previous = current;
                        }
                        let (phase_s, phase_c) = (angle * (input.len() - 1) as f64).sin_cos();
                        let real = previous - rotation_c * prior;
                        let imaginary = rotation_s * prior;
                        let scalar_xc = real * phase_c + imaginary * phase_s;
                        let scalar_xs = real * phase_s - imaginary * phase_c;
                        let factor = (angle * input.len() as f64).sin() / rotation_s;
                        let scalar_cc = (input.len() as f64 + factor * phase_c) * 0.5;
                        let scalar_ss = (input.len() as f64 - factor * phase_c) * 0.5;
                        let scalar_cs = factor * phase_s * 0.5;
                        let scalar_det = scalar_cc * scalar_ss - scalar_cs * scalar_cs;
                        scalar[2 * (harmonic - 1)] =
                            ((scalar_xc * scalar_ss - scalar_xs * scalar_cs) / scalar_det) as f32;
                        scalar[2 * (harmonic - 1) + 1] =
                            ((scalar_xs * scalar_cc - scalar_xc * scalar_cs) / scalar_det) as f32;
                        let (mut cc, mut ss, mut cs, mut xc, mut xs) = (0.0, 0.0, 0.0, 0.0, 0.0);
                        for (index, &sample) in input.iter().enumerate() {
                            let (sin, cos) = (angle * index as f64).sin_cos();
                            cc += cos * cos;
                            ss += sin * sin;
                            cs += cos * sin;
                            xc += f64::from(sample) * cos;
                            xs += f64::from(sample) * sin;
                        }
                        let determinant = cc * ss - cs * cs;
                        expected[2 * (harmonic - 1)] = (xc * ss - xs * cs) / determinant;
                        expected[2 * (harmonic - 1) + 1] = (xs * cc - xc * cs) / determinant;
                    }
                    for index in 0..edges[4] {
                        let position = positions[index];
                        if position == usize::MAX {
                            assert_eq!(actual[index], 0.0);
                            continue;
                        }
                        assert_eq!(
                            actual[index].to_bits(),
                            scalar[position].to_bits(),
                            "scalar sample={samples} f0={f0} pattern={pattern} parameter={position}"
                        );
                        assert!(
                            (f64::from(actual[index]) - expected[position]).abs() < 2e-6,
                            "samples={samples} f0={f0} pattern={pattern} parameter={position}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn rounded_lag_reuse_preserves_all_scores_and_eight_pitch_finalists() {
        for samples in [480, 960] {
            for pattern in 0..9 {
                let input: Vec<_> = (0..samples + 120)
                    .map(|index| {
                        let mixed =
                            (index as u64 + 0x1234_5678).wrapping_mul(0x9e37_79b9_7f4a_7c15);
                        let noise = ((mixed >> 40) as f64 / 8388608.0 - 1.0) as f32;
                        let angle = std::f64::consts::TAU * 200.0 * index as f64 / 48000.0;
                        let voice = (0.3 * angle.cos() + 0.2 * (7.0 * angle).sin()) as f32;
                        match pattern {
                            0 => 0.0f32,
                            1 => 1.0f32,
                            2 => {
                                if index % 2 == 0 {
                                    1.0
                                } else {
                                    -1.0
                                }
                            }
                            3 => {
                                if index == 0 || index == samples + 119 {
                                    1.0
                                } else {
                                    0.0
                                }
                            }
                            4 => noise,
                            5 => voice,
                            6 => (voice + noise * 0.03).clamp(-1.0, 1.0),
                            7 => (voice + noise * 0.10).clamp(-1.0, 1.0),
                            _ => (voice + noise * 0.30).clamp(-1.0, 1.0),
                        }
                    })
                    .collect();
                let mut prefix = (0usize, 0.0f64);
                let mut first = MIN_F0;
                while first <= MAX_F0 {
                    let (end, lags, scores, count) = periodicity_batch(&input, first, &mut prefix);
                    for f0 in first..end {
                        let lag = (48000 + f0 / 2) / f0;
                        let actual = lags[..count]
                            .iter()
                            .position(|&candidate| candidate == lag)
                            .map_or(0.0, |lane| scores[lane]);
                        assert_eq!(
                            actual.to_bits(),
                            periodicity(&input, f0).to_bits(),
                            "batched samples={samples} pattern={pattern} f0={f0}"
                        );
                    }
                    first = end;
                }
                let mut previous = (usize::MAX, 0.0f32);
                let mut uncached = [(f32::NEG_INFINITY, 0usize); 8];
                for f0 in MIN_F0..=MAX_F0 {
                    let score = periodicity(&input, f0);
                    assert_eq!(
                        cached_periodicity(&input, f0, &mut previous).to_bits(),
                        score.to_bits(),
                        "samples={samples} pattern={pattern} f0={f0}"
                    );
                    if score < 0.55 {
                        continue;
                    }
                    if score > uncached[7].0 {
                        uncached[7] = (score, f0);
                        uncached.sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
                    }
                }
                let cached = candidates(&input);
                for (actual, expected) in cached.iter().zip(uncached) {
                    assert_eq!(actual.0.to_bits(), expected.0.to_bits());
                    assert_eq!(actual.1, expected.1);
                }
            }
        }
    }

    #[test]
    fn batched_periodicity_preserves_every_score_and_duplicate_finalist() {
        for length in [0, 1, 119, 120, 180, 181, 599, 600, 601, 1079, 1080, 1081] {
            for pattern in 0..8 {
                let input: Vec<f32> = (0..length)
                    .map(|index| match pattern {
                        0 => 0.0,
                        1 => -0.0,
                        2 => 1.0,
                        3 => {
                            if index % 2 == 0 {
                                1.0
                            } else {
                                -1.0
                            }
                        }
                        4 => {
                            if index % 7 == 0 {
                                f32::from_bits(1)
                            } else {
                                -f32::from_bits(3)
                            }
                        }
                        5 => {
                            if index % 31 == 0 {
                                f32::MAX
                            } else {
                                1e-12
                            }
                        }
                        6 => ((index * 7919 % 997) as f32 / 997.0 - 0.5) * 1.3e-6,
                        _ => (std::f64::consts::TAU * 137.0 * index as f64 / 48000.0).sin() as f32,
                    })
                    .collect();
                let mut prefix = (0usize, 0.0f64);
                let mut first = MIN_F0;
                let mut evaluated = 0;
                let mut expected = [(f32::NEG_INFINITY, 0usize); 8];
                while first <= MAX_F0 {
                    let (end, lags, scores, count) = periodicity_batch(&input, first, &mut prefix);
                    assert!(end > first);
                    let mut lane = 0;
                    for f0 in first..end {
                        let lag = (48000 + f0 / 2) / f0;
                        while lane + 1 < count && lag < lags[lane] {
                            lane += 1;
                        }
                        let actual = if count > 0 && lag == lags[lane] {
                            scores[lane]
                        } else {
                            0.0
                        };
                        let score = periodicity(&input, f0);
                        assert_eq!(
                            actual.to_bits(),
                            score.to_bits(),
                            "length={length} pattern={pattern} f0={f0}"
                        );
                        evaluated += 1;
                        if score >= 0.55 && score > expected[7].0 {
                            expected[7] = (score, f0);
                            expected
                                .sort_unstable_by(|a, b| b.0.total_cmp(&a.0).then(a.1.cmp(&b.1)));
                        }
                    }
                    first = end;
                }
                assert_eq!(evaluated, MAX_F0 - MIN_F0 + 1);
                for (actual, expected) in candidates(&input).iter().zip(expected) {
                    assert_eq!(actual.0.to_bits(), expected.0.to_bits());
                    assert_eq!(actual.1, expected.1);
                }
            }
        }
    }

    #[test]
    fn harmonic_layout_has_unique_pairs_and_bounded_padding() {
        for f0 in MIN_F0..=MAX_F0 {
            let mut positions = [usize::MAX; 960];
            let (dimensions, edges) = layout(f0, &mut positions);
            assert!(edges[4] <= 320);
            for group in 0..4 {
                assert_eq!(edges[group + 1] - edges[group], dimensions[group]);
                let valid = positions[edges[group]..edges[group + 1]]
                    .iter()
                    .take_while(|&&p| p != usize::MAX)
                    .count();
                assert_eq!(valid % 2, 0);
                assert!(
                    positions[edges[group] + valid..edges[group + 1]]
                        .iter()
                        .all(|&p| p == usize::MAX)
                );
            }
            let mut found = [false; 214];
            for &position in &positions[..edges[4]] {
                if position == usize::MAX {
                    continue;
                }
                assert!(position < 2 * (8000 / f0));
                assert!(!found[position]);
                found[position] = true;
            }
            assert!(found[..2 * (8000 / f0)].iter().all(|&f| f));
        }
    }
}

#[cfg(test)]
mod synthesis_lane_tests {
    use super::*;
    fn scalar_reference_synthesize(
        input: &mut [f32],
        samples: usize,
        f0: usize,
        positions: &[usize; 960],
        values: &[f32; 960],
    ) {
        input.fill(0.0);
        let mut amplitudes = [0.0f32; 214];
        for (&value, &position) in values.iter().zip(positions) {
            if position != usize::MAX {
                amplitudes[position] = value;
            }
        }
        let start = (samples - 120) / 2;
        for harmonic in 1..=8000 / f0 {
            let cosine = f64::from(amplitudes[2 * (harmonic - 1)]);
            let sine = f64::from(amplitudes[2 * (harmonic - 1) + 1]);
            if cosine == 0.0 && sine == 0.0 {
                continue;
            }
            let angle = std::f64::consts::TAU * (harmonic * f0) as f64 / 48000.0;
            let (rotation_s, rotation_c) = angle.sin_cos();
            let (mut sin, mut cos) = (0.0, 1.0);
            for sample in &mut input[start..start + samples + 120] {
                *sample += (cosine * cos + sine * sin) as f32;
                (sin, cos) = (
                    sin * rotation_c + cos * rotation_s,
                    cos * rotation_c - sin * rotation_s,
                );
            }
        }
    }
    #[test]
    fn lanes_preserve_scalar_float_bits_at_every_pitch() {
        for samples in [480, 960] {
            for f0 in MIN_F0..=MAX_F0 {
                let mut positions = [usize::MAX; 960];
                let (_, edges) = layout(f0, &mut positions);
                for kind in 0..5 {
                    let mut values = [0.0f32; 960];
                    for (index, value) in values[..edges[4]].iter_mut().enumerate() {
                        *value = match kind {
                            0 => 0.0,
                            1 => {
                                if index % 2 == 0 {
                                    -0.0
                                } else {
                                    0.0
                                }
                            }
                            2 => {
                                if index % 7 == 0 {
                                    f32::from_bits(1 + index as u32)
                                } else {
                                    0.0
                                }
                            }
                            3 => {
                                if index % 5 == 0 {
                                    (index as f32 * 0.9).sin() * 2.0
                                } else {
                                    0.0
                                }
                            }
                            _ => (index as f32 * 0.37).cos() * 0.2,
                        };
                    }
                    let mut expected = [3.0f32; 1920];
                    let mut actual = expected;
                    scalar_reference_synthesize(&mut expected, samples, f0, &positions, &values);
                    synthesize(&mut actual, samples, f0, &positions, &values);
                    assert!(
                        actual
                            .iter()
                            .zip(&expected)
                            .all(|(a, b)| a.to_bits() == b.to_bits()),
                        "samples={samples} F0={f0} kind={kind}"
                    );
                }
            }
        }
    }
}
