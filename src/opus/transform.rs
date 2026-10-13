// SPDX-License-Identifier: MIT
//! CELT mono transform and filtering state from RFC 6716 sections 4.3.7 and 5.3.
//!
//! Each channel owns its stream state. All four plans and their scratch buffers
//! are prepared at construction; frame-size and transient changes allocate
//! nothing. Spectrum samples are frequency-major, with short transforms
//! interleaved in time. PCM is normalized floating point, without the reference
//! codec's internal 32768 PCM scale.

use crate::transform::{Error, MdctPlan, MdctWorkspace};

pub(crate) const OVERLAP: usize = 120;
pub(crate) const MAX_FRAME: usize = 960;
// The RFC's decimal value denotes this exact Q15 coefficient.
const PRE_EMPHASIS: f32 = 27_853.0 / 32_768.0;
const HISTORY: usize = 1024;
const SAMPLE_LIMIT: f32 = f32::MAX / (128 * MAX_FRAME) as f32;

fn frame_index(length: usize) -> Result<usize, Error> {
    match length {
        120 => Ok(0),
        240 => Ok(1),
        480 => Ok(2),
        960 => Ok(3),
        _ => Err(Error::Length),
    }
}

fn validate(input: &[f32], output: &[f32]) -> Result<usize, Error> {
    let index = frame_index(input.len())?;
    if output.len() != input.len() {
        return Err(Error::Length);
    }
    if input
        .iter()
        .any(|sample| !sample.is_finite() || sample.abs() > SAMPLE_LIMIT)
    {
        return Err(Error::Sample);
    }
    Ok(index)
}

#[derive(Debug)]
struct Transforms {
    plans: [MdctPlan; 4],
    scratch: [MdctWorkspace; 4],
    block: [f32; 2 * MAX_FRAME],
    short: [f32; OVERLAP],
}

impl Transforms {
    fn new() -> Self {
        let plans = [120, 240, 480, 960]
            .map(|length| MdctPlan::new(length, OVERLAP).expect("CELT transform dimensions"));
        let scratch = std::array::from_fn(|index| MdctWorkspace::new(&plans[index]));
        Self {
            plans,
            scratch,
            block: [0.0; 2 * MAX_FRAME],
            short: [0.0; OVERLAP],
        }
    }
}

/// Pitch post-filter parameters. A zero gain switches filtering off smoothly.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PostFilterParams {
    pub(crate) period: usize,
    pub(crate) gain: f32,
    pub(crate) tapset: usize,
}

impl PostFilterParams {
    pub(crate) const DISABLED: Self = Self {
        period: 15,
        gain: 0.0,
        tapset: 0,
    };

    pub(crate) fn new(period: usize, gain: f32, tapset: usize) -> Result<Self, Error> {
        let parameters = Self {
            period,
            gain,
            tapset,
        };
        parameters.validate()?;
        Ok(parameters)
    }

    fn validate(self) -> Result<(), Error> {
        if !(15..=1022).contains(&self.period) || self.tapset > 2 {
            return Err(Error::Length);
        }
        if !self.gain.is_finite() || !(0.0..=0.75).contains(&self.gain) {
            return Err(Error::Sample);
        }
        Ok(())
    }

    fn taps(self) -> [f32; 3] {
        // Exact rational values behind the decimal coefficients in RFC 6716.
        let taps = match self.tapset {
            0 => [10_048.0 / 32_768.0, 7112.0 / 32_768.0, 4248.0 / 32_768.0],
            1 => [15_200.0 / 32_768.0, 8784.0 / 32_768.0, 0.0],
            2 => [26_208.0 / 32_768.0, 3280.0 / 32_768.0, 0.0],
            _ => unreachable!("validated post-filter tapset"),
        };
        taps.map(|tap| tap * self.gain)
    }
}

