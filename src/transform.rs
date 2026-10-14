// SPDX-License-Identifier: MIT
//! Planned mixed-radix FFT and low-overlap MDCT kernels.
//!
//! Plans hold immutable coefficients. Callers own input, output and MDCT scratch
//! buffers, allowing plans to be shared between streams without locking. All
//! trigonometry and allocation happens during construction.

use std::fmt;

/// Invalid transform dimensions or sample values.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// The transform size is unsupported or buffers have incorrect lengths.
    Length,
    /// Input contains non-finite values or exceeds the transform's numeric headroom.
    Sample,
}
impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Length => "invalid transform length",
            Self::Sample => "invalid transform input sample",
        })
    }
}
impl std::error::Error for Error {}

/// A complex, single-precision sample.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Complex {
    /// Real component.
    pub real: f32,
    /// Imaginary component.
    pub imaginary: f32,
}
impl Complex {
    /// The complex value zero.
    pub const ZERO: Self = Self {
        real: 0.0,
        imaginary: 0.0,
    };
    fn add(self, other: Self) -> Self {
        Self {
            real: self.real + other.real,
            imaginary: self.imaginary + other.imaginary,
        }
    }
    fn sub(self, other: Self) -> Self {
        Self {
            real: self.real - other.real,
            imaginary: self.imaginary - other.imaginary,
        }
    }
    fn scale(self, gain: f32) -> Self {
        Self {
            real: self.real * gain,
            imaginary: self.imaginary * gain,
        }
    }
    fn multiply(self, other: Self) -> Self {
        Self {
            real: self.real * other.real - self.imaginary * other.imaginary,
            imaginary: self.real * other.imaginary + self.imaginary * other.real,
        }
    }
    fn rotate(self, inverse: bool) -> Self {
        if inverse {
            Self {
                real: -self.imaginary,
                imaginary: self.real,
            }
        } else {
            Self {
                real: self.imaginary,
                imaginary: -self.real,
            }
        }
    }
    fn phase(angle: f64) -> Self {
        Self {
            real: angle.cos() as f32,
            imaginary: angle.sin() as f32,
        }
    }
}

#[derive(Debug)]
struct Stage {
    size: usize,
    radix: usize,
    // One contiguous row per butterfly. Unused legs contain zero; the first
    // row's unity factors are skipped by the separate leading butterfly.
    twiddles: Vec<[Complex; 4]>,
}

/// An immutable FFT plan for lengths factorizable into 2, 3 and 5, up to 1,920.
#[derive(Debug)]
pub struct FftPlan {
    length: usize,
    stages: Vec<Stage>,
}
impl FftPlan {
    /// Precompute a transform's coefficients and factorization.
    pub fn new(length: usize) -> Result<Self, Error> {
        if !(1..=1_920).contains(&length) {
            return Err(Error::Length);
        }
        let mut size = length;
        let mut stages = Vec::new();
        while size > 1 {
            let radix = [4, 2, 3, 5]
                .into_iter()
                .find(|radix| size.is_multiple_of(*radix))
                .ok_or(Error::Length)?;
            stages.push(Stage {
                size,
                radix,
                twiddles: Vec::new(),
            });
            size /= radix;
        }
        let roots: Vec<_> = (0..length)
            .map(|index| Complex::phase(-std::f64::consts::TAU * index as f64 / length as f64))
            .collect();
        for stage in &mut stages {
            let step = length / stage.size;
            stage.twiddles = (0..stage.size / stage.radix)
                .map(|index| {
                    std::array::from_fn(|leg| {
                        if leg + 1 < stage.radix {
                            roots[(leg + 1) * index * step]
                        } else {
                            Complex::ZERO
                        }
                    })
                })
                .collect();
        }
        Ok(Self { length, stages })
    }

    /// Number of complex samples transformed by this plan.
    pub fn len(&self) -> usize {
        self.length
    }

    /// FFT plans always contain at least one sample.
    pub fn is_empty(&self) -> bool {
        false
    }

