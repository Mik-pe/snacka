// SPDX-License-Identifier: MIT
//! Original causal antialias filter for the SILK encoder's native sample rate.

use super::{Bandwidth, Error};

const TAPS: usize = 177;
const HISTORY: usize = 256;
const LIMIT: f32 = f32::MAX / 16.0;

// BEGIN GENERATED MODEL
const COEFFICIENTS: [[f32; TAPS]; 3] = [
    [
        -2.4975701e-05,
        -6.080323e-05,
        -9.159477e-05,
        -0.00010447884,
        -8.861054e-05,
        -3.9521987e-05,
        3.7581496e-05,
        0.00012692666,
        0.00020451989,
        0.00024369864,
        0.00022281312,
        0.00013307168,
        -1.5857644e-05,
        -0.00019467114,
        -0.0003589211,
        -0.00045892835,
        -0.00045319303,
        -0.00032198962,
        -7.71913e-05,
        0.00023520985,
        0.00054189353,
        0.0007579572,
        0.0008086278,
        0.0006517618,
        0.00029491624,
        -0.0001986576,
        -0.00071848853,
        -0.0011296762,
        -0.0013058354,
        -0.0011646127,
        -0.0006964059,
        2.2524979e-05,
        0.00083830365,
        0.001549737,
        0.001955461,
        0.0019074755,
        0.0013575265,
        0.00038299095,
        -0.0008172616,
        -0.0019597095,
        -0.0027400032,
        -0.0029101854,
        -0.0023505348,
        -0.0011164628,
        0.0005548166,
        0.0022843094,
        0.0036329136,
        0.0042082164,
        0.0037714872,
        0.002317096,
        0.000100697245,
        -0.0023968176,
        -0.004567398,
        -0.005817538,
        -0.005721995,
        -0.0041514873,
        -0.0013389203,
        0.0021393634,
        0.005471725,
        0.007790982,
        0.00838808,
        0.006909527,
        0.0034872734,
        -0.0012387261,
        -0.0062225224,
        -0.010226966,
        -0.012110542,
        -0.011123648,
        -0.0071429885,
        -0.0007802903,
        0.0066748126,
        0.013482638,
        0.017817259,
        0.018196352,
        0.0138852075,
        0.0051801926,
        -0.0065044905,
        -0.018785883,
        -0.028687663,
        -0.03318238,
        -0.029800657,
        -0.017186219,
        0.004520997,
        0.033562765,
        0.06674181,
        0.09988332,
        0.12850203,
        0.14855446,
        0.15713549,
        0.15298644,
        0.136717,
        0.11069756,
        0.07864323,
        0.044969738,
        0.014044484,
        -0.010525809,
        -0.026440408,
        -0.03298846,
        -0.03103132,
        -0.022703331,
        -0.010904843,
        0.001302116,
        0.011272607,
        0.017222079,
        0.018458828,
        0.015378164,
        0.009241043,
        0.0018022036,
        -0.005121653,
        -0.010048377,
        -0.012112036,
        -0.011181615,
        -0.0078077056,
        -0.00302599,
        0.001925367,
        0.005903788,
        0.008108424,
        0.00822477,
        0.0064453986,
        0.0033742767,
        -0.00015241063,
        -0.0032802639,
        -0.005333506,
        -0.005951484,
        -0.005139508,
        -0.0032307156,
        -0.00077640143,
        0.0016029561,
        0.0033677341,
        0.0041754837,
        0.0039399127,
        0.002824546,
        0.0011790637,
        -0.0005610919,
        -0.0019847096,
        -0.0027977359,
        -0.0028791544,
        -0.002290574,
        -0.0012421184,
        -2.7244827e-05,
        0.0010548707,
        0.0017697659,
        0.0019951214,
        0.0017365229,
        0.0011113563,
        0.00030824004,
        -0.0004656027,
        -0.0010351498,
        -0.0012947031,
        -0.0012241172,
        -0.00088277704,
        -0.00038547377,
        0.00013181244,
        0.0005467631,
        0.0007777601,
        0.0007976227,
        0.0006324815,
        0.00034804456,
        2.829204e-05,
        -0.0002471878,
        -0.00042151462,
        -0.00047063595,
        -0.00040396382,
        -0.00025677483,
        -7.748869e-05,
        8.62382e-05,
        0.00019895853,
        0.00024394858,
        0.00022395344,
        0.00015692817,
        6.870395e-05,
        -1.5010595e-05,
        -7.510522e-05,
        -0.00010260044,
        -9.883712e-05,
        -7.288852e-05,
        -3.7414487e-05,
    ],
    [
        -6.8098416e-05,
        -6.8218375e-05,
        -2.0516361e-05,
        6.180578e-05,
        0.00013575934,
        0.00014852681,
        7.064206e-05,
        -7.674211e-05,
        -0.00022071316,
        -0.0002694367,
        -0.00016619153,
        6.52633e-05,
        0.0003153577,
        0.00043631077,
        0.00032473402,
        -4.735062e-06,
        -0.00040249966,
        -0.00064613787,
        -0.0005596379,
        -0.00012837777,
        0.00046070194,
        0.0008920905,
        0.00088488415,
        0.0003648268,
        -0.00045563883,
        -0.0011525837,
        -0.0013027403,
        -0.0007302273,
        0.00035136024,
        0.0014011702,
        0.0018130249,
        0.0012550994,
        -9.811097e-05,
        -0.0015916267,
        -0.00239597,
        -0.0019568722,
        -0.00035031067,
        0.0016738547,
        0.003026975,
        0.0028547514,
        0.0010530192,
        -0.001574989,
        -0.003655615,
        -0.0039479025,
        -0.0020608113,
        0.0012194356,
        0.004225134,
        0.005235888,
        0.003438749,
        -0.0005043281,
        -0.0046477863,
        -0.00669638,
        -0.0052474933,
        -0.00068381085,
        0.0048239334,
        0.008311658,
        0.0075789094,
        0.0025092233,
        -0.0046078223,
        -0.010051721,
        -0.010560652,
        -0.0052029523,
        0.003804266,
        0.011911412,
        0.014446003,
        0.009183803,
        -0.0020758829,
        -0.013920893,
        -0.01977008,
        -0.015314088,
        -0.0012491002,
        0.016255278,
        0.02793907,
        0.025878234,
        0.00799922,
        -0.019571705,
        -0.043793663,
        -0.04951652,
        -0.026007112,
        0.02758906,
        0.10040367,
        0.17274949,
        0.22288024,
        0.23498826,
        0.20518693,
        0.14299704,
        0.06771592,
        0.0011832793,
        -0.040075365,
        -0.050180633,
        -0.03473931,
        -0.007192731,
        0.017462242,
        0.028764373,
        0.02420237,
        0.008839749,
        -0.008220065,
        -0.018593779,
        -0.018348403,
        -0.009167918,
        0.0032383015,
        0.012395419,
        0.014258869,
        0.008847,
        -0.00027646916,
        -0.008167249,
        -0.011077712,
        -0.008151201,
        -0.001509021,
        0.0051328745,
        0.008477054,
        0.007228765,
        0.0025165149,
        -0.002933375,
        -0.006325627,
        -0.006194225,
        -0.0029886507,
        0.0013573717,
        0.0045512617,
        0.0051251757,
        0.0030798977,
        -0.00027545253,
        -0.00312121,
        -0.00409373,
        -0.0029175065,
        -0.0004220207,
        0.0019975165,
        0.0031429045,
        0.0025906833,
        0.0008150651,
        -0.0011540566,
        -0.0023124202,
        -0.002181468,
        -0.0009848056,
        0.0005494063,
        0.0016154988,
        0.0017435066,
        0.0009918734,
        -0.00014992745,
        -0.0010628592,
        -0.001325412,
        -0.00089785195,
        -8.96066e-05,
        0.0006444084,
        0.00095209,
        0.0007453363,
        0.00020503264,
        -0.00035002283,
        -0.0006437988,
        -0.0005743347,
        -0.00023671688,
        0.00015692714,
        0.0004032901,
        0.00040811743,
        0.00021408664,
        -4.548373e-05,
        -0.00023044553,
        -0.00026551506,
        -0.00016529168,
        -8.539426e-06,
        0.00011514817,
        0.00015313191,
        0.00010817545,
        2.4041208e-05,
        -4.70581e-05,
        -7.3765725e-05,
        -0.0,
        -0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        -0.0,
        -0.0,
        -0.0,
        -0.0,
        -0.0,
    ],
    [
        3.15248e-05,
        -4.0849638e-05,
        -0.00010068326,
        -7.3058065e-05,
        4.6062993e-05,
        0.00015860226,
        0.00013937092,
        -3.3815875e-05,
        -0.0002236913,
        -0.0002347104,
        -5.7715306e-06,
        0.00028878837,
        0.0003615863,
        8.393814e-05,
        -0.00034288917,
        -0.0005190332,
        -0.0002115671,
        0.00037205513,
        0.00070268323,
        0.00039924143,
        -0.00035836303,
        -0.00090238854,
        -0.0006542204,
        0.00028242663,
        0.0011036274,
        0.000981244,
        -0.00012192491,
        -0.0012844149,
        -0.0013780503,
        -0.000143773,
        0.0014188682,
        0.0018378225,
        0.00053614686,
        -0.0014731118,
        -0.002343113,
        -0.0010704888,
        0.0014116922,
        0.0028709208,
        0.0017604851,
        -0.0011918138,
        -0.0033847333,
        -0.0026086282,
        0.00077294884,
        0.003843023,
        0.0036140007,
        -0.00010824865,
        -0.0041886973,
        -0.0047600544,
        -0.0008428872,
        0.004361151,
        0.0060266675,
        0.002128083,
        -0.0042814855,
        -0.007375062,
        -0.0037878219,
        0.003866611,
        0.008764712,
        0.0058769174,
        -0.003005988,
        -0.01013461,
        -0.008453596,
        0.0015708874,
        0.011424283,
        0.0116197,
        0.00063268805,
        -0.012548509,
        -0.01553514,
        -0.0038938916,
        0.013416323,
        0.020532593,
        0.008757817,
        -0.013882366,
        -0.027319072,
        -0.016371213,
        0.01370929,
        0.0377444,
        0.029841628,
        -0.012259674,
        -0.058095742,
        -0.061345376,
        0.006641963,
        0.13079338,
        0.25489438,
        0.3140239,
        0.27570027,
        0.16106611,
        0.031065026,
        -0.052555785,
        -0.06406632,
        -0.023806147,
        0.022785565,
        0.039343692,
        0.020710401,
        -0.010632931,
        -0.02718571,
        -0.018446451,
        0.0040158103,
        0.019536458,
        0.016419604,
        3.4180604e-05,
        -0.014086432,
        -0.01445791,
        -0.0025948428,
        0.009957179,
        0.012536527,
        0.0041839983,
        -0.0067285537,
        -0.010656523,
        -0.005074459,
        0.004194489,
        0.008856386,
        0.0054582153,
        -0.0022163796,
        -0.007160203,
        -0.0054581156,
        0.00071503007,
        0.0056085805,
        0.00518709,
        0.00038754693,
        -0.004218651,
        -0.0047225263,
        -0.00114341,
        0.0030162784,
        0.0041443706,
        0.0016163243,
        -0.0020027892,
        -0.0035053138,
        -0.0018524517,
        0.0011851836,
        0.0028609692,
        0.001909075,
        -0.00054895703,
        -0.0022431228,
        -0.0018275034,
        8.492843e-05,
        0.0016849213,
        0.0016559848,
        0.00023225562,
        -0.0011992286,
        -0.0014271908,
        -0.0004215911,
        0.0007995167,
        0.001176558,
        0.00051218725,
        -0.00048361413,
        -0.0009251276,
        -0.0005255844,
        0.00025039812,
        0.00069397927,
        0.00048801454,
        -8.8381516e-05,
        -0.0004922845,
        -0.00041766078,
        -1.1770945e-05,
        0.000328247,
        0.00033351665,
        6.4758104e-05,
        -0.000201627,
        -0.00024719333,
        -8.210854e-05,
        0.000111508474,
        0.00016906769,
        7.682146e-05,
        -5.2228235e-05,
        -0.00010383279,
        -5.8345227e-05,
        1.8318844e-05,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
        0.0,
    ],
];
// END GENERATED MODEL