/// Five-tap pitch comb with sample-wise interpolation of feedback history.
#[derive(Debug)]
struct CombFilter {
    history: [f32; HISTORY],
    cursor: usize,
    previous: PostFilterParams,
    transition: [f32; OVERLAP],
}

impl CombFilter {
    fn new() -> Self {
        let transition = std::array::from_fn(|index| {
            let phase = std::f64::consts::FRAC_PI_2 * (index as f64 + 0.5) / OVERLAP as f64;
            let window = (std::f64::consts::FRAC_PI_2 * phase.sin().powi(2)).sin();
            (window * window) as f32
        });
        Self {
            history: [0.0; HISTORY],
            cursor: 0,
            previous: PostFilterParams::DISABLED,
            transition,
        }
    }

    fn delayed(&self, period: usize, taps: [f32; 3]) -> f32 {
        let center = self.cursor.wrapping_sub(period) & (HISTORY - 1);
        taps[0] * self.history[center]
            + taps[1]
                * (self.history[center.wrapping_sub(1) & (HISTORY - 1)]
                    + self.history[(center + 1) & (HISTORY - 1)])
            + taps[2]
                * (self.history[center.wrapping_sub(2) & (HISTORY - 1)]
                    + self.history[(center + 2) & (HISTORY - 1)])
    }

    fn process(&mut self, samples: &mut [f32], parameters: PostFilterParams, inverse: bool) {
        if self.previous.gain == 0.0 && parameters.gain == 0.0 {
            // Disabled filters still remember their input for a future enable.
            // A frame is smaller than the ring, so at most two copies suffice.
            let first = samples.len().min(HISTORY - self.cursor);
            self.history[self.cursor..self.cursor + first].copy_from_slice(&samples[..first]);
            self.history[..samples.len() - first].copy_from_slice(&samples[first..]);
            self.cursor = (self.cursor + samples.len()) & (HISTORY - 1);
            self.previous = parameters;
            return;
        }
        let old_taps = self.previous.taps();
        let new_taps = parameters.taps();
        let changed = self.previous != parameters;
        for (index, sample) in samples.iter_mut().enumerate() {
            let input = *sample;
            let mut prediction = self.delayed(parameters.period, new_taps);
            if changed && index < OVERLAP {
                let fade = self.transition[index];
                prediction =
                    (1.0 - fade) * self.delayed(self.previous.period, old_taps) + fade * prediction;
            }
            if inverse {
                *sample -= prediction;
                self.history[self.cursor] = input;
            } else {
                *sample += prediction;
                self.history[self.cursor] = *sample;
            }
            self.cursor = (self.cursor + 1) & (HISTORY - 1);
        }
        self.previous = parameters;
    }

    fn reset(&mut self) {
        self.history.fill(0.0);
        self.cursor = 0;
        self.previous = PostFilterParams::DISABLED;
    }
}

/// One channel of analysis, including pre-emphasis and optional pitch filtering.
#[derive(Debug)]
pub(crate) struct Analysis {
    transform: Transforms,
    previous: [f32; OVERLAP],
    emphasis: f32,
    filter: CombFilter,
    filtered: [f32; MAX_FRAME],
}

impl Analysis {
    pub(crate) fn new() -> Self {
        Self {
            transform: Transforms::new(),
            previous: [0.0; OVERLAP],
            emphasis: 0.0,
            filter: CombFilter::new(),
            filtered: [0.0; MAX_FRAME],
        }
    }

    pub(crate) fn reset(&mut self) {
        self.previous.fill(0.0);
        self.emphasis = 0.0;
        self.filter.reset();
    }

    pub(crate) fn forward(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        transient: bool,
    ) -> Result<(), Error> {
        self.forward_with_filter(input, output, transient, PostFilterParams::DISABLED)
    }