    fn twiddle<const INVERSE: bool>(sample: Complex, root: Option<Complex>) -> Complex {
        match root {
            None => sample,
            Some(mut root) => {
                if INVERSE {
                    root.imaginary = -root.imaginary;
                }
                sample.multiply(root)
            }
        }
    }

    #[inline]
    fn butterfly_five<const INVERSE: bool>(samples: [Complex; 5]) -> [Complex; 5] {
        let [a, b, c, d, e] = samples;
        let outer = b.add(e);
        let inner = c.add(d);
        let outer_difference = b.sub(e);
        let inner_difference = c.sub(d);
        let first_value = a.add(outer.scale(0.309_017)).add(inner.scale(-0.809_017));
        let second_value = a.add(outer.scale(-0.809_017)).add(inner.scale(0.309_017));
        let first_rotation = outer_difference
            .scale(0.951_056_54)
            .add(inner_difference.scale(0.587_785_24))
            .rotate(INVERSE);
        let second_rotation = outer_difference
            .scale(0.587_785_24)
            .sub(inner_difference.scale(0.951_056_54))
            .rotate(INVERSE);
        [
            a.add(outer).add(inner),
            first_value.add(first_rotation),
            second_value.add(second_rotation),
            second_value.sub(second_rotation),
            first_value.sub(first_rotation),
        ]
    }