// Conversion from f32 to f64 is exact. Prepare the immutable coefficients
// once during compilation instead of converting all 177 taps at every output.
static WIDE_COEFFICIENTS: [[f64; TAPS]; 3] = {
    let mut result = [[0.0; TAPS]; 3];
    let mut band = 0;
    while band < 3 {
        let mut tap = 0;
        while tap < TAPS {
            result[band][tap] = COEFFICIENTS[band][tap] as f64;
            tap += 1;
        }
        band += 1;
    }
    result
};

/// Fixed-history 48 kHz to native SILK-rate encoder downsampler.
/// Coefficients are owned mathematical data, generated without a codec oracle.
pub(crate) struct Downsampler {
    coefficients: &'static [f64; TAPS],
    history: [f32; HISTORY],
    cursor: usize,
    factor: usize,
}

impl Downsampler {
    pub(crate) fn new(bandwidth: Bandwidth) -> Self {
        let (index, factor) = match bandwidth {
            Bandwidth::Narrow => (0, 6),
            Bandwidth::Medium => (1, 4),
            Bandwidth::Wide => (2, 3),
        };
        Self {
            coefficients: &WIDE_COEFFICIENTS[index],
            history: [0.0; HISTORY],
            cursor: 0,
            factor,
        }
    }

    pub(crate) fn reset(&mut self, bandwidth: Bandwidth) {
        *self = Self::new(bandwidth);
    }

