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
    step: usize,
}

/// An immutable FFT plan for lengths factorizable into 2, 3 and 5, up to 1,920.
#[derive(Debug)]
pub struct FftPlan {
    length: usize,
    roots: Vec<Complex>,
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
            let radix = [2, 3, 5]
                .into_iter()
                .find(|radix| size.is_multiple_of(*radix))
                .ok_or(Error::Length)?;
            stages.push(Stage {
                size,
                radix,
                step: length / size,
            });
            size /= radix;
        }
        let roots = (0..length)
            .map(|index| Complex::phase(-std::f64::consts::TAU * index as f64 / length as f64))
            .collect();
        Ok(Self {
            length,
            roots,
            stages,
        })
    }

    /// Number of complex samples transformed by this plan.
    pub fn len(&self) -> usize {
        self.length
    }

    /// FFT plans always contain at least one sample.
    pub fn is_empty(&self) -> bool {
        false
    }

    fn root(&self, index: usize, inverse: bool) -> Complex {
        let mut root = self.roots[index];
        if inverse {
            root.imaginary = -root.imaginary;
        }
        root
    }

    fn twiddle(&self, sample: Complex, index: usize, inverse: bool) -> Complex {
        if index == 0 {
            sample
        } else {
            sample.multiply(self.root(index, inverse))
        }
    }

    fn split(
        &self,
        input: &[Complex],
        stride: usize,
        output: &mut [Complex],
        depth: usize,
        inverse: bool,
    ) {
        if output.len() == 1 {
            output[0] = input[0];
            return;
        }
        let stage = &self.stages[depth];
        let width = stage.size / stage.radix;
        if width == 1 {
            for (index, sample) in output.iter_mut().enumerate() {
                *sample = input[index * stride];
            }
        } else {
            for (index, group) in output.chunks_exact_mut(width).enumerate() {
                self.split(
                    &input[index * stride..],
                    stride * stage.radix,
                    group,
                    depth + 1,
                    inverse,
                );
            }
        }
        for index in 0..width {
            let a = output[index];
            let b = self.twiddle(output[width + index], index * stage.step, inverse);
            match stage.radix {
                2 => {
                    output[index] = a.add(b);
                    output[width + index] = a.sub(b);
                }
                3 => {
                    let c =
                        self.twiddle(output[2 * width + index], 2 * index * stage.step, inverse);
                    let sum = b.add(c);
                    let center = a.sub(sum.scale(0.5));
                    let rotation = b.sub(c).scale(0.866_025_4).rotate(inverse);
                    output[index] = a.add(sum);
                    output[width + index] = center.add(rotation);
                    output[2 * width + index] = center.sub(rotation);
                }
                5 => {
                    let c =
                        self.twiddle(output[2 * width + index], 2 * index * stage.step, inverse);
                    let d =
                        self.twiddle(output[3 * width + index], 3 * index * stage.step, inverse);
                    let e =
                        self.twiddle(output[4 * width + index], 4 * index * stage.step, inverse);
                    let outer = b.add(e);
                    let inner = c.add(d);
                    let outer_difference = b.sub(e);
                    let inner_difference = c.sub(d);
                    let first = a.add(outer.scale(0.309_017)).add(inner.scale(-0.809_017));
                    let second = a.add(outer.scale(-0.809_017)).add(inner.scale(0.309_017));
                    let first_rotation = outer_difference
                        .scale(0.951_056_54)
                        .add(inner_difference.scale(0.587_785_24))
                        .rotate(inverse);
                    let second_rotation = outer_difference
                        .scale(0.587_785_24)
                        .sub(inner_difference.scale(0.951_056_54))
                        .rotate(inverse);
                    output[index] = a.add(outer).add(inner);
                    output[width + index] = first.add(first_rotation);
                    output[2 * width + index] = second.add(second_rotation);
                    output[3 * width + index] = second.sub(second_rotation);
                    output[4 * width + index] = first.sub(first_rotation);
                }
                _ => unreachable!(),
            }
        }
    }

    fn run(&self, input: &[Complex], output: &mut [Complex], inverse: bool) {
        self.split(input, 1, output, 0, inverse);
        if inverse {
            for sample in output {
                *sample = sample.scale(1.0 / self.length as f32);
            }
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

/// An immutable low-overlap MDCT plan using one half-length complex FFT.
///
/// Forward coefficients use `2 / N` scaling; the inverse is unscaled. Windowed
/// inverse blocks overlap by `N` samples and are added to reconstruct audio.
#[derive(Debug)]
pub struct MdctPlan {
    coefficients: usize,
    window: Vec<f32>,
    pre: Vec<Complex>,
    post: Vec<Complex>,
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
        let pre = (0..coefficients / 2)
            .map(|index| Complex::phase(-std::f64::consts::PI * index as f64 / coefficients as f64))
            .collect();
        let post = (0..coefficients / 2)
            .map(|index| {
                Complex::phase(-std::f64::consts::PI * (index as f64 + 0.25) / coefficients as f64)
            })
            .collect();
        Ok(Self {
            coefficients,
            window,
            pre,
            post,
            fft,
        })
    }

    /// Number of frequency coefficients in this transform.
    pub fn coefficients(&self) -> usize {
        self.coefficients
    }

    pub(crate) fn window(&self) -> &[f32] {
        &self.window
    }

    fn validate(
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
        let limit = f32::MAX / (16 * n) as f32;
        if input
            .iter()
            .any(|sample| !sample.is_finite() || sample.abs() > limit)
        {
            return Err(Error::Sample);
        }
        Ok(())
    }

    fn fold(&self, input: &[f32], index: usize) -> f32 {
        let n = self.coefficients;
        let mirrored = 3 * n / 2 - 1 - index;
        if index < n / 2 {
            let right = 3 * n / 2 + index;
            -input[mirrored] * self.window[mirrored] - input[right] * self.window[right]
        } else {
            let left = index - n / 2;
            input[left] * self.window[left] - input[mirrored] * self.window[mirrored]
        }
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
        for (index, sample) in workspace.input.iter_mut().enumerate() {
            *sample = Complex {
                real: self.fold(input, 2 * index),
                imaginary: self.fold(input, n - 1 - 2 * index),
            }
            .multiply(self.pre[index]);
        }
        self.fft.run(&workspace.input, &mut workspace.output, false);
        for (index, &sample) in workspace.output.iter().enumerate() {
            let value = sample.multiply(self.post[index]).scale(2.0 / n as f32);
            output[2 * index] = value.real;
            output[n - 1 - 2 * index] = -value.imaginary;
        }
        Ok(())
    }

    fn unfold(&self, output: &mut [f32], index: usize, sample: f32) {
        let n = self.coefficients;
        let mirrored = 3 * n / 2 - 1 - index;
        output[mirrored] = -sample * self.window[mirrored];
        if index < n / 2 {
            let right = 3 * n / 2 + index;
            output[right] = -sample * self.window[right];
        } else {
            let left = index - n / 2;
            output[left] = sample * self.window[left];
        }
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
        let n = self.coefficients;
        for (index, sample) in workspace.input.iter_mut().enumerate() {
            *sample = Complex {
                real: input[2 * index],
                imaginary: input[n - 1 - 2 * index],
            }
            .multiply(self.pre[index]);
        }
        self.fft.run(&workspace.input, &mut workspace.output, false);
        for (index, &sample) in workspace.output.iter().enumerate() {
            let value = sample.multiply(self.post[index]);
            self.unfold(output, 2 * index, value.real);
            self.unfold(output, n - 1 - 2 * index, -value.imaginary);
        }
        Ok(())
    }
}