    fn split<const INVERSE: bool>(
        &self,
        input: &[Complex],
        stride: usize,
        output: &mut [Complex],
        depth: usize,
    ) {
        if output.len() == 1 {
            output[0] = input[0];
            return;
        }
        let stage = &self.stages[depth];
        let width = stage.size / stage.radix;
        // CELT's factorizations end with a five-point unity butterfly. Its
        // samples can go directly to registers without the general stage's
        // split slices, twiddle rows and empty trailing iterator.
        if width == 1 && stage.radix == 5 {
            let samples = std::array::from_fn(|index| input[index * stride]);
            *output.first_chunk_mut::<5>().expect("FFT leaf width") =
                Self::butterfly_five::<INVERSE>(samples);
            return;
        }
        if width == 1 {
            for (index, sample) in output.iter_mut().enumerate() {
                *sample = input[index * stride];
            }
        } else {
            for (index, group) in output.chunks_exact_mut(width).enumerate() {
                self.split::<INVERSE>(
                    &input[index * stride..],
                    stride * stage.radix,
                    group,
                    depth + 1,
                );
            }
        }
        match stage.radix {
            2 => {
                let (first, second) = output.split_at_mut(width);
                let apply = |first: &mut Complex,
                             second: &mut Complex,
                             rotations: Option<&[Complex; 4]>| {
                    let a = *first;
                    let b = Self::twiddle::<INVERSE>(*second, rotations.map(|row| row[0]));
                    *first = a.add(b);
                    *second = a.sub(b);
                };
                let (first_start, first) = first.split_first_mut().expect("FFT stage width");
                let (second_start, second) = second.split_first_mut().expect("FFT stage width");
                apply(first_start, second_start, None);
                for ((first, second), rotations) in
                    first.iter_mut().zip(second).zip(&stage.twiddles[1..])
                {
                    apply(first, second, Some(rotations));
                }
            }
            3 => {
                let (first, rest) = output.split_at_mut(width);
                let (second, third) = rest.split_at_mut(width);
                let apply = |first: &mut Complex,
                             second: &mut Complex,
                             third: &mut Complex,
                             rotations: Option<&[Complex; 4]>| {
                    let a = *first;
                    let b = Self::twiddle::<INVERSE>(*second, rotations.map(|row| row[0]));
                    let c = Self::twiddle::<INVERSE>(*third, rotations.map(|row| row[1]));
                    let sum = b.add(c);
                    let center = a.sub(sum.scale(0.5));
                    let rotation = b.sub(c).scale(0.866_025_4).rotate(INVERSE);
                    *first = a.add(sum);
                    *second = center.add(rotation);
                    *third = center.sub(rotation);
                };
                let (first_start, first) = first.split_first_mut().expect("FFT stage width");
                let (second_start, second) = second.split_first_mut().expect("FFT stage width");
                let (third_start, third) = third.split_first_mut().expect("FFT stage width");
                apply(first_start, second_start, third_start, None);
                for (((first, second), third), rotations) in first
                    .iter_mut()
                    .zip(second)
                    .zip(third)
                    .zip(&stage.twiddles[1..])
                {
                    apply(first, second, third, Some(rotations));
                }
            }
            4 => {
                let (first, rest) = output.split_at_mut(width);
                let (second, rest) = rest.split_at_mut(width);
                let (third, fourth) = rest.split_at_mut(width);
                let apply = |first: &mut Complex,
                             second: &mut Complex,
                             third: &mut Complex,
                             fourth: &mut Complex,
                             rotations: Option<&[Complex; 4]>| {
                    let a = *first;
                    let b = Self::twiddle::<INVERSE>(*second, rotations.map(|row| row[0]));
                    let c = Self::twiddle::<INVERSE>(*third, rotations.map(|row| row[1]));
                    let d = Self::twiddle::<INVERSE>(*fourth, rotations.map(|row| row[2]));
                    let even = a.add(c);
                    let difference = a.sub(c);
                    let odd = b.add(d);
                    let rotation = b.sub(d).rotate(INVERSE);
                    *first = even.add(odd);
                    *second = difference.add(rotation);
                    *third = even.sub(odd);
                    *fourth = difference.sub(rotation);
                };
                let (first_start, first) = first.split_first_mut().expect("FFT stage width");
                let (second_start, second) = second.split_first_mut().expect("FFT stage width");
                let (third_start, third) = third.split_first_mut().expect("FFT stage width");
                let (fourth_start, fourth) = fourth.split_first_mut().expect("FFT stage width");
                apply(first_start, second_start, third_start, fourth_start, None);
                for ((((first, second), third), fourth), rotations) in first
                    .iter_mut()
                    .zip(second)
                    .zip(third)
                    .zip(fourth)
                    .zip(&stage.twiddles[1..])
                {
                    apply(first, second, third, fourth, Some(rotations));
                }
            }
            5 => {
                let (first, rest) = output.split_at_mut(width);
                let (second, rest) = rest.split_at_mut(width);
                let (third, rest) = rest.split_at_mut(width);
                let (fourth, fifth) = rest.split_at_mut(width);
                let apply = |first: &mut Complex,
                             second: &mut Complex,
                             third: &mut Complex,
                             fourth: &mut Complex,
                             fifth: &mut Complex,
                             rotations: Option<&[Complex; 4]>| {
                    let a = *first;
                    let b = Self::twiddle::<INVERSE>(*second, rotations.map(|row| row[0]));
                    let c = Self::twiddle::<INVERSE>(*third, rotations.map(|row| row[1]));
                    let d = Self::twiddle::<INVERSE>(*fourth, rotations.map(|row| row[2]));
                    let e = Self::twiddle::<INVERSE>(*fifth, rotations.map(|row| row[3]));
                    [*first, *second, *third, *fourth, *fifth] =
                        Self::butterfly_five::<INVERSE>([a, b, c, d, e]);
                };
                let (first_start, first) = first.split_first_mut().expect("FFT stage width");
                let (second_start, second) = second.split_first_mut().expect("FFT stage width");
                let (third_start, third) = third.split_first_mut().expect("FFT stage width");
                let (fourth_start, fourth) = fourth.split_first_mut().expect("FFT stage width");
                let (fifth_start, fifth) = fifth.split_first_mut().expect("FFT stage width");
                apply(
                    first_start,
                    second_start,
                    third_start,
                    fourth_start,
                    fifth_start,
                    None,
                );
                for (((((first, second), third), fourth), fifth), rotations) in first
                    .iter_mut()
                    .zip(second)
                    .zip(third)
                    .zip(fourth)
                    .zip(fifth)
                    .zip(&stage.twiddles[1..])
                {
                    apply(first, second, third, fourth, fifth, Some(rotations));
                }
            }
            _ => unreachable!(),
        }
    }

