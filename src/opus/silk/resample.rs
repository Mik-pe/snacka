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

const HISTORY: usize = 32;
const TAPS: usize = 14;
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

pub(crate) struct Resampler {
    factor: usize,
    delay: usize,
    numerators: &'static [[f64; TAPS]; 6],
    history: [f64; HISTORY],
    cursor: usize,
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
            history: [0.0; HISTORY],
            cursor: 0,
        }
    }

    pub(crate) fn reset(&mut self, bandwidth: Bandwidth) {
        *self = Self::new(bandwidth);
    }

    pub(crate) fn ratio(&self) -> usize {
        self.factor
    }

    /// Produce exactly `input.len() * ratio()` samples. Invalid lengths or
    /// numeric inputs leave both output and streaming history unchanged.
    pub(crate) fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), Error> {
        if input.len().checked_mul(self.factor) != Some(output.len()) {
            return Err(Error::Output);
        }
        if input
            .iter()
            .any(|sample| !sample.is_finite() || sample.abs() > SAMPLE_LIMIT)
            || self
                .history
                .iter()
                .any(|sample| !sample.is_finite() || sample.abs() > STATE_LIMIT)
        {
            return Err(Error::Argument);
        }
        // All poles are real, negative and below 0.848 in magnitude. The
        // feedback transfer has l1 gain <25.64, and every complete phase has
        // l1 gain <3.12. SAMPLE_LIMIT therefore bounds states well inside f64
        // and output inside f32, even for alternating maximum-domain samples.
        match self.factor {
            3 => self.run::<3>(input, output),
            4 => self.run::<4>(input, output),
            6 => self.run::<6>(input, output),
            _ => unreachable!("constructor supports only SILK sampling rates"),
        }
        debug_assert!(output.iter().all(|sample| sample.is_finite()));
        Ok(())
    }

    fn run<const PHASES: usize>(&mut self, input: &[f32], output: &mut [f32]) {
        for (&sample, block) in input.iter().zip(output.as_chunks_mut::<PHASES>().0) {
            let mut filtered = f64::from(sample);
            for (lag, &coefficient) in FEEDBACK.iter().enumerate() {
                filtered -=
                    coefficient * self.history[(self.cursor + HISTORY - 1 - lag) & (HISTORY - 1)];
            }
            self.history[self.cursor] = filtered;
            let mut sums = [0.0f64; PHASES];
            let begin = (self.cursor + HISTORY - self.delay) & (HISTORY - 1);
            let head = TAPS.min(begin + 1);
            for (lag, &past) in self.history[..=begin].iter().rev().take(head).enumerate() {
                for (phase, sum) in sums.iter_mut().enumerate() {
                    *sum += past * self.numerators[phase][lag];
                }
            }
            for (extra, &past) in self.history[begin + 1..]
                .iter()
                .rev()
                .take(TAPS - head)
                .enumerate()
            {
                let lag = head + extra;
                for (phase, sum) in sums.iter_mut().enumerate() {
                    *sum += past * self.numerators[phase][lag];
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