    pub(crate) const fn ratio(&self) -> usize {
        self.factor
    }

    /// Restore chronological recent input when leaving native-rate bypass.
    /// Sampling intervals are whole native-rate blocks, so the next input is
    /// still the phase-zero sample. No output or convolution is required here.
    pub(crate) fn prime(&mut self, tail: &[f32]) {
        debug_assert!(tail.len() < HISTORY);
        self.history.fill(0.0);
        self.history[HISTORY - tail.len()..].copy_from_slice(tail);
        self.cursor = 0;
    }

    /// Fractional low-frequency group delay in 48 kHz input samples.
    #[cfg(test)]
    pub(crate) const fn delay_samples(&self) -> f64 {
        match self.factor {
            6 => 88.176,
            4 => 82.784,
            _ => 83.112,
        }
    }

    /// Consume complete sampling-factor blocks. Trailing output remains intact.
    /// Geometry and finite-domain errors precede every history/output change.
    /// Initialization, reset and processing do not allocate or execute trig.
    pub(crate) fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), Error> {
        if !input.len().is_multiple_of(self.factor)
            || input
                .iter()
                .any(|value| !value.is_finite() || value.abs() > LIMIT)
        {
            return Err(Error::Argument);
        }
        let samples = input.len() / self.factor;
        if output.len() < samples {
            return Err(Error::Output);
        }
        match self.factor {
            3 => self.run::<3>(input, &mut output[..samples]),
            4 => self.run::<4>(input, &mut output[..samples]),
            6 => self.run::<6>(input, &mut output[..samples]),
            _ => unreachable!("constructor selects native SILK rates"),
        }
        Ok(())
    }

    fn run<const FACTOR: usize>(&mut self, input: &[f32], output: &mut [f32]) {
        for (block, destination) in input.as_chunks::<FACTOR>().0.iter().zip(output) {
            self.history[self.cursor] = block[0];
            let head = TAPS.min(self.cursor + 1);
            let mut partial = [0.0f64; 4];
            dot_reverse(
                &self.history[..=self.cursor],
                &self.coefficients[..head],
                &mut partial,
            );
            dot_reverse(
                &self.history[self.cursor + 1..],
                &self.coefficients[head..],
                &mut partial,
            );
            *destination = (partial[0] + partial[1] + partial[2] + partial[3]) as f32;
            self.cursor = (self.cursor + 1) & (HISTORY - 1);
            for &sample in &block[1..] {
                self.history[self.cursor] = sample;
                self.cursor = (self.cursor + 1) & (HISTORY - 1);
            }
        }
    }
}