    fn run(&self, input: &[Complex], output: &mut [Complex], inverse: bool) {
        if inverse {
            self.split::<true>(input, 1, output, 0);
            for sample in output {
                *sample = sample.scale(1.0 / self.length as f32);
            }
        } else {
            self.split::<false>(input, 1, output, 0);
        }
    }

    fn validate(&self, input: &[Complex], output: &[Complex]) -> Result<(), Error> {
        if input.len() != self.length || output.len() != self.length {
            return Err(Error::Length);
        }
        let limit = f32::MAX / (4 * self.length) as f32;
        if input.iter().any(|sample| {
            !sample.real.is_finite()
                || !sample.imaginary.is_finite()
                || sample.real.abs() > limit
                || sample.imaginary.abs() > limit
        }) {
            return Err(Error::Sample);
        }
        Ok(())
    }

    /// Compute an unscaled forward FFT without allocation.
    ///
    /// Input and output must each contain exactly `len()` samples. Invalid
    /// dimensions or numeric input leave output untouched.
    pub fn forward(&self, input: &[Complex], output: &mut [Complex]) -> Result<(), Error> {
        self.validate(input, output)?;
        self.run(input, output, false);
        Ok(())
    }

    /// Compute an inverse FFT, including `1 / len()` normalization.
    pub fn inverse(&self, input: &[Complex], output: &mut [Complex]) -> Result<(), Error> {
        self.validate(input, output)?;
        self.run(input, output, true);
        Ok(())
    }
}

/// A per-stream MDCT workspace, allocated before processing audio.
#[derive(Debug)]
pub struct MdctWorkspace {
    input: Vec<Complex>,
    output: Vec<Complex>,
}
impl MdctWorkspace {
    /// Allocate scratch for this plan's half-length complex FFT.
    pub fn new(plan: &MdctPlan) -> Self {
        let length = plan.coefficients / 2;
        Self {
            input: vec![Complex::ZERO; length],
            output: vec![Complex::ZERO; length],
        }
    }
}

#[derive(Debug)]
struct MdctRow {
    window: [f32; 4],
    pre: Complex,
    post: Complex,
}

#[derive(Clone, Copy)]
pub(crate) struct WindowView<'a> {
    rows: &'a [MdctRow],
}

impl<'a> WindowView<'a> {
    pub(crate) fn len(self) -> usize {
        4 * self.rows.len()
    }

    fn slot(self, index: usize) -> (usize, usize) {
        assert!(index < self.len());
        let q = self.rows.len();
        let (folded, edge) = if index < q {
            (q + index, true)
        } else if index < 3 * q {
            (3 * q - 1 - index, false)
        } else {
            (index - 3 * q, true)
        };
        if folded.is_multiple_of(2) {
            (folded / 2, usize::from(edge))
        } else {
            ((2 * q - 1 - folded) / 2, 2 + usize::from(edge))
        }
    }

    #[cfg(test)]
    pub(crate) fn iter(self) -> impl Iterator<Item = &'a f32> {
        (0..self.len()).map(move |index| {
            let (row, field) = self.slot(index);
            &self.rows[row].window[field]
        })
    }
}

impl std::ops::Index<usize> for WindowView<'_> {
    type Output = f32;

    fn index(&self, index: usize) -> &Self::Output {
        let (row, field) = self.slot(index);
        &self.rows[row].window[field]
    }
}

