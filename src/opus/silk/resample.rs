// SPDX-License-Identifier: MIT
//! Independently designed rational interpolation from SILK rates to 48 kHz.
//!
//! RFC 6716 section 4.2.9 permits an alternative resampler. A linear-phase
//! low-pass cannot reproduce the reference's frequency-dependent phase, which
//! matters when the SILK and CELT layers are summed in hybrid packets. This
//! model uses one shared stable third-order IIR and small polyphase numerators.
//!
//! Coefficients are Snacka-owned mathematical model data: least-squares design
//! against black-box impulse observations, followed by a minimum frequency-error
//! projection enforcing unit DC gain and the Table 54 delay for each phase.
//! No implementation source or coefficient tables were copied. Reproduce them
//! with `tools/reference/design_opus_resampler.py`; the reference is used only
//! by that development tool. Normal construction, reset and processing allocate
//! nothing, execute no trigonometry, and use only immutable inline constants.

use super::{Bandwidth, Error};

const HISTORY: usize = 64;
const TAPS: usize = 14;
const FILTER_TAPS: usize = 48;
const MAX_OUTPUT_FILTER: usize = 97;
const TAIL_SAMPLES: usize = 96;
const SAMPLE_LIMIT: f32 = f32::MAX / 16.0;
const STATE_LIMIT: f64 = f32::MAX as f64 * 8.0;

// BEGIN GENERATED MODEL
const FEEDBACK: [f64; 3] = [1.8113962145672327, 1.0364310311595564, 0.18601825545020234];

const NUMERATORS: [[[f64; 14]; 6]; 3] = [
    [
        [
            0.0001415085832446766,
            0.00036908163207878184,
            0.0007365733535097278,
            0.03845647726547991,
            0.4975283669979194,
            1.5779899586156612,
            1.5826074858167047,
            0.37635350035185133,
            -0.04387776164119633,
            0.006073848057575448,
            -0.0016155260619540378,
            -0.000252572402291329,
            -0.000531471480851255,
            -0.00013396791074157571,
        ],
        [
            0.00013566448404986247,
            0.0003535511323684365,
            0.0011103497495044687,
            0.06684882372213284,
            0.6563992797967845,
            1.7096936861117777,
            1.4057113955803786,
            0.22816944053600866,
            -0.03859168384581491,
            0.006788675292717912,
            -0.00197037036833453,
            -0.0001446528591147302,
            -0.0005286022818396195,
            -0.00013005587362863053,
        ],
        [
            0.00013850540958476774,
            0.0004137202947073574,
            0.002335439061111348,
            0.10978285441546326,
            0.8355880354210795,
            1.7955023432432295,
            1.1999082182391556,
            0.11471400240201593,
            -0.027580566665066593,
            0.005624934227809102,
            -0.0017896295737402689,
            -0.00017602171170818105,
            -0.0004895638937996678,
            -0.0001267696928510099,
        ],
        [
            0.00014012072308959518,
            0.0004490134084052666,
            0.005029174249610736,
            0.17125291221266553,
            1.0281893303631298,
            1.8273561304909607,
            0.9800430095897564,
            0.03534564266271869,
            -0.015264107625840833,
            0.0036129877290123446,
            -0.0013601321611026525,
            -0.0003178056009672872,
            -0.00047292898881375625,
            -0.00015784587563427634,
        ],
        [
            0.00013321578688787642,
            0.0004306567333796186,
            0.010506546738076047,
            0.25489616299872636,
            1.2244168005559146,
            1.8013375790471051,
            0.7611605363564529,
            -0.013601245875831875,
            -0.004939243024019487,
            0.0013175643339491368,
            -0.0008168416615864722,
            -0.00043757987612721256,
            -0.0004120351189782075,
            -0.0001466158169573112,
        ],
        [
            0.0001328081845546795,
            0.0005211156221601558,
            0.020822919497954354,
            0.3633160797978524,
            1.4120279490678083,
            1.717909328046836,
            0.5564619526459074,
            -0.03774491324119391,
            0.002285085700893934,
            -0.0004515725134743889,
            -0.0003724282958815947,
            -0.0005283581275277345,
            -0.00038294604252893513,
            -0.00015151916636965384,
        ],
    ],
    [
        [
            0.0001415085832446766,
            0.00036908163207878184,
            0.0007365733535097278,
            0.03845647726547991,
            0.4975283669979194,
            1.5779899586156612,
            1.5826074858167047,
            0.37635350035185133,
            -0.04387776164119633,
            0.006073848057575448,
            -0.0016155260619540378,
            -0.000252572402291329,
            -0.000531471480851255,
            -0.00013396791074157571,
        ],
        [
            0.0001370738929628828,
            0.00035766341592950593,
            0.0015461602120283204,
            0.08624601330197318,
            0.7438692320415481,
            1.7589774384336005,
            1.3055157562171502,
            0.16704302794543363,
            -0.03349158983675282,
            0.006374964503250641,
            -0.0019383550208290912,
            -0.00015031282585240013,
            -0.0005102818097421878,
            -0.0001312892937095781,
        ],
        [
            0.00014012072308959518,
            0.0004490134084052666,
            0.005029174249610736,
            0.17125291221266553,
            1.0281893303631298,
            1.8273561304909607,
            0.9800430095897564,
            0.03534564266271869,
            -0.015264107625840833,
            0.0036129877290123446,
            -0.0013601321611026525,
            -0.0003178056009672872,
            -0.00047292898881375625,
            -0.00015784587563427634,
        ],
        [
            0.00012905786148452816,
            0.0004488524565598071,
            0.014900943615433274,
            0.305910222730804,
            1.3201334845183865,
            1.7665604172440748,
            0.6562814666834075,
            -0.02836688949317045,
            -0.0008926872470345921,
            0.00034863872322482483,
            -0.0005700857946174607,
            -0.0004818946032874383,
            -0.00039572838512837096,
            -0.00016029713314626723,
        ],
        [
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ],
        [
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ],
    ],
    [
        [
            0.0004592702771443036,
            0.0012036538314767224,
            0.0017350556473908795,
            0.03928758062690455,
            0.49812200943036317,
            1.5783461467124626,
            1.5827262129845254,
            0.3762347731840302,
            -0.044233949737998005,
            0.005480205625131799,
            -0.0024466294233783555,
            -0.0012510546961722673,
            -0.0013660436802492171,
            -0.0004517296046412623,
        ],
        [
            0.00045626710348442963,
            0.0012482924941053897,
            0.00333392135499261,
            0.110613957776888,
            0.8361816778535234,
            1.7958585313400313,
            1.2000269454069763,
            0.11459527523419477,
            -0.027936754761868306,
            0.005031291795365388,
            -0.002620732935164678,
            -0.0011745040055892292,
            -0.0013241360931977217,
            -0.0004445313867507315,
        ],
        [
            0.0004509774807875034,
            0.0012652289327775594,
            0.011505029031957197,
            0.255727266360151,
            1.2250104429883584,
            1.8016937671439066,
            0.7612792635242737,
            -0.013719973043653019,
            -0.005295431120821159,
            0.0007239219015054882,
            -0.0016479450230107897,
            -0.0014360621700081509,
            -0.0012466073183761694,
            -0.00046437751085699785,
        ],
        [
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ],
        [
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ],
        [
            0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0,
        ],
    ],
];
// END GENERATED MODEL