    /// Validate before modifying output or persistent history.
    pub(crate) fn forward_with_filter(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        transient: bool,
        parameters: PostFilterParams,
    ) -> Result<(), Error> {
        let index = validate(input, output)?;
        parameters.validate()?;
        for (&sample, destination) in input.iter().zip(&mut self.filtered) {
            *destination = sample - PRE_EMPHASIS * self.emphasis;
            self.emphasis = sample;
        }
        self.filter
            .process(&mut self.filtered[..input.len()], parameters, true);
        Self::transform(
            &mut self.transform,
            &mut self.previous,
            &self.filtered[..input.len()],
            output,
            transient,
            index,
        )
    }

    /// Transform a stream that the caller has already pre-emphasized/filtered.
    /// Do not mix this path with `forward` in the same stream.
    #[cfg(test)]
    pub(crate) fn forward_preemphasized(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        transient: bool,
    ) -> Result<(), Error> {
        let index = validate(input, output)?;
        Self::transform(
            &mut self.transform,
            &mut self.previous,
            input,
            output,
            transient,
            index,
        )
    }

    fn transform(
        transform: &mut Transforms,
        previous: &mut [f32; OVERLAP],
        input: &[f32],
        output: &mut [f32],
        transient: bool,
        index: usize,
    ) -> Result<(), Error> {
        let n = input.len();
        if transient && n > OVERLAP {
            let count = n / OVERLAP;
            transform.block[..OVERLAP].copy_from_slice(previous);
            for (time, chunk) in input.as_chunks::<OVERLAP>().0.iter().enumerate() {
                transform.block[OVERLAP..2 * OVERLAP].copy_from_slice(chunk);
                transform.plans[0].forward(
                    &transform.block[..2 * OVERLAP],
                    &mut transform.short,
                    &mut transform.scratch[0],
                )?;
                for (frequency, &coefficient) in transform.short.iter().enumerate() {
                    output[frequency * count + time] = coefficient;
                }
                transform.block[..OVERLAP].copy_from_slice(chunk);
            }
        } else {
            let padding = (n - OVERLAP) / 2;
            transform.block[..2 * n].fill(0.0);
            transform.block[padding..padding + OVERLAP].copy_from_slice(previous);
            transform.block[padding + OVERLAP..padding + OVERLAP + n].copy_from_slice(input);
            transform.plans[index].forward(
                &transform.block[..2 * n],
                output,
                &mut transform.scratch[index],
            )?;
        }
        previous.copy_from_slice(&input[n - OVERLAP..]);
        Ok(())
    }
}

/// One synthesis channel; output is delayed by exactly 120 input samples.
#[derive(Debug)]
pub(crate) struct Synthesis {
    transform: Transforms,
    overlap: [f32; OVERLAP],
    emphasis: f32,
    filter: CombFilter,
    pending_filter: PostFilterParams,
}

impl Synthesis {
    pub(crate) fn new() -> Self {
        Self {
            transform: Transforms::new(),
            overlap: [0.0; OVERLAP],
            emphasis: 0.0,
            filter: CombFilter::new(),
            pending_filter: PostFilterParams::DISABLED,
        }
    }

    pub(crate) fn reset(&mut self) {
        self.overlap.fill(0.0);
        self.emphasis = 0.0;
        self.filter.reset();
        self.pending_filter = PostFilterParams::DISABLED;
    }

    pub(crate) fn inverse(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        transient: bool,
        parameters: PostFilterParams,
    ) -> Result<(), Error> {
        let index = validate(input, output)?;
        parameters.validate()?;
        self.transform(input, output, transient, index)?;
        // The first short block belongs to the previous frame's look-ahead.
        // At 2.5 ms the whole frame is that block: incoming parameters become
        // active on the next call. Larger frames then transition to the current
        // parameters in their second short block.
        let (previous, current) = output.split_at_mut(OVERLAP);
        self.filter.process(previous, self.pending_filter, false);
        if !current.is_empty() {
            self.filter.process(current, parameters, false);
        }
        self.pending_filter = parameters;
        for sample in output {
            *sample += PRE_EMPHASIS * self.emphasis;
            self.emphasis = *sample;
        }
        Ok(())
    }