/// An immutable low-overlap MDCT plan using one half-length complex FFT.
///
/// Forward coefficients use `2 / N` scaling; the inverse is unscaled. Windowed
/// inverse blocks overlap by `N` samples and are added to reconstruct audio.
#[derive(Debug)]
pub struct MdctPlan {
    coefficients: usize,
    rows: Vec<MdctRow>,
    fft: FftPlan,
}
impl MdctPlan {
    /// Precompute a symmetric, power-complementary low-overlap window and phases.
    ///
    /// `coefficients` is even, at most 3,840, and half its size must factor into
    /// 2, 3 and 5. `overlap` is positive, no larger than the coefficient count,
    /// and has the same parity. CELT frame sizes use 120, 240, 480 or 960
    /// coefficients with 120 samples of overlap at 48 kHz.
    pub fn new(coefficients: usize, overlap: usize) -> Result<Self, Error> {
        if coefficients == 0
            || coefficients > 3_840
            || !coefficients.is_multiple_of(2)
            || overlap == 0
            || overlap > coefficients
            || !(coefficients - overlap).is_multiple_of(2)
        {
            return Err(Error::Length);
        }
        let fft = FftPlan::new(coefficients / 2)?;
        let padding = (coefficients - overlap) / 2;
        let mut window = vec![0.0; 2 * coefficients];
        window[padding + overlap..coefficients + padding].fill(1.0);
        for index in 0..overlap {
            let angle = std::f64::consts::FRAC_PI_2 * (index as f64 + 0.5) / overlap as f64;
            let value = (std::f64::consts::FRAC_PI_2 * angle.sin().powi(2)).sin() as f32;
            window[padding + index] = value;
            window[2 * coefficients - 1 - padding - index] = value;
        }
        let q = coefficients / 2;
        let mut rows = Vec::with_capacity(q);
        for index in 0..q {
            let real = 2 * index;
            let imaginary = 2 * q - 1 - real;
            let edge = |folded| {
                if folded < q {
                    3 * q + folded
                } else {
                    folded - q
                }
            };
            rows.push(MdctRow {
                window: [
                    window[3 * q - 1 - real],
                    window[edge(real)],
                    window[3 * q - 1 - imaginary],
                    window[edge(imaginary)],
                ],
                pre: Complex::ZERO,
                post: Complex::ZERO,
            });
        }
        for (index, row) in rows.iter_mut().enumerate() {
            row.pre = Complex::phase(-std::f64::consts::PI * index as f64 / coefficients as f64);
        }
        for (index, row) in rows.iter_mut().enumerate() {
            row.post =
                Complex::phase(-std::f64::consts::PI * (index as f64 + 0.25) / coefficients as f64);
        }
        Ok(Self {
            coefficients,
            rows,
            fft,
        })
    }

    /// Number of frequency coefficients in this transform.
    pub fn coefficients(&self) -> usize {
        self.coefficients
    }