// Tap-major storage lets independent phases accumulate together while each
// output retains the original newest-to-oldest floating-point sum.
const PHASE_NUMERATORS: [[[f64; 6]; TAPS]; 3] = {
    let mut result = [[[0.0; 6]; TAPS]; 3];
    let mut family = 0;
    while family < 3 {
        let mut tap = 0;
        while tap < TAPS {
            let mut phase = 0;
            while phase < 6 {
                result[family][tap][phase] = NUMERATORS[family][phase][tap];
                phase += 1;
            }
            tap += 1;
        }
        family += 1;
    }
    result
};

/// Composition of the owned interpolation numerator and an output low-pass.
/// The stable feedback denominator remains shared with normal interpolation.
pub(crate) struct FilterPlan {
    factor: usize,
    step: usize,
    taps: usize,
    numerators: [[f64; FILTER_TAPS]; 6],
}

impl FilterPlan {
    pub(crate) fn new(
        bandwidth: Bandwidth,
        rate: u32,
        coefficients: &[f32],
    ) -> Result<Self, Error> {
        let (factor, family) = match bandwidth {
            Bandwidth::Narrow => (6, 0),
            Bandwidth::Medium => (4, 1),
            Bandwidth::Wide => (3, 2),
        };
        let step = match rate {
            8_000 => 6,
            12_000 => 4,
            _ => return Err(Error::Argument),
        };
        if step <= factor
            || coefficients.is_empty()
            || coefficients.len() > MAX_OUTPUT_FILTER
            || coefficients.iter().any(|value| !value.is_finite())
            || coefficients
                .iter()
                .map(|&value| f64::from(value).abs())
                .sum::<f64>()
                > 4.0
        {
            return Err(Error::Argument);
        }
        let taps = TAPS + (coefficients.len() - 1).div_ceil(factor);
        let mut numerators = [[0.0; FILTER_TAPS]; 6];
        for (phase, destination) in numerators[..factor].iter_mut().enumerate() {
            for (tap, &coefficient) in coefficients.iter().enumerate() {
                let source_phase = (phase + factor - tap % factor) % factor;
                let shift = (tap + factor - 1 - phase) / factor;
                for (lag, &value) in NUMERATORS[family][source_phase].iter().enumerate() {
                    destination[shift + lag] += f64::from(coefficient) * value;
                }
            }
        }
        Ok(Self {
            factor,
            step,
            taps,
            numerators,
        })
    }
}