// Four independent sums avoid a dependency chain across every FIR tap. Both
// ring slices contain at least as many samples as their coefficient slice.
fn dot_reverse(history: &[f32], coefficients: &[f64], partial: &mut [f64; 4]) {
    let mut values = history.iter().rev();
    let (groups, tail) = coefficients.as_chunks::<4>();
    for group in groups {
        partial[0] += f64::from(*values.next().unwrap()) * group[0];
        partial[1] += f64::from(*values.next().unwrap()) * group[1];
        partial[2] += f64::from(*values.next().unwrap()) * group[2];
        partial[3] += f64::from(*values.next().unwrap()) * group[3];
    }
    for (&coefficient, &value) in tail.iter().zip(values) {
        partial[0] += f64::from(value) * coefficient;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chronological_history_priming_retains_all_native_sampling_phases() {
        let input: Vec<f32> = (0..3840)
            .map(|index| ((index * 73 % 257) as f32 - 128.0) / 256.0)
            .collect();
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide] {
            for prefix in [480, 960, 1920, 2880] {
                let mut continuous = Downsampler::new(bandwidth);
                let ratio = continuous.ratio();
                let mut discard = vec![0.0; prefix / ratio];
                continuous.process(&input[..prefix], &mut discard).unwrap();
                let mut primed = Downsampler::new(bandwidth);
                // A chronological FIR tail need not be a complete decimation
                // block: it restores prior samples, not the sampling clock.
                primed.prime(&input[prefix - TAPS..prefix]);
                let mut expected = vec![0.0; (input.len() - prefix) / ratio];
                let mut actual = expected.clone();
                continuous.process(&input[prefix..], &mut expected).unwrap();
                primed.process(&input[prefix..], &mut actual).unwrap();
                for (&a, &b) in actual.iter().zip(&expected) {
                    assert!((a - b).abs() < 1e-7, "{bandwidth:?}, {prefix}: {a} != {b}");
                }
            }
        }
    }

    #[test]
    fn literal_convolution_and_irregular_chunks_have_identical_history() {
        for (bandwidth, coefficients) in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide]
            .into_iter()
            .zip(&COEFFICIENTS)
        {
            let mut plan = Downsampler::new(bandwidth);
            let factor = plan.ratio();
            let input: Vec<f32> = (0..factor * 751)
                .map(|i| ((i * 37 % 103) as f32 - 51.0) / 64.0)
                .collect();
            let mut output = vec![0.0; 751];
            plan.process(&input, &mut output).unwrap();
            for (index, &sample) in output.iter().enumerate() {
                let at = factor * index;
                // Use the original mathematical f32 model, independently of
                // the cached f64 representation used by processing.
                let wanted = coefficients[..TAPS.min(at + 1)]
                    .iter()
                    .enumerate()
                    .map(|(lag, &coefficient)| f64::from(coefficient) * f64::from(input[at - lag]))
                    .sum::<f64>() as f32;
                assert_eq!(sample, wanted);
            }
            let mut streaming = Downsampler::new(bandwidth);
            let mut split = vec![0.0; 751];
            let mut cursor = 0;
            while cursor < split.len() {
                let end = (cursor + 1 + cursor % 71).min(split.len());
                streaming
                    .process(
                        &input[cursor * factor..end * factor],
                        &mut split[cursor..end],
                    )
                    .unwrap();
                cursor = end;
            }
            assert_eq!(output, split);
            streaming.reset(bandwidth);
            streaming.process(&input, &mut split).unwrap();
            assert_eq!(output, split);
        }
    }

    #[test]
    fn response_and_total_codec_delay_match_mathematical_design() {
        for (bandwidth, decoder_delay) in [
            (Bandwidth::Narrow, 25.824),
            (Bandwidth::Medium, 33.216),
            (Bandwidth::Wide, 33.888),
        ] {
            let plan = Downsampler::new(bandwidth);
            let mass = plan.coefficients.iter().sum::<f64>();
            let moment = plan
                .coefficients
                .iter()
                .enumerate()
                .map(|(i, &v)| i as f64 * v)
                .sum::<f64>()
                / mass;
            assert!((mass - 1.0).abs() < 2e-7);
            assert!((moment - plan.delay_samples()).abs() < 2e-5);
            assert!((moment + plan.ratio() as f64 + decoder_delay - 120.0).abs() < 2e-5);
            let gain = |frequency: f64| {
                let (real, imaginary) =
                    plan.coefficients
                        .iter()
                        .enumerate()
                        .fold((0.0, 0.0), |(r, j), (i, &value)| {
                            let angle = 2.0 * std::f64::consts::PI * frequency * i as f64;
                            (r + value * angle.cos(), j + value * angle.sin())
                        });
                real.hypot(imaginary)
            };
            assert!((gain(0.4 / plan.ratio() as f64) - 1.0).abs() < 0.015);
            assert!(gain(0.6 / plan.ratio() as f64) < 0.0015);
            assert!(
                plan.coefficients
                    .iter()
                    .map(|value| value.abs())
                    .sum::<f64>()
                    < 3.0
            );
        }
    }

    #[test]
    fn invalid_calls_preserve_output_and_future_state_extremes_stay_finite() {
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide] {
            let mut plan = Downsampler::new(bandwidth);
            let mut control = Downsampler::new(bandwidth);
            let factor = plan.ratio();
            let input = vec![0.125; factor * 25];
            let mut output = [2.0; 26];
            let mut wanted = [2.0; 26];
            assert_eq!(
                plan.process(&[f32::NAN; 6][..factor], &mut output),
                Err(Error::Argument)
            );
            assert_eq!(plan.process(&input, &mut output[..24]), Err(Error::Output));
            assert_eq!(output, [2.0; 26]);
            plan.process(&input, &mut output).unwrap();
            control.process(&input, &mut wanted).unwrap();
            assert_eq!(output, wanted);
            let extreme: Vec<_> = (0..factor * 10000)
                .map(|i| if i % 2 == 0 { LIMIT } else { -LIMIT })
                .collect();
            let mut result = vec![0.0; 10000];
            plan.process(&extreme, &mut result).unwrap();
            assert!(result.iter().all(|value| value.is_finite()));
        }
    }
}