    /// Inverse transform only, leaving pitch filtering and de-emphasis to caller.
    /// Do not mix this path with `inverse` in the same stream.
    #[cfg(test)]
    pub(crate) fn inverse_preemphasized(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        transient: bool,
    ) -> Result<(), Error> {
        let index = validate(input, output)?;
        self.transform(input, output, transient, index)
    }

    fn transform(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        transient: bool,
        index: usize,
    ) -> Result<(), Error> {
        let n = input.len();
        if transient && n > OVERLAP {
            let count = n / OVERLAP;
            for (time, chunk) in output.as_chunks_mut::<OVERLAP>().0.iter_mut().enumerate() {
                for (frequency, coefficient) in self.transform.short.iter_mut().enumerate() {
                    *coefficient = input[frequency * count + time];
                }
                self.transform.plans[0].inverse(
                    &self.transform.short,
                    &mut self.transform.block[..2 * OVERLAP],
                    &mut self.transform.scratch[0],
                )?;
                for ((destination, &sample), &previous) in chunk
                    .iter_mut()
                    .zip(&self.transform.block[..OVERLAP])
                    .zip(&self.overlap)
                {
                    *destination = sample + previous;
                }
                self.overlap
                    .copy_from_slice(&self.transform.block[OVERLAP..2 * OVERLAP]);
            }
        } else {
            let padding = (n - OVERLAP) / 2;
            self.transform.plans[index].inverse(
                input,
                &mut self.transform.block[..2 * n],
                &mut self.transform.scratch[index],
            )?;
            output.copy_from_slice(&self.transform.block[padding..padding + n]);
            for (destination, &previous) in output.iter_mut().zip(&self.overlap) {
                *destination += previous;
            }
            self.overlap
                .copy_from_slice(&self.transform.block[padding + n..padding + n + OVERLAP]);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signal(length: usize) -> Vec<f32> {
        let mut random = 0x7956_212du32;
        (0..length)
            .map(|_| {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                (random as f64 / u32::MAX as f64 - 0.5) as f32
            })
            .collect()
    }

    fn window(index: usize, n: usize) -> f64 {
        let padding = (n - OVERLAP) / 2;
        let reflected = index.min(2 * n - 1 - index);
        if reflected < padding {
            0.0
        } else if reflected >= padding + OVERLAP {
            1.0
        } else {
            let phase =
                std::f64::consts::FRAC_PI_2 * ((reflected - padding) as f64 + 0.5) / OVERLAP as f64;
            (std::f64::consts::FRAC_PI_2 * phase.sin().powi(2)).sin()
        }
    }

    #[test]
    fn long_and_interleaved_short_coefficients_match_literal_cosine_sum() {
        for n in [120, 240, 480, 960] {
            for transient in [false, true] {
                let source = signal(n + OVERLAP);
                let mut analysis = Analysis::new();
                analysis.previous.copy_from_slice(&source[..OVERLAP]);
                let mut output = vec![0.0; n];
                analysis
                    .forward_preemphasized(&source[OVERLAP..], &mut output, transient)
                    .unwrap();
                let blocks = if transient { n / OVERLAP } else { 1 };
                let size = n / blocks;
                let padding = (size - OVERLAP) / 2;
                let mut error = 0.0;
                let mut energy = 0.0;
                for time in 0..blocks {
                    for frequency in 0..size {
                        let expected = source[time * size..time * size + size + OVERLAP]
                            .iter()
                            .enumerate()
                            .map(|(position, &sample)| {
                                let position = position + padding;
                                let phase = std::f64::consts::PI / size as f64
                                    * (position as f64 + 0.5 + size as f64 / 2.0)
                                    * (frequency as f64 + 0.5);
                                f64::from(sample) * window(position, size) * phase.cos()
                            })
                            .sum::<f64>()
                            * 2.0
                            / size as f64;
                        energy += expected * expected;
                        error += (expected - f64::from(output[frequency * blocks + time])).powi(2);
                    }
                }
                let snr = 10.0 * (energy / error.max(1e-30)).log10();
                assert!(snr > 110.0, "MDCT {n}/{transient}: {snr} dB");
            }
        }
    }

    #[test]
    fn inverse_scaling_and_alignment_match_literal_cosine_sum() {
        for n in [120, 240, 480, 960] {
            let input = signal(n);
            let mut output = vec![0.0; n];
            let mut synthesis = Synthesis::new();
            synthesis
                .inverse_preemphasized(&input, &mut output, false)
                .unwrap();
            let padding = (n - OVERLAP) / 2;
            let mut energy = 0.0;
            let mut error = 0.0;
            for (position, &actual) in output.iter().enumerate() {
                let position = padding + position;
                let expected = input
                    .iter()
                    .enumerate()
                    .map(|(frequency, &coefficient)| {
                        let phase = std::f64::consts::PI / n as f64
                            * (position as f64 + 0.5 + n as f64 / 2.0)
                            * (frequency as f64 + 0.5);
                        f64::from(coefficient) * phase.cos()
                    })
                    .sum::<f64>()
                    * window(position, n);
                energy += expected * expected;
                error += (expected - f64::from(actual)).powi(2);
            }
            let snr = 10.0 * (energy / error.max(1e-30)).log10();
            assert!(snr > 110.0, "IMDCT {n}: {snr} dB");
        }
    }

    #[test]
    fn changing_frame_sizes_and_transients_reconstructs_with_120_sample_delay() {
        let frames = [
            (960, false),
            (480, true),
            (120, false),
            (240, true),
            (960, true),
            (480, false),
        ];
        let length = frames.iter().map(|&(n, _)| n).sum::<usize>();
        let source = signal(length);
        let mut analysis = Analysis::new();
        let mut synthesis = Synthesis::new();
        let mut recovered = Vec::with_capacity(length + OVERLAP);
        let mut coefficients = [0.0; MAX_FRAME];
        let mut output = [0.0; MAX_FRAME];
        let mut position = 0;
        for (n, transient) in frames {
            analysis
                .forward(
                    &source[position..position + n],
                    &mut coefficients[..n],
                    transient,
                )
                .unwrap();
            synthesis
                .inverse(
                    &coefficients[..n],
                    &mut output[..n],
                    transient,
                    PostFilterParams::DISABLED,
                )
                .unwrap();
            recovered.extend_from_slice(&output[..n]);
            position += n;
        }
        analysis
            .forward(&[0.0; OVERLAP], &mut coefficients[..OVERLAP], false)
            .unwrap();
        synthesis
            .inverse(
                &coefficients[..OVERLAP],
                &mut output[..OVERLAP],
                false,
                PostFilterParams::DISABLED,
            )
            .unwrap();
        recovered.extend_from_slice(&output[..OVERLAP]);
        assert!(
            recovered[..OVERLAP]
                .iter()
                .all(|sample| sample.abs() < 3e-6)
        );
        for (&expected, &actual) in source.iter().zip(&recovered[OVERLAP..]) {
            assert!(
                (expected - actual).abs() < 6e-6,
                "{expected} versus {actual}"
            );
        }
    }

    #[test]
    fn pitch_prefilter_exactly_inverts_recursive_postfilter_during_transitions() {
        let source = signal(8 * MAX_FRAME);
        let mut prefilter = CombFilter::new();
        let mut postfilter = CombFilter::new();
        let filters = [
            PostFilterParams::DISABLED,
            PostFilterParams::new(15, 0.75, 0).unwrap(),
            PostFilterParams::new(1022, 0.375, 1).unwrap(),
            PostFilterParams::new(127, 0.65625, 2).unwrap(),
            PostFilterParams::new(61, 0.09375, 0).unwrap(),
            PostFilterParams::DISABLED,
            PostFilterParams::new(15, 0.75, 2).unwrap(),
            PostFilterParams::DISABLED,
        ];
        for (input, parameters) in source.as_chunks::<MAX_FRAME>().0.iter().zip(filters) {
            let mut output = input.to_vec();
            prefilter.process(&mut output, parameters, true);
            postfilter.process(&mut output, parameters, false);
            for (&expected, actual) in input.iter().zip(output) {
                assert!((expected - actual).abs() < 3e-7);
            }
        }
    }

    #[test]
    fn pitch_parameters_follow_the_overlap_delay_when_frame_sizes_change() {
        let frames = [
            (120, false),
            (120, false),
            (960, false),
            (240, true),
            (480, false),
            (120, false),
            (960, true),
        ];
        let length = frames.iter().map(|&(n, _)| n).sum();
        let source = signal(length);
        let mut analysis = Analysis::new();
        let mut synthesis = Synthesis::new();
        let mut recovered = Vec::with_capacity(length + OVERLAP);
        let mut coefficients = [0.0; MAX_FRAME];
        let mut output = [0.0; MAX_FRAME];
        let mut position = 0;
        for (frame, (n, transient)) in frames.into_iter().enumerate() {
            let parameters =
                PostFilterParams::new(15 + frame * 27, (frame % 5) as f32 * 0.09375, frame % 3)
                    .unwrap();
            analysis
                .forward_with_filter(
                    &source[position..position + n],
                    &mut coefficients[..n],
                    transient,
                    parameters,
                )
                .unwrap();
            synthesis
                .inverse(&coefficients[..n], &mut output[..n], transient, parameters)
                .unwrap();
            recovered.extend_from_slice(&output[..n]);
            position += n;
        }
        analysis
            .forward(&[0.0; OVERLAP], &mut coefficients[..OVERLAP], false)
            .unwrap();
        synthesis
            .inverse(
                &coefficients[..OVERLAP],
                &mut output[..OVERLAP],
                false,
                PostFilterParams::DISABLED,
            )
            .unwrap();
        recovered.extend_from_slice(&output[..OVERLAP]);
        for (&expected, &actual) in source.iter().zip(&recovered[OVERLAP..]) {
            assert!(
                (expected - actual).abs() < 8e-6,
                "pitch-filter delay: {expected} versus {actual}"
            );
        }
    }

    #[test]
    fn reset_and_invalid_calls_preserve_stream_contract() {
        let mut analysis = Analysis::new();
        let mut synthesis = Synthesis::new();
        let input = signal(240);
        let mut coefficients = [0.0; 240];
        let mut output = [0.0; 240];
        analysis.forward(&input, &mut coefficients, false).unwrap();
        synthesis
            .inverse(
                &coefficients,
                &mut output,
                false,
                PostFilterParams::DISABLED,
            )
            .unwrap();
        let expected = coefficients;
        analysis.reset();
        synthesis.reset();
        let mut bad = input.clone();
        bad[73] = f32::NAN;
        assert_eq!(
            analysis.forward(&bad, &mut coefficients, false),
            Err(Error::Sample)
        );
        assert_eq!(coefficients, expected);
        output.fill(7.0);
        assert_eq!(
            synthesis.inverse(
                &[f32::INFINITY; 240],
                &mut output,
                false,
                PostFilterParams::DISABLED
            ),
            Err(Error::Sample)
        );
        assert_eq!(output, [7.0; 240]);
        analysis.forward(&input, &mut coefficients, false).unwrap();
        assert_eq!(coefficients, expected);
        assert!(PostFilterParams::new(14, 0.5, 0).is_err());
        assert!(PostFilterParams::new(1023, 0.5, 0).is_err());
        assert!(PostFilterParams::new(15, f32::NAN, 0).is_err());
        assert!(PostFilterParams::new(15, 0.751, 0).is_err());
        assert!(PostFilterParams::new(15, 0.5, 3).is_err());
    }
}