    pub(crate) fn window_view(&self) -> WindowView<'_> {
        WindowView { rows: &self.rows }
    }

    #[cfg(test)]
    pub(crate) fn set_window_sample(&mut self, index: usize, value: f32) {
        let (row, field) = self.window_view().slot(index);
        self.rows[row].window[field] = value;
    }

    fn validate(
        &self,
        input: &[f32],
        output: &[f32],
        workspace: &MdctWorkspace,
        inverse: bool,
    ) -> Result<(), Error> {
        self.validate_dimensions(input, output, workspace, inverse)?;
        let n = self.coefficients;
        let limit = f32::MAX / (16 * n) as f32;
        if input
            .iter()
            .any(|sample| !sample.is_finite() || sample.abs() > limit)
        {
            return Err(Error::Sample);
        }
        Ok(())
    }

    fn validate_dimensions(
        &self,
        input: &[f32],
        output: &[f32],
        workspace: &MdctWorkspace,
        inverse: bool,
    ) -> Result<(), Error> {
        let n = self.coefficients;
        let (input_length, output_length) = if inverse { (n, 2 * n) } else { (2 * n, n) };
        if input.len() != input_length
            || output.len() != output_length
            || workspace.input.len() != n / 2
            || workspace.output.len() != n / 2
        {
            return Err(Error::Length);
        }
        Ok(())
    }

    /// Transform `2 * N` time samples to `N` coefficients, applying the plan's window.
    ///
    /// Invalid buffers, workspace sizes or numeric input leave output untouched.
    pub fn forward(
        &self,
        input: &[f32],
        output: &mut [f32],
        workspace: &mut MdctWorkspace,
    ) -> Result<(), Error> {
        self.validate(input, output, workspace, false)?;
        let n = self.coefficients;
        let q = n / 2;
        let boundary = q.div_ceil(2);
        for (index, (sample, row)) in workspace.input.iter_mut().zip(&self.rows).enumerate() {
            let real = 2 * index;
            let imaginary = n - 1 - real;
            let mirrored_real = 3 * q - 1 - real;
            let mirrored_imaginary = 3 * q - 1 - imaginary;
            let (real, imaginary) = if index < boundary {
                (
                    -input[mirrored_real] * row.window[0] - input[3 * q + real] * row.window[1],
                    input[imaginary - q] * row.window[3]
                        - input[mirrored_imaginary] * row.window[2],
                )
            } else {
                (
                    input[real - q] * row.window[1] - input[mirrored_real] * row.window[0],
                    -input[mirrored_imaginary] * row.window[2]
                        - input[3 * q + imaginary] * row.window[3],
                )
            };
            *sample = Complex { real, imaginary }.multiply(row.pre);
        }
        self.fft.run(&workspace.input, &mut workspace.output, false);
        for (index, (&sample, row)) in workspace.output.iter().zip(&self.rows).enumerate() {
            let value = sample.multiply(row.post).scale(2.0 / n as f32);
            output[2 * index] = value.real;
            output[n - 1 - 2 * index] = -value.imaginary;
        }
        Ok(())
    }

    /// Transform `N` coefficients to a windowed `2 * N` inverse block.
    ///
    /// Add adjacent inverse blocks with an `N`-sample stride to cancel time aliasing.
    pub fn inverse(
        &self,
        input: &[f32],
        output: &mut [f32],
        workspace: &mut MdctWorkspace,
    ) -> Result<(), Error> {
        self.validate(input, output, workspace, true)?;
        self.inverse_inner(input, output, workspace);
        Ok(())
    }

    /// Internal path after the caller has checked all coefficients against
    /// `f32::MAX / (16 * coefficients)`. Buffer/workspace checks remain here.
    #[cfg(feature = "opus")]
    pub(crate) fn inverse_finite(
        &self,
        input: &[f32],
        output: &mut [f32],
        workspace: &mut MdctWorkspace,
    ) -> Result<(), Error> {
        self.validate_dimensions(input, output, workspace, true)?;
        debug_assert!(
            input
                .iter()
                .all(|sample| { sample.abs() <= f32::MAX / (16 * self.coefficients) as f32 })
        );
        self.inverse_inner(input, output, workspace);
        Ok(())
    }

    fn inverse_inner(&self, input: &[f32], output: &mut [f32], workspace: &mut MdctWorkspace) {
        let n = self.coefficients;
        for (index, (sample, row)) in workspace.input.iter_mut().zip(&self.rows).enumerate() {
            *sample = Complex {
                real: input[2 * index],
                imaginary: input[n - 1 - 2 * index],
            }
            .multiply(row.pre);
        }
        self.fft.run(&workspace.input, &mut workspace.output, false);
        let q = n / 2;
        let boundary = q.div_ceil(2);
        if q.is_multiple_of(2) {
            let (low, rest) = output.split_at_mut(q);
            let (lower_middle, rest) = rest.split_at_mut(q);
            let (upper_middle, high) = rest.split_at_mut(q);
            let (low, _) = low.as_chunks_mut::<2>();
            let (lower_middle, _) = lower_middle.as_chunks_mut::<2>();
            let (upper_middle, _) = upper_middle.as_chunks_mut::<2>();
            let (high, _) = high.as_chunks_mut::<2>();
            for (((((&sample, row), low), lower), upper), high) in workspace.output[..boundary]
                .iter()
                .zip(&self.rows[..boundary])
                .zip(low.iter_mut().rev())
                .zip(lower_middle.iter_mut())
                .zip(upper_middle.iter_mut().rev())
                .zip(high.iter_mut())
            {
                let value = sample.multiply(row.post);
                let imaginary_value = -value.imaginary;
                upper[1] = -value.real * row.window[0];
                high[0] = -value.real * row.window[1];
                lower[0] = -imaginary_value * row.window[2];
                low[1] = imaginary_value * row.window[3];
            }
            for (((((&sample, row), low), lower), upper), high) in workspace.output[boundary..]
                .iter()
                .zip(&self.rows[boundary..])
                .zip(low.iter_mut())
                .zip(lower_middle.iter_mut().rev())
                .zip(upper_middle.iter_mut())
                .zip(high.iter_mut().rev())
            {
                let value = sample.multiply(row.post);
                let imaginary_value = -value.imaginary;
                lower[1] = -value.real * row.window[0];
                low[0] = value.real * row.window[1];
                upper[0] = -imaginary_value * row.window[2];
                high[1] = -imaginary_value * row.window[3];
            }
            return;
        }
        for (index, (&sample, row)) in workspace.output.iter().zip(&self.rows).enumerate() {
            let value = sample.multiply(row.post);
            let real = 2 * index;
            let imaginary = n - 1 - real;
            let mirrored_real = 3 * q - 1 - real;
            let mirrored_imaginary = 3 * q - 1 - imaginary;
            let imaginary_value = -value.imaginary;
            output[mirrored_real] = -value.real * row.window[0];
            if index < boundary {
                output[3 * q + real] = -value.real * row.window[1];
                output[mirrored_imaginary] = -imaginary_value * row.window[2];
                output[imaginary - q] = imaginary_value * row.window[3];
            } else {
                output[real - q] = value.real * row.window[1];
                output[mirrored_imaginary] = -imaginary_value * row.window[2];
                output[3 * q + imaginary] = -imaginary_value * row.window[3];
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_windows_preserve_literal_bits_and_allocated_storage() {
        assert_eq!(std::mem::size_of::<MdctRow>(), 32);
        assert_eq!(std::mem::align_of::<MdctRow>(), 4);
        let mut pairs = 0;
        for n in (2..=3840).step_by(2) {
            if FftPlan::new(n / 2).is_err() {
                continue;
            }
            for (case, overlap) in [n, 120].into_iter().enumerate() {
                if overlap > n || (case == 1 && n == 120) {
                    continue;
                }
                let plan = MdctPlan::new(n, overlap).unwrap();
                let padding = (n - overlap) / 2;
                let mut expected = vec![0.0f32; 2 * n];
                expected[padding + overlap..n + padding].fill(1.0);
                for t in 0..overlap {
                    let angle = std::f64::consts::FRAC_PI_2 * (t as f64 + 0.5) / overlap as f64;
                    let value = (std::f64::consts::FRAC_PI_2 * angle.sin().powi(2)).sin() as f32;
                    expected[padding + t] = value;
                    expected[2 * n - 1 - padding - t] = value;
                }
                let old_pre: Vec<Complex> = (0..n / 2).map(|_| Complex::ZERO).collect();
                let old_post: Vec<Complex> = (0..n / 2).map(|_| Complex::ZERO).collect();
                let old_bytes = expected.capacity() * std::mem::size_of::<f32>()
                    + (old_pre.capacity() + old_post.capacity()) * std::mem::size_of::<Complex>();
                assert!(plan.rows.capacity() * std::mem::size_of::<MdctRow>() <= old_bytes);
                let window = plan.window_view();
                assert_eq!(window.len(), expected.len());
                for (k, &value) in expected.iter().enumerate() {
                    assert_eq!(
                        window[k].to_bits(),
                        value.to_bits(),
                        "N={n}, overlap={overlap}, k={k}"
                    );
                }
                assert_eq!(
                    window.iter().map(|x| x.to_bits()).collect::<Vec<_>>(),
                    expected.iter().map(|x| x.to_bits()).collect::<Vec<_>>()
                );
                pairs += 1;
            }
        }
        assert_eq!(pairs, 186);
    }
}