pub(crate) struct Resampler {
    factor: usize,
    delay: usize,
    numerators: &'static [[f64; TAPS]; 6],
    phase_numerators: &'static [[f64; 6]; TAPS],
    // Mirrored halves make each fourteen-sample window contiguous. This
    // removes ring-wrap branches from every phase without changing tap order.
    history: [f64; 2 * HISTORY],
    cursor: usize,
    clock: usize,
}

impl Resampler {
    pub(crate) fn new(bandwidth: Bandwidth) -> Self {
        let (factor, delay, family) = match bandwidth {
            Bandwidth::Narrow => (6, 0, 0),
            Bandwidth::Medium => (4, 4, 1),
            Bandwidth::Wide => (3, 7, 2),
        };
        Self {
            factor,
            delay,
            numerators: &NUMERATORS[family],
            phase_numerators: &PHASE_NUMERATORS[family],
            history: [0.0; 2 * HISTORY],
            cursor: 0,
            clock: 0,
        }
    }

    pub(crate) fn reset(&mut self, bandwidth: Bandwidth) {
        *self = Self::new(bandwidth);
    }

    pub(crate) fn ratio(&self) -> usize {
        self.factor
    }

    /// Reconstruct the chronological 48 kHz tail needed to prime an output
    /// filter after direct native-domain filtering. This consumes no history.
    pub(crate) fn tail_48k(&self, output: &mut [f32]) -> Result<(), Error> {
        if output.len() > TAIL_SAMPLES {
            return Err(Error::Output);
        }
        self.validate(&[])?;
        let length = output.len();
        for (index, destination) in output.iter_mut().enumerate() {
            let back = length - 1 - index;
            let phase = self.factor - 1 - back % self.factor;
            let begin = self.cursor + HISTORY - 1 - self.delay - back / self.factor;
            let mut sum = 0.0;
            for (&value, &coefficient) in self.history[begin + 1 - TAPS..=begin]
                .iter()
                .rev()
                .zip(&self.numerators[phase])
            {
                sum += value * coefficient;
            }
            *destination = sum as f32;
        }
        Ok(())
    }

    /// Filter below the native SILK rate without constructing unused 48 kHz
    /// samples. The immutable plan is built once from the owned filter taps.
    pub(crate) fn process_filtered(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        plan: &FilterPlan,
    ) -> Result<(), Error> {
        if plan.factor != self.factor {
            return Err(Error::Argument);
        }
        let span = input.len().checked_mul(self.factor).ok_or(Error::Output)?;
        if self.output_count(span, plan.step) != output.len() {
            return Err(Error::Output);
        }
        self.validate(input)?;
        match (self.factor, plan.step) {
            (3, 4) => self.run_filtered::<3, 4>(input, output, plan),
            (3, 6) => self.run_filtered::<3, 6>(input, output, plan),
            (4, 6) => self.run_filtered::<4, 6>(input, output, plan),
            _ => unreachable!("validated below-native filter geometry"),
        }
        self.clock = (self.clock + span % 12) % 12;
        Ok(())
    }

    /// Produce exactly `input.len() * ratio()` samples. Invalid lengths or
    /// numeric inputs leave both output and streaming history unchanged.
    pub(crate) fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), Error> {
        self.process_rate(input, output, 48_000)
    }

    /// Evaluate only phases needed at a standard output rate at or above the
    /// native SILK rate. The global 48 kHz sample clock carries across chunks
    /// and rate changes; history matches full-rate interpolation exactly.
    pub(crate) fn process_rate(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        rate: u32,
    ) -> Result<(), Error> {
        let step = match rate {
            8_000 => 6,
            12_000 => 4,
            16_000 => 3,
            24_000 => 2,
            48_000 => 1,
            _ => return Err(Error::Argument),
        };
        if step > self.factor {
            return Err(Error::Argument);
        }
        let span = input.len().checked_mul(self.factor).ok_or(Error::Output)?;
        if self.output_count(span, step) != output.len() {
            return Err(Error::Output);
        }
        self.validate(input)?;
        // All poles are real, negative and below 0.848 in magnitude. The
        // feedback transfer has l1 gain <25.64, and every complete phase has
        // l1 gain <3.12. SAMPLE_LIMIT therefore bounds states well inside f64
        // and output inside f32, even for alternating maximum-domain samples.
        match (self.factor, step) {
            (3, 1) => self.run::<3>(input, output),
            (4, 1) => self.run::<4>(input, output),
            (6, 1) => self.run::<6>(input, output),
            (3, 2) => self.run_rate::<3, 2>(input, output),
            (3, 3) => self.run_rate::<3, 3>(input, output),
            (4, 2) => self.run_rate::<4, 2>(input, output),
            (4, 3) => self.run_rate::<4, 3>(input, output),
            (4, 4) => self.run_rate::<4, 4>(input, output),
            (6, 2) => self.run_rate::<6, 2>(input, output),
            (6, 3) => self.run_rate::<6, 3>(input, output),
            (6, 4) => self.run_rate::<6, 4>(input, output),
            (6, 6) => self.run_rate::<6, 6>(input, output),
            _ => unreachable!("constructor supports only SILK sampling rates"),
        }
        // All standard rate divisors divide twelve, so this bounded clock
        // preserves absolute sample phase even when the output rate changes.
        self.clock = (self.clock + span % 12) % 12;
        debug_assert!(output.iter().all(|sample| sample.is_finite()));
        Ok(())
    }

    fn output_count(&self, span: usize, step: usize) -> usize {
        let first = (step - self.clock % step) % step;
        if first < span {
            1 + (span - 1 - first) / step
        } else {
            0
        }
    }

    fn validate(&self, input: &[f32]) -> Result<(), Error> {
        if input
            .iter()
            .any(|sample| !sample.is_finite() || sample.abs() > SAMPLE_LIMIT)
            || self.history[..HISTORY]
                .iter()
                .any(|sample| !sample.is_finite() || sample.abs() > STATE_LIMIT)
        {
            return Err(Error::Argument);
        }
        Ok(())
    }

    fn run_filtered<const PHASES: usize, const STEP: usize>(
        &mut self,
        input: &[f32],
        output: &mut [f32],
        plan: &FilterPlan,
    ) {
        let mut clock = self.clock % STEP;
        let mut destination = 0;
        for &sample in input {
            let mut filtered = f64::from(sample);
            for (lag, &coefficient) in FEEDBACK.iter().enumerate() {
                filtered -= coefficient * self.history[self.cursor + HISTORY - 1 - lag];
            }
            self.history[self.cursor] = filtered;
            self.history[self.cursor + HISTORY] = filtered;
            let begin = self.cursor + HISTORY - self.delay;
            let past = &self.history[begin + 1 - plan.taps..=begin];
            let first = if clock == 0 { 0 } else { STEP - clock };
            for phase in (first..PHASES).step_by(STEP) {
                let mut sum = 0.0;
                for (&past, &coefficient) in
                    past.iter().rev().zip(&plan.numerators[phase][..plan.taps])
                {
                    sum += past * coefficient;
                }
                output[destination] = sum as f32;
                destination += 1;
            }
            clock = (clock + PHASES) % STEP;
            self.cursor = (self.cursor + 1) & (HISTORY - 1);
        }
        debug_assert_eq!(destination, output.len());
    }

    fn run_rate<const PHASES: usize, const STEP: usize>(
        &mut self,
        input: &[f32],
        output: &mut [f32],
    ) {
        let mut clock = self.clock % STEP;
        let mut destination = 0;
        for &sample in input {
            let mut filtered = f64::from(sample);
            for (lag, &coefficient) in FEEDBACK.iter().enumerate() {
                filtered -= coefficient * self.history[self.cursor + HISTORY - 1 - lag];
            }
            self.history[self.cursor] = filtered;
            self.history[self.cursor + HISTORY] = filtered;
            let begin = self.cursor + HISTORY - self.delay;
            let past = &self.history[begin + 1 - TAPS..=begin];
            let first = if clock == 0 { 0 } else { STEP - clock };
            for phase in (first..PHASES).step_by(STEP) {
                let mut sum = 0.0;
                for (&past, &coefficient) in past.iter().rev().zip(&self.numerators[phase]) {
                    sum += past * coefficient;
                }
                output[destination] = sum as f32;
                destination += 1;
            }
            clock = (clock + PHASES) % STEP;
            self.cursor = (self.cursor + 1) & (HISTORY - 1);
        }
        debug_assert_eq!(destination, output.len());
    }

    fn run<const PHASES: usize>(&mut self, input: &[f32], output: &mut [f32]) {
        for (&sample, block) in input.iter().zip(output.as_chunks_mut::<PHASES>().0) {
            let mut filtered = f64::from(sample);
            for (lag, &coefficient) in FEEDBACK.iter().enumerate() {
                filtered -= coefficient * self.history[self.cursor + HISTORY - 1 - lag];
            }
            self.history[self.cursor] = filtered;
            self.history[self.cursor + HISTORY] = filtered;
            let mut sums = [0.0f64; PHASES];
            let begin = self.cursor + HISTORY - self.delay;
            for (&past, coefficients) in self.history[begin + 1 - TAPS..=begin]
                .iter()
                .rev()
                .zip(self.phase_numerators)
            {
                for (phase, sum) in sums.iter_mut().enumerate() {
                    *sum += past * coefficients[phase];
                }
            }
            for (destination, &value) in block.iter_mut().zip(&sums) {
                *destination = value as f32;
            }
            self.cursor = (self.cursor + 1) & (HISTORY - 1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CASES: [(Bandwidth, usize, f64); 3] = [
        (Bandwidth::Narrow, 6, 4.304),
        (Bandwidth::Medium, 4, 8.304),
        (Bandwidth::Wide, 3, 11.296),
    ];

    fn literal(plan: &Resampler, input: &[f32]) -> Vec<f64> {
        let mut filtered = vec![0.0; input.len()];
        for (index, &sample) in input.iter().enumerate() {
            filtered[index] = f64::from(sample);
            for (lag, &coefficient) in FEEDBACK.iter().enumerate() {
                if index > lag {
                    filtered[index] -= coefficient * filtered[index - lag - 1];
                }
            }
        }
        (0..input.len() * plan.factor)
            .map(|index| {
                let sample = index / plan.factor;
                let phase = index % plan.factor;
                if sample < plan.delay {
                    return 0.0;
                }
                let sample = sample - plan.delay;
                (0..TAPS.min(sample + 1))
                    .map(|lag| plan.numerators[phase][lag] * filtered[sample - lag])
                    .sum()
            })
            .collect()
    }

    #[test]
    fn streaming_matches_literal_double_precision_convolution_and_chunking() {
        let input: Vec<f32> = (0..1024)
            .map(|i| ((i * 73 % 103) as f32 - 51.0) / 64.0)
            .collect();
        for (bandwidth, factor, _) in CASES {
            let mut plan = Resampler::new(bandwidth);
            assert_eq!(plan.ratio(), factor);
            let expected = literal(&plan, &input);
            let mut output = vec![0.0; input.len() * factor];
            plan.process(&input, &mut output).unwrap();
            for (&actual, &reference) in output.iter().zip(&expected) {
                assert!((f64::from(actual) - reference).abs() < 3e-7);
            }
            let mut split = Resampler::new(bandwidth);
            let mut chunked = vec![0.0; output.len()];
            let mut offset = 0;
            for width in [1, 31, 2, 57, 3, 67, 11, 5, 101, 7, 43] {
                split
                    .process(
                        &input[offset..offset + width],
                        &mut chunked[offset * factor..(offset + width) * factor],
                    )
                    .unwrap();
                offset += width;
            }
            split
                .process(&input[offset..], &mut chunked[offset * factor..])
                .unwrap();
            assert_eq!(chunked, output);
            assert_eq!(split.history, plan.history);
            assert_eq!(split.cursor, plan.cursor);
        }
    }

    #[test]
    fn selected_rates_are_exact_full_rate_phases_across_chunks_and_rate_changes() {
        let input: Vec<f32> = (0..512)
            .map(|i| ((i * 73 % 103) as f32 - 51.0) / 64.0)
            .collect();
        for (bandwidth, factor, _) in CASES {
            let mut reference = Resampler::new(bandwidth);
            let mut full = vec![0.0; input.len() * factor];
            reference.process(&input, &mut full).unwrap();
            for rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
                if rate < 48_000 / factor as u32 {
                    continue;
                }
                let step = 48_000 / rate as usize;
                let mut selected = Resampler::new(bandwidth);
                let mut offset = 0;
                for width in [1, 31, 2, 57, 3, 67, 11, 5, 101, 7, 43, 184] {
                    let begin = offset * factor;
                    let end = (offset + width) * factor;
                    let expected: Vec<_> = (begin..end)
                        .filter(|index| index.is_multiple_of(step))
                        .map(|index| full[index])
                        .collect();
                    let mut output = vec![0.0; expected.len()];
                    selected
                        .process_rate(&input[offset..offset + width], &mut output, rate)
                        .unwrap();
                    assert_eq!(output, expected, "{bandwidth:?}, {rate}, {offset}");
                    offset += width;
                }
                assert_eq!(offset, input.len());
                assert_eq!(selected.history, reference.history);
                assert_eq!(selected.cursor, reference.cursor);
                assert_eq!(selected.clock, reference.clock);
            }

            let mut selected = Resampler::new(bandwidth);
            let mut offset = 0;
            for (width, rate) in [
                (1, 48_000),
                (5, 24_000),
                (7, 48_000 / factor as u32),
                (9, 48_000),
            ] {
                let step = 48_000 / rate as usize;
                let begin = offset * factor;
                let end = (offset + width) * factor;
                let expected: Vec<_> = (begin..end)
                    .filter(|index| index.is_multiple_of(step))
                    .map(|index| full[index])
                    .collect();
                let mut output = vec![0.0; expected.len()];
                selected
                    .process_rate(&input[offset..offset + width], &mut output, rate)
                    .unwrap();
                assert_eq!(output, expected);
                offset += width;
            }
            let before = (selected.history, selected.cursor, selected.clock);
            let mut output = [123.0];
            assert_eq!(
                selected.process_rate(&[0.0], &mut output, 44_100),
                Err(Error::Argument)
            );
            assert_eq!(
                selected.process_rate(&[0.0], &mut output, 48_000),
                Err(Error::Output)
            );
            let mut full_output = vec![123.0; factor];
            assert_eq!(
                selected.process_rate(&[f32::NAN], &mut full_output, 48_000),
                Err(Error::Argument)
            );
            assert_eq!((selected.history, selected.cursor, selected.clock), before);
            assert_eq!(output, [123.0]);
            assert!(full_output.iter().all(|&sample| sample == 123.0));
        }
    }

    fn low_pass(rate: u32) -> [f32; MAX_OUTPUT_FILTER] {
        let mut taps = [0.0; MAX_OUTPUT_FILTER];
        let cutoff = 0.45 * f64::from(rate) / 48_000.0;
        let middle = (MAX_OUTPUT_FILTER - 1) as f64 * 0.5;
        for (index, tap) in taps.iter_mut().enumerate() {
            let position = index as f64 - middle;
            let phase = std::f64::consts::TAU * index as f64 / (MAX_OUTPUT_FILTER - 1) as f64;
            let window = 0.42 - 0.5 * phase.cos() + 0.08 * (2.0 * phase).cos();
            let sinc = if position == 0.0 {
                2.0 * cutoff
            } else {
                (std::f64::consts::TAU * cutoff * position).sin()
                    / (std::f64::consts::PI * position)
            };
            *tap = (window * sinc) as f32;
        }
        let gain = taps.iter().sum::<f32>();
        for tap in &mut taps {
            *tap /= gain;
        }
        taps
    }

    #[test]
    fn fused_filters_match_the_literal_owned_chain_and_reconstruct_transition_tails() {
        let input: Vec<f32> = (0..512)
            .map(|i| ((i * 73 % 103) as f32 - 51.0) / 64.0)
            .collect();
        for (bandwidth, factor, _) in CASES {
            for rate in [8_000, 12_000] {
                if rate >= 48_000 / factor as u32 {
                    continue;
                }
                let coefficients = low_pass(rate);
                let plan = FilterPlan::new(bandwidth, rate, &coefficients).unwrap();
                let mut reference = Resampler::new(bandwidth);
                let mut full = vec![0.0; input.len() * factor];
                reference.process(&input, &mut full).unwrap();
                let step = 48_000 / rate as usize;
                let mut selected = Resampler::new(bandwidth);
                let mut offset = 0;
                let mut maximum_error = 0.0f32;
                for width in [1, 31, 2, 57, 3, 67, 11, 5, 101, 7, 43, 184] {
                    let begin = offset * factor;
                    let end = (offset + width) * factor;
                    let expected: Vec<f32> = (begin..end)
                        .filter(|index| index.is_multiple_of(step))
                        .map(|index| {
                            coefficients[..MAX_OUTPUT_FILTER.min(index + 1)]
                                .iter()
                                .enumerate()
                                .map(|(lag, &tap)| f64::from(tap) * f64::from(full[index - lag]))
                                .sum::<f64>() as f32
                        })
                        .collect();
                    let mut output = vec![0.0; expected.len()];
                    selected
                        .process_filtered(&input[offset..offset + width], &mut output, &plan)
                        .unwrap();
                    for (&actual, &expected) in output.iter().zip(&expected) {
                        maximum_error = maximum_error.max((actual - expected).abs());
                    }
                    offset += width;
                }
                // Composition removes the intermediate f32 quantization. Its
                // error stays below that original output-filter precision.
                assert!(
                    maximum_error < 2e-7,
                    "{bandwidth:?}, {rate}: {maximum_error}"
                );
                assert_eq!(selected.history, reference.history);
                assert_eq!(selected.cursor, reference.cursor);
                assert_eq!(selected.clock, reference.clock);
                let mut tail = [123.0; TAIL_SAMPLES];
                selected.tail_48k(&mut tail).unwrap();
                assert_eq!(tail, full[full.len() - TAIL_SAMPLES..]);
                assert_eq!(selected.history[..HISTORY], selected.history[HISTORY..]);

                let before = (selected.history, selected.cursor, selected.clock);
                assert_eq!(
                    selected.process_filtered(&[0.0; 12], &mut [], &plan),
                    Err(Error::Output)
                );
                let mut too_long = [123.0; TAIL_SAMPLES + 1];
                assert_eq!(selected.tail_48k(&mut too_long), Err(Error::Output));
                assert_eq!(too_long, [123.0; TAIL_SAMPLES + 1]);
                assert_eq!((selected.history, selected.cursor, selected.clock), before);
            }
        }
        assert!(FilterPlan::new(Bandwidth::Narrow, 8_000, &[1.0]).is_err());
        assert!(FilterPlan::new(Bandwidth::Wide, 8_000, &[]).is_err());
        assert!(FilterPlan::new(Bandwidth::Wide, 8_000, &[f32::NAN]).is_err());
        assert!(FilterPlan::new(Bandwidth::Wide, 8_000, &[4.001]).is_err());
        assert!(FilterPlan::new(Bandwidth::Wide, 8_000, &[0.0; 98]).is_err());
    }

    #[test]
    fn dc_gain_and_fractional_impulse_centroid_match_the_prescribed_delay() {
        for (bandwidth, factor, delay) in CASES {
            let mut plan = Resampler::new(bandwidth);
            let input = [0.75; 128];
            let mut output = vec![0.0; input.len() * factor];
            plan.process(&input, &mut output).unwrap();
            // Recursive transients need a settling interval before measuring DC.
            for &sample in &output[96 * factor..] {
                assert!((sample - 0.75).abs() < 2e-7);
            }
            plan.reset(bandwidth);
            let mut impulse = [0.0; 256];
            impulse[0] = 1.0;
            let mut output = vec![0.0; impulse.len() * factor];
            plan.process(&impulse, &mut output).unwrap();
            let mass: f64 = output.iter().map(|&sample| f64::from(sample)).sum();
            let moment: f64 = output
                .iter()
                .enumerate()
                .map(|(index, &sample)| index as f64 * f64::from(sample))
                .sum();
            assert!((mass - factor as f64).abs() < 1e-6);
            assert!((moment / mass - delay * factor as f64).abs() < 1e-5);
            assert!(
                output[224 * factor..]
                    .iter()
                    .all(|&sample| sample.abs() < 1e-14)
            );
        }
    }

    fn response(plan: &Resampler, frequency: f64) -> (f64, f64) {
        let omega = 2.0 * std::f64::consts::PI * frequency;
        let mut nr = 0.0;
        let mut ni = 0.0;
        for phase in 0..plan.factor {
            for (lag, &coefficient) in plan.numerators[phase].iter().enumerate() {
                let angle = -omega * ((lag + plan.delay) * plan.factor + phase) as f64;
                nr += coefficient * angle.cos();
                ni += coefficient * angle.sin();
            }
        }
        let mut dr = 1.0;
        let mut di = 0.0;
        for (lag, &coefficient) in FEEDBACK.iter().enumerate() {
            let angle = -omega * ((lag + 1) * plan.factor) as f64;
            dr += coefficient * angle.cos();
            di += coefficient * angle.sin();
        }
        let denominator = (dr * dr + di * di) * plan.factor as f64;
        (
            (nr * dr + ni * di) / denominator,
            (ni * dr - nr * di) / denominator,
        )
    }

    #[test]
    fn dense_full_native_band_keeps_passband_and_limits_sampling_images() {
        for (bandwidth, factor, _) in CASES {
            let plan = Resampler::new(bandwidth);
            for step in 0..=500 {
                let frequency = step as f64 / 1000.0;
                let (real, imaginary) = response(&plan, frequency / factor as f64);
                let gain = real.hypot(imaginary);
                let lower = if frequency <= 0.45 {
                    0.99
                } else if frequency <= 0.47 {
                    0.975
                } else {
                    0.69
                };
                assert!(
                    gain >= lower && gain <= 1.005,
                    "{bandwidth:?} f={frequency} gain={gain}"
                );
                if frequency <= 0.40 {
                    for image in 1..factor {
                        let (real, imaginary) =
                            response(&plan, (image as f64 + frequency) / factor as f64);
                        assert!(
                            real.hypot(imaginary) < 0.0035,
                            "{bandwidth:?} image={image} f={frequency}"
                        );
                    }
                }
            }
            for step in 0..=2048 {
                let (real, imaginary) = response(&plan, step as f64 / 4096.0);
                assert!(
                    real.is_finite() && imaginary.is_finite() && real.hypot(imaginary) <= 1.005
                );
            }
        }
    }

    #[test]
    fn complex_response_matches_independent_black_box_observations() {
        let fixture = include_bytes!("../testdata/resampler-response.bin");
        assert_eq!(&fixture[..8], b"SNKRSF01");
        let count = u32::from_le_bytes(fixture[8..12].try_into().unwrap()) as usize;
        assert_eq!(fixture.len(), 12 + CASES.len() * count * 24);
        let mut records = fixture[12..].as_chunks::<24>().0.iter();
        for (bandwidth, factor, _) in CASES {
            let plan = Resampler::new(bandwidth);
            for _ in 0..count {
                let record = records.next().unwrap();
                let frequency = f64::from_le_bytes(record[..8].try_into().unwrap());
                let expected_real = f64::from_le_bytes(record[8..16].try_into().unwrap());
                let expected_imaginary = f64::from_le_bytes(record[16..].try_into().unwrap());
                let (real, imaginary) = response(&plan, frequency / factor as f64);
                assert!(
                    (real - expected_real).hypot(imaginary - expected_imaginary) < 0.0035,
                    "{bandwidth:?} input frequency={frequency}"
                );
            }
        }
    }

    #[test]
    fn schur_stability_and_absolute_kernel_bounds_cover_the_numeric_domain() {
        // Schur reduction independently checks that all feedback poles lie
        // strictly inside the unit circle, without a polynomial root solver.
        let mut polynomial = [1.0, FEEDBACK[0], FEEDBACK[1], FEEDBACK[2]];
        for degree in (1..=3).rev() {
            let reflection = polynomial[degree];
            assert!(reflection.abs() < 1.0);
            let previous = polynomial;
            for index in 1..degree {
                polynomial[index] = (previous[index] - reflection * previous[degree - index])
                    / (1.0 - reflection * reflection);
            }
        }
        let mut recursive = [0.0; 512];
        recursive[0] = 1.0;
        for index in 1..recursive.len() {
            for lag in 0..3.min(index) {
                recursive[index] -= FEEDBACK[lag] * recursive[index - lag - 1];
            }
        }
        let state_gain: f64 = recursive.iter().map(|sample| sample.abs()).sum();
        assert!(state_gain < 25.64);
        assert!(state_gain * f64::from(SAMPLE_LIMIT) < STATE_LIMIT);
        let mut impulse = [0.0; 512];
        impulse[0] = 1.0;
        for (bandwidth, factor, _) in CASES {
            let plan = Resampler::new(bandwidth);
            let output = literal(&plan, &impulse);
            for phase in 0..factor {
                let gain: f64 = output
                    .iter()
                    .skip(phase)
                    .step_by(factor)
                    .map(|sample| sample.abs())
                    .sum();
                assert!(gain < 3.12);
                assert!(gain * f64::from(SAMPLE_LIMIT) < f64::from(f32::MAX));
            }
        }
    }

    #[test]
    fn alternating_extreme_domain_is_bounded_and_decays_after_reset() {
        for (bandwidth, factor, _) in CASES {
            let mut plan = Resampler::new(bandwidth);
            let input: Vec<_> = (0..4096)
                .map(|i| {
                    if i % 2 == 0 {
                        SAMPLE_LIMIT
                    } else {
                        -SAMPLE_LIMIT
                    }
                })
                .collect();
            let mut output = vec![0.0; input.len() * factor];
            plan.process(&input, &mut output).unwrap();
            assert!(output.iter().all(|sample| sample.is_finite()));
            assert!(
                plan.history
                    .iter()
                    .all(|sample| sample.is_finite() && sample.abs() < STATE_LIMIT)
            );
            plan.reset(bandwidth);
            plan.process(&[0.0; 256], &mut output[..256 * factor])
                .unwrap();
            assert!(output[..256 * factor].iter().all(|&sample| sample == 0.0));
        }
    }

    #[test]
    fn invalid_frames_preserve_output_history_and_future_audio() {
        for (bandwidth, factor, _) in CASES {
            let mut plan = Resampler::new(bandwidth);
            let mut control = Resampler::new(bandwidth);
            let input = [0.25; 43];
            let mut warm = vec![0.0; input.len() * factor];
            plan.process(&input, &mut warm).unwrap();
            control.process(&input, &mut warm).unwrap();
            let history = plan.history;
            let cursor = plan.cursor;
            for invalid in [f32::NAN, f32::INFINITY, -f32::INFINITY, f32::MAX] {
                let mut output = vec![123.0; factor];
                assert_eq!(plan.process(&[invalid], &mut output), Err(Error::Argument));
                assert!(output.iter().all(|&sample| sample == 123.0));
            }
            let mut wrong = [123.0; 1];
            assert_eq!(plan.process(&input, &mut wrong), Err(Error::Output));
            assert_eq!(wrong, [123.0]);
            assert_eq!(plan.history, history);
            assert_eq!(plan.cursor, cursor);
            let mut actual = vec![0.0; warm.len()];
            let mut expected = vec![0.0; warm.len()];
            plan.process(&input, &mut actual).unwrap();
            control.process(&input, &mut expected).unwrap();
            assert_eq!(actual, expected);
            plan.process(&[], &mut []).unwrap();
        }
    }
}
