// SPDX-License-Identifier: MIT
//! Experimental standalone speech codec with a versioned native bitstream.

use crate::entropy;
use crate::transform::{MdctPlan, MdctWorkspace};
use std::fmt;

/// PCM samples per second for the native mono profile.
pub const SAMPLE_RATE: u32 = 48_000;
/// PCM samples in the default 20 ms frame, also the largest supported frame.
pub const FRAME_SAMPLES: usize = 960;
/// Maximum encoded bytes per frame at 32 kb/s, including framing and checksum.
pub const MAX_PACKET_BYTES: usize = 80;
/// Additional sample delay introduced by the transform pipeline.
pub const ALGORITHMIC_DELAY: usize = 120;

const HEADER: usize = 2;
const CHECKSUM: usize = 2;
const VERSION: u8 = 7;
const END: u8 = 16;
const MIN_PACKET_BYTES: usize = 15;
const GAIN_STEPS: [f32; 17] = [
    1f32,
    1.044_273_7_f32,
    1.090_507_7_f32,
    1.138_788_6_f32,
    1.189_207_1_f32,
    1.241_857_8_f32,
    1.296_839_6_f32,
    1.354_255_6_f32,
    std::f32::consts::SQRT_2,
    1.476_826_2_f32,
    1.542_210_8_f32,
    1.610_490_3_f32,
    1.681_792_9_f32,
    1.756_252_2_f32,
    1.834_008_1_f32,
    1.915_206_6_f32,
    2f32,
];
const SHAPE_BOUNDARIES: [f32; 7] = [
    1.0 / 98.0,
    5.0 / 98.0,
    13.0 / 98.0,
    25.0 / 98.0,
    41.0 / 98.0,
    61.0 / 98.0,
    85.0 / 98.0,
];

/// Invalid configuration, audio input, packet or stream state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Bitrate lies outside 12,000 through 32,000 bits per second.
    Config,
    /// Input has an incorrect frame length or is not finite normalized PCM in [-1, 1].
    Input,
    /// Output cannot hold the frame or has an incorrect PCM length.
    Output,
    /// Framing, checksum or spectral parameters are invalid.
    Packet,
    /// The bitstream version is unsupported.
    Version,
    /// The required decoding profile or model is unsupported.
    Profile,
    /// A frame is missing, duplicated or out of order.
    Sequence {
        /// Next sequence number required by this decoder.
        expected: u8,
        /// Sequence number carried by the input packet.
        actual: u8,
    },
    /// The stream has been finished; reset before starting another stream.
    Finished,
    /// Entropy input or output failed validation.
    Entropy(entropy::Error),
}

impl fmt::Display for Error {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Config => formatter.write_str("bitrate must be between 12000 and 32000 bits/s"),
            Self::Input => formatter.write_str("invalid normalized PCM frame"),
            Self::Output => formatter.write_str("invalid output capacity"),
            Self::Packet => formatter.write_str("invalid native codec packet"),
            Self::Version => formatter.write_str("unsupported bitstream version"),
            Self::Profile => formatter.write_str("unsupported decoding profile"),
            Self::Sequence { expected, actual } => {
                write!(formatter, "expected frame {expected}, received {actual}")
            }
            Self::Finished => formatter.write_str("codec stream is finished"),
            Self::Entropy(error) => error.fmt(formatter),
        }
    }
}
impl std::error::Error for Error {}
impl From<entropy::Error> for Error {
    fn from(error: entropy::Error) -> Self {
        Self::Entropy(error)
    }
}

/// Duration of one independently framed mono audio packet.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum FrameDuration {
    /// 10 ms: 480 samples, lower buffering delay and higher framing overhead.
    Ms10,
    /// 20 ms: 960 samples, the default for interactive speech.
    #[default]
    Ms20,
}
impl FrameDuration {
    /// PCM samples in each frame at 48 kHz.
    pub const fn samples(self) -> usize {
        match self {
            Self::Ms10 => 480,
            Self::Ms20 => 960,
        }
    }
    /// Frame duration in milliseconds.
    pub const fn milliseconds(self) -> u32 {
        match self {
            Self::Ms10 => 10,
            Self::Ms20 => 20,
        }
    }
    fn divisor(self) -> u32 {
        8_000 / self.milliseconds()
    }
    fn profile(self) -> u8 {
        match self {
            Self::Ms20 => 0,
            Self::Ms10 => 1,
        }
    }
}

/// Mono, 48 kHz encoding configuration.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    bitrate: u32,
    duration: FrameDuration,
    harmonic_speech: bool,
}
impl Config {
    /// Set the total packet bitrate, including framing and checksum overhead.
    ///
    /// Fractional byte budgets alternate between adjacent packet lengths, so
    /// any integer bitrate in the supported range is accepted.
    pub fn new(bitrate: u32) -> Result<Self, Error> {
        if !(12_000..=32_000).contains(&bitrate) {
            return Err(Error::Config);
        }
        Ok(Self {
            bitrate,
            duration: FrameDuration::default(),
            harmonic_speech: false,
        })
    }
    /// Select a frame duration before constructing the stream.
    pub fn with_frame_duration(mut self, duration: FrameDuration) -> Self {
        self.duration = duration;
        self
    }
    /// Enable experimental harmonic speech candidates at 12–16 kb/s.
    ///
    /// Disabled by default because the additional analysis increases CPU cost.
    /// Peers must support harmonic profiles 14 and 15 before enabling this.
    pub fn with_harmonic_speech(mut self, enabled: bool) -> Self {
        self.harmonic_speech = enabled;
        self
    }
    /// Whether experimental harmonic speech candidates are enabled.
    pub fn harmonic_speech(&self) -> bool {
        self.harmonic_speech
    }
    /// Duration selected for this stream.
    pub fn frame_duration(&self) -> FrameDuration {
        self.duration
    }
    /// Exact input and output PCM length for this stream.
    pub fn frame_samples(&self) -> usize {
        self.duration.samples()
    }
    /// Target bits per second, including all encoded packet bytes.
    pub fn bitrate(&self) -> u32 {
        self.bitrate
    }
    /// Output capacity sufficient for every packet at this bitrate.
    pub fn maximum_packet_bytes(&self) -> usize {
        self.bitrate.div_ceil(self.duration.divisor()) as usize
    }
}
impl Default for Config {
    fn default() -> Self {
        Self {
            bitrate: 24_000,
            duration: FrameDuration::default(),
            harmonic_speech: false,
        }
    }
}

const fn crc_table() -> [u16; 256] {
    let mut table = [0; 256];
    let mut index = 0;
    while index < 256 {
        let mut value = (index as u16) << 8;
        let mut bit = 0;
        while bit < 8 {
            value = if value & 0x8000 != 0 {
                (value << 1) ^ 0x1021
            } else {
                value << 1
            };
            bit += 1;
        }
        table[index] = value;
        index += 1;
    }
    table
}
const CRC_TABLE: [u16; 256] = crc_table();
fn checksum(bytes: &[u8]) -> u16 {
    bytes.iter().fold(0xffff, |state, &byte| {
        (state << 8) ^ CRC_TABLE[usize::from((state >> 8) as u8 ^ byte)]
    })
}

// ceil(log2(binomial(M, K))) for exact integer subset cardinalities.
const SUBSET_BITS: [[usize; 84]; 5] = [
    [
        0, 8, 14, 20, 25, 30, 35, 39, 44, 48, 52, 55, 59, 62, 66, 69, 72, 75, 78, 81, 84, 87, 89,
        92, 95, 97, 99, 102, 104, 106, 108, 110, 112, 114, 116, 118, 120, 122, 123, 125, 127, 128,
        130, 131, 132, 134, 135, 136, 138, 139, 140, 141, 142, 143, 144, 145, 146, 147, 148, 149,
        149, 150, 151, 151, 152, 152, 153, 153, 154, 154, 155, 155, 155, 156, 156, 156, 156, 156,
        156, 156, 157, 156, 156, 156,
    ],
    [
        0, 8, 15, 22, 28, 33, 38, 43, 48, 53, 58, 62, 66, 70, 74, 78, 82, 86, 89, 93, 96, 100, 103,
        106, 110, 113, 116, 119, 122, 124, 127, 130, 133, 135, 138, 141, 143, 146, 148, 150, 153,
        155, 157, 159, 162, 164, 166, 168, 170, 172, 174, 176, 177, 179, 181, 183, 185, 186, 188,
        190, 191, 193, 194, 196, 197, 199, 200, 201, 203, 204, 205, 207, 208, 209, 210, 211, 212,
        214, 215, 216, 217, 218, 219, 220,
    ],
    [
        0, 9, 16, 23, 29, 35, 41, 46, 52, 57, 62, 67, 71, 76, 80, 85, 89, 93, 97, 101, 105, 109,
        113, 116, 120, 123, 127, 130, 134, 137, 140, 144, 147, 150, 153, 156, 159, 162, 165, 168,
        171, 173, 176, 179, 181, 184, 187, 189, 192, 194, 197, 199, 201, 204, 206, 208, 210, 213,
        215, 217, 219, 221, 223, 225, 227, 229, 231, 233, 235, 237, 239, 241, 242, 244, 246, 248,
        249, 251, 253, 254, 256, 257, 259, 260,
    ],
    [
        0, 9, 17, 25, 32, 38, 44, 50, 56, 62, 68, 73, 78, 84, 89, 94, 98, 103, 108, 112, 117, 121,
        126, 130, 134, 139, 143, 147, 151, 155, 159, 163, 166, 170, 174, 177, 181, 185, 188, 192,
        195, 199, 202, 205, 209, 212, 215, 218, 222, 225, 228, 231, 234, 237, 240, 243, 246, 249,
        252, 254, 257, 260, 263, 265, 268, 271, 274, 276, 279, 281, 284, 286, 289, 291, 294, 296,
        299, 301, 303, 306, 308, 310, 313, 315,
    ],
    [
        0, 10, 19, 28, 36, 43, 50, 58, 64, 71, 78, 84, 90, 97, 103, 109, 115, 120, 126, 132, 137,
        143, 148, 154, 159, 164, 169, 174, 179, 184, 189, 194, 199, 204, 209, 213, 218, 223, 227,
        232, 236, 241, 245, 250, 254, 259, 263, 267, 271, 276, 280, 284, 288, 292, 296, 300, 304,
        308, 312, 316, 320, 324, 328, 332, 335, 339, 343, 347, 350, 354, 358, 361, 365, 369, 372,
        376, 379, 383, 386, 390, 393, 397, 400, 404,
    ],
];
fn coefficient_count(payload_bytes: usize, bins: usize, amplitude_bits: usize) -> usize {
    let table = match bins {
        160 => 0,
        240 => 1,
        320 => 2,
        480 => 3,
        960 => 4,
        _ => unreachable!("fixed spectral dimensions"),
    };
    let subset_bits = &SUBSET_BITS[table];
    let budget = payload_bytes * 8;
    // Gain and quantizer use nine raw bits; three bits cover interval rounding and termination.
    // Subset-plus-amplitude cost is monotone across these table entries.
    let mut start = 0;
    let mut end = subset_bits.len();
    while start < end {
        let middle = (start + end) / 2;
        if subset_bits[middle] + amplitude_bits * middle + 12 <= budget {
            start = middle + 1;
        } else {
            end = middle;
        }
    }
    start.saturating_sub(1)
}

/// Borrowed framing of one native codec packet, with checksum validation.
///
/// Parsing validates framing only; the audio decoder also validates entropy
/// and spectral parameters. A checksum detects corruption, not authenticity.
#[derive(Debug, Clone, Copy)]
pub struct Packet<'a> {
    bytes: &'a [u8],
}
impl<'a> Packet<'a> {
    /// Validate packet length, version, profile and CRC-16/CCITT-FALSE.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if !(MIN_PACKET_BYTES..=MAX_PACKET_BYTES).contains(&bytes.len()) {
            return Err(Error::Packet);
        }
        if bytes[0] >> 5 != VERSION {
            return Err(Error::Version);
        }
        if bytes[0] & 6 == 6 && bytes[0] & 8 == 0 {
            return Err(Error::Profile);
        }
        let duration = if bytes[0] & 1 == 0 {
            FrameDuration::Ms20
        } else {
            FrameDuration::Ms10
        };
        let divisor = duration.divisor();
        if !(12_000 / divisor..=32_000 / divisor).contains(&(bytes.len() as u32)) {
            return Err(Error::Packet);
        }
        let end = bytes.len() - CHECKSUM;
        if checksum(&bytes[..end]) != u16::from_le_bytes(bytes[end..].try_into().unwrap()) {
            return Err(Error::Packet);
        }
        Ok(Self { bytes })
    }
    /// Required frame duration encoded by the decoding profile.
    pub fn frame_duration(&self) -> FrameDuration {
        if self.bytes[0] & 1 == 0 {
            FrameDuration::Ms20
        } else {
            FrameDuration::Ms10
        }
    }
    /// Nominal coded bandwidth: 8,000, 12,000 or 24,000 Hz.
    ///
    /// Harmonic profiles report their upper harmonic frequency; their finite-window
    /// transform can carry energy above this frequency.
    ///
    /// PCM always uses 48 kHz. Bandwidth can change between packets without reset.
    pub fn bandwidth_hz(&self) -> u32 {
        if self.is_harmonic() {
            8000
        } else {
            24_000 / (1 + u32::from((self.bytes[0] & 6) >> 1))
        }
    }
    /// Number of coded gain bands: one scalar gain or four normalized pulse bands.
    ///
    /// Four gains cover 0–1, 1–2, 2–4 kHz and the remaining coded spectrum or
    /// harmonic parameters.
    pub fn gain_bands(&self) -> usize {
        if self.bytes[0] & 8 == 0 { 1 } else { 4 }
    }
    fn is_harmonic(&self) -> bool {
        self.bytes[0] & 14 == 14
    }
    fn coded_bins(&self) -> usize {
        if self.is_harmonic() {
            self.frame_duration().samples() / 3
        } else {
            self.frame_duration().samples() / (1 + usize::from((self.bytes[0] & 6) >> 1))
        }
    }
    /// Frame sequence number modulo 256.
    pub fn sequence(&self) -> u8 {
        self.bytes[1]
    }
    /// Whether this packet drains and finishes the stream.
    pub fn is_final(&self) -> bool {
        self.bytes[0] & END != 0
    }
    /// Encoded spectral payload excluding framing and checksum.
    pub fn payload(&self) -> &'a [u8] {
        &self.bytes[HEADER..self.bytes.len() - CHECKSUM]
    }
}

fn gain(code: u8) -> f32 {
    if code == 0 {
        return 0.0;
    }
    let exponent = if code <= 64 {
        2 * (i32::from(code) - 128)
    } else {
        i32::from(code) - 192
    };
    let scale = f32::from_bits(((exponent.div_euclid(16) + 127) as u32) << 23);
    scale * GAIN_STEPS[exponent.rem_euclid(16) as usize]
}
fn gain_code(peak: f32) -> u8 {
    if peak <= 1.0 / 65_536.0 {
        return 0;
    }
    let exponent = ((peak.to_bits() >> 23) & 255) as i32 - 127;
    let mantissa = f32::from_bits((peak.to_bits() & 0x7f_ffff) | (127 << 23));
    if exponent < -8 {
        let step = GAIN_STEPS
            .iter()
            .step_by(2)
            .take_while(|&&threshold| threshold < mantissa)
            .count();
        (128 + 8 * exponent + step as i32) as u8
    } else {
        let step = GAIN_STEPS.partition_point(|&threshold| threshold < mantissa);
        (192 + 16 * exponent + step as i32) as u8
    }
}

fn quantize(value: f32, scale: f32, bits: usize) -> i32 {
    if scale == 0.0 {
        return 0;
    }
    let normalized = value.abs() / scale;
    let magnitude = if bits == 4 {
        SHAPE_BOUNDARIES.partition_point(|&boundary| normalized >= boundary) as i32
    } else {
        (normalized * 31.0).round().min(31.0) as i32
    };
    if value < 0.0 { -magnitude } else { magnitude }
}
fn reconstruct(value: i32, scale: f32, bits: usize) -> f32 {
    if bits == 4 {
        (value * value.abs()) as f32 * (scale / 49.0)
    } else {
        value as f32 * (scale / 31.0)
    }
}

/// Stateful native audio encoder. Construction allocates transform storage.
///
/// All processing uses caller-owned buffers and performs no heap allocation.
/// The first packet primes the overlap; `finish` drains the final input frame.
pub struct Encoder {
    config: Config,
    plan: MdctPlan,
    workspace: MdctWorkspace,
    previous: [f32; ALGORITHMIC_DELAY],
    input: [f32; 2 * FRAME_SAMPLES],
    coefficients: [f32; FRAME_SAMPLES],
    sequence: u8,
    quota: u32,
    finished: bool,
}
impl Encoder {
    /// Allocate stream state for a validated configuration.
    pub fn new(config: Config) -> Self {
        let plan = MdctPlan::new(config.frame_samples(), ALGORITHMIC_DELAY)
            .expect("fixed transform dimensions");
        let workspace = MdctWorkspace::new(&plan);
        Self {
            config,
            plan,
            workspace,
            previous: [0.0; ALGORITHMIC_DELAY],
            input: [0.0; 2 * FRAME_SAMPLES],
            coefficients: [0.0; FRAME_SAMPLES],
            sequence: 0,
            quota: 0,
            finished: false,
        }
    }
    /// Change the total packet bitrate without resetting audio or sequence history.
    ///
    /// The fractional byte remainder is retained. The decoder needs no bitrate
    /// update because each packet length determines its own spectral budget.
    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<(), Error> {
        let config = Config::new(bitrate)?;
        self.config.bitrate = config.bitrate;
        Ok(())
    }
    /// Encode one configured frame of normalized PCM.
    ///
    /// Invalid input or insufficient output capacity preserves output and
    /// stream history. Unused caller output remains unchanged.
    pub fn encode(&mut self, input: &[f32], output: &mut [u8]) -> Result<usize, Error> {
        self.encode_frame(input, output, false)
    }
    /// Emit one drain packet and finish the stream.
    ///
    /// The drain packet produces `ALGORITHMIC_DELAY` valid samples. Reset
    /// both sides before starting another stream.
    pub fn finish(&mut self, output: &mut [u8]) -> Result<usize, Error> {
        self.encode_frame(
            &[0.0; FRAME_SAMPLES][..self.config.frame_samples()],
            output,
            true,
        )
    }
    /// Reset history, byte budget and sequence without allocating.
    pub fn reset(&mut self) {
        self.previous.fill(0.0);
        self.sequence = 0;
        self.quota = 0;
        self.finished = false;
    }
    fn encode_frame(
        &mut self,
        input: &[f32],
        output: &mut [u8],
        ending: bool,
    ) -> Result<usize, Error> {
        if self.finished {
            return Err(Error::Finished);
        }
        if input.len() != self.config.frame_samples()
            || input
                .iter()
                .any(|value| !value.is_finite() || value.abs() > 1.0)
        {
            return Err(Error::Input);
        }
        let quota = self.quota + self.config.bitrate;
        let length = (quota / self.config.duration.divisor()) as usize;
        if output.len() < length {
            return Err(Error::Output);
        }
        let samples = self.config.frame_samples();
        let start = (samples - ALGORITHMIC_DELAY) / 2;
        self.input[start..start + ALGORITHMIC_DELAY].copy_from_slice(&self.previous);
        self.input[start + ALGORITHMIC_DELAY..start + ALGORITHMIC_DELAY + samples]
            .copy_from_slice(input);
        self.plan
            .forward(
                &self.input[..2 * samples],
                &mut self.coefficients[..samples],
                &mut self.workspace,
            )
            .map_err(|_| Error::Input)?;
        let payload_bytes = length - HEADER - CHECKSUM;
        let mut indices: [usize; FRAME_SAMPLES] = std::array::from_fn(|index| index);
        let mut selected_indices = [0usize; 84];
        let mut best_improvement = f32::NEG_INFINITY;
        let mut code = 0;
        let mut bits = 4;
        let mut count = 0;
        let mut band = 0;
        let mut pulse_mode = false;
        let mut pulse_codes = [0u8; 4];
        let mut pulse_allocation = [0usize; 4];
        let mut best_pulses = [0i16; FRAME_SAMPLES];
        for candidate_band in 0..3 {
            let coded_bins = samples / (1 + candidate_band);
            let counts = [
                coefficient_count(payload_bytes, coded_bins, 4),
                coefficient_count(payload_bytes, coded_bins, 6),
            ];
            let maximum_count = counts[0].max(counts[1]);
            for (index, value) in indices[..coded_bins].iter_mut().enumerate() {
                *value = index;
            }
            let order = |&left: &usize, &right: &usize| {
                self.coefficients[right]
                    .abs()
                    .total_cmp(&self.coefficients[left].abs())
                    .then(left.cmp(&right))
            };
            indices[..coded_bins].select_nth_unstable_by(maximum_count, order);
            indices[..maximum_count].sort_unstable_by(order);
            let ceiling = gain_code(self.coefficients[indices[0]].abs());
            for candidate_gain in ceiling.saturating_sub(1).max(u8::from(ceiling != 0))..=ceiling {
                let scale = gain(candidate_gain);
                for (candidate_bits, candidate_count) in [(4, counts[0]), (6, counts[1])] {
                    let mut improvement = 0.0;
                    for &index in &indices[..candidate_count] {
                        let value = self.coefficients[index];
                        let decoded = reconstruct(
                            quantize(value, scale, candidate_bits),
                            scale,
                            candidate_bits,
                        );
                        improvement += value * value - (value - decoded).powi(2);
                    }
                    if improvement > best_improvement {
                        best_improvement = improvement;
                        code = candidate_gain;
                        bits = candidate_bits;
                        count = candidate_count;
                        band = candidate_band;
                        selected_indices[..count].copy_from_slice(&indices[..count]);
                    }
                }
            }
        }
        let gain_bits = crate::pulse::gain_bits(samples, payload_bytes);
        let gain_step = 1u8 << (8 - gain_bits);
        let quantize_gain = |start: usize, end: usize| {
            let energy: f32 = self.coefficients[start..end]
                .iter()
                .map(|value| value * value)
                .sum();
            let amplitude = energy.sqrt().min(2.0);
            let mask = gain_step - 1;
            let ceiling = (gain_code(amplitude) + mask) & !mask;
            let lower = ceiling
                .saturating_sub(gain_step)
                .max(u8::from(ceiling != 0) * gain_step);
            if (amplitude - gain(lower)).abs() <= (amplitude - gain(ceiling)).abs() {
                lower
            } else {
                ceiling
            }
        };
        // The first three bands share inputs and gains across bandwidths; only their pulse counts can change.
        let fixed_edges = crate::pulse::edges(samples, samples);
        let fixed_codes: [u8; 3] =
            std::array::from_fn(|group| quantize_gain(fixed_edges[group], fixed_edges[group + 1]));
        let mut last_pulses = [usize::MAX; 3];
        let mut cached_norms = [0.0; 3];
        let mut candidate_pulses = [0i16; FRAME_SAMPLES];
        for candidate_band in 0..3 {
            let coded_bins = samples / (1 + candidate_band);
            let edges = crate::pulse::edges(samples, coded_bins);
            let codes = [
                fixed_codes[0],
                fixed_codes[1],
                fixed_codes[2],
                quantize_gain(edges[3], edges[4]),
            ];
            let allocation = crate::pulse::allocate(samples, coded_bins, &codes, payload_bytes);
            let mut improvement = 0.0;
            for group in 0..4 {
                let range = edges[group]..edges[group + 1];
                let values = &self.coefficients[range.clone()];
                let shape = &mut candidate_pulses[range.clone()];
                let norm = if group < 3 && last_pulses[group] == allocation[group] {
                    cached_norms[group]
                } else {
                    let norm = crate::pulse::quantize(
                        values,
                        allocation[group],
                        shape,
                        &mut indices[range],
                    );
                    if group < 3 {
                        last_pulses[group] = allocation[group];
                        cached_norms[group] = norm;
                    }
                    norm
                };
                let scale = if norm == 0.0 {
                    0.0
                } else {
                    gain(codes[group]) / norm
                };
                for (&value, &pulse) in values.iter().zip(shape.iter()) {
                    let decoded = f32::from(pulse) * scale;
                    improvement += value * value - (value - decoded).powi(2);
                }
            }
            if improvement > best_improvement {
                best_improvement = improvement;
                band = candidate_band;
                pulse_mode = true;
                pulse_codes = codes;
                pulse_allocation = allocation;
                best_pulses = candidate_pulses;
            }
        }
        let mut harmonic_mode = false;
        let mut harmonic_f0 = 0usize;
        let mut harmonic_dimensions = [0usize; 4];
        let mut harmonic_edges = [0usize; 5];
        if self.config.harmonic_speech && self.config.bitrate <= 16000 {
            let mut positions = [usize::MAX; FRAME_SAMPLES];
            let mut compact_values = [0.0f32; FRAME_SAMPLES];
            let mut compact_shape = [0i16; FRAME_SAMPLES];
            let finalists = crate::harmonic::candidates(
                &self.input[start..start + samples + ALGORITHMIC_DELAY],
            );
            let mut original_harmonic = (f32::NEG_INFINITY, 0usize);
            #[cfg(test)]
            crate::harmonic_joint::reset_trials();
            for joint in [false, true] {
                let trials = if joint {
                    usize::from(original_harmonic.1 != 0)
                } else {
                    finalists.len()
                };
                for &(_, finalist_f0) in &finalists[..trials] {
                    let f0 = if joint {
                        original_harmonic.1
                    } else {
                        finalist_f0
                    };
                    if f0 == 0 {
                        continue;
                    }
                    let (dimensions, edges) = crate::harmonic::layout(f0, &mut positions);
                    crate::harmonic::analyze(
                        &self.input[start..start + samples + ALGORITHMIC_DELAY],
                        f0,
                        &positions,
                        &mut compact_values,
                    );
                    #[cfg(test)]
                    if joint {
                        crate::harmonic_joint::attempted_trial(f0);
                    }
                    if joint
                        && !crate::harmonic_joint::correct(
                            &self.input[start..start + samples + ALGORITHMIC_DELAY],
                            f0,
                            &positions,
                            self.plan.window_view(),
                            &mut compact_values,
                        )
                    {
                        continue;
                    }
                    let codes: [u8; 4] = std::array::from_fn(|group| {
                        let amplitude = compact_values[edges[group]..edges[group + 1]]
                            .iter()
                            .map(|x| x * x)
                            .sum::<f32>()
                            .sqrt()
                            .min(2.0);
                        let mask = gain_step - 1;
                        let ceiling = (gain_code(amplitude) + mask) & !mask;
                        let lower = ceiling
                            .saturating_sub(gain_step)
                            .max(u8::from(ceiling != 0) * gain_step);
                        if (amplitude - gain(lower)).abs() <= (amplitude - gain(ceiling)).abs() {
                            lower
                        } else {
                            ceiling
                        }
                    });
                    let allocation = crate::pulse::allocate_dimensions(
                        dimensions,
                        &codes,
                        payload_bytes * 8 - 9,
                        4 * gain_bits as usize,
                    );
                    let mut decoded_values = [0.0f32; FRAME_SAMPLES];
                    for group in 0..4 {
                        let range = edges[group]..edges[group + 1];
                        let norm = crate::pulse::quantize(
                            &compact_values[range.clone()],
                            allocation[group],
                            &mut compact_shape[range.clone()],
                            &mut indices[range.clone()],
                        );
                        let scale = if norm == 0.0 {
                            0.0
                        } else {
                            gain(codes[group]) / norm
                        };
                        for index in range {
                            decoded_values[index] = f32::from(compact_shape[index]) * scale;
                        }
                    }
                    let mut waveform = [0.0f32; 2 * FRAME_SAMPLES];
                    let mut decoded_coefficients = [0.0f32; FRAME_SAMPLES];
                    crate::harmonic::synthesize(
                        &mut waveform,
                        samples,
                        f0,
                        &positions,
                        &decoded_values,
                    );
                    self.plan
                        .forward(
                            &waveform[..2 * samples],
                            &mut decoded_coefficients[..samples],
                            &mut self.workspace,
                        )
                        .map_err(|_| Error::Input)?;
                    let improvement: f32 = self.coefficients[..samples]
                        .iter()
                        .zip(&decoded_coefficients[..samples])
                        .map(|(&x, &y)| x * x - (x - y).powi(2))
                        .sum();
                    if !joint {
                        #[cfg(test)]
                        crate::harmonic_joint::original_trial(f0, improvement);
                        if improvement > original_harmonic.0 {
                            original_harmonic = (improvement, f0);
                        }
                    }
                    if improvement > best_improvement {
                        best_improvement = improvement;
                        harmonic_mode = true;
                        harmonic_f0 = f0;
                        harmonic_dimensions = dimensions;
                        harmonic_edges = edges;
                        pulse_codes = codes;
                        pulse_allocation = allocation;
                        best_pulses = compact_shape;
                    }
                }
                #[cfg(test)]
                crate::harmonic_joint::selection_score(joint, best_improvement);
            }
        }
        let coded_bins = samples / (1 + band);
        let scale = gain(code);
        selected_indices[..count].sort_unstable();
        let mut packet = [0; MAX_PACKET_BYTES];
        packet[0] = (VERSION << 5)
            | if harmonic_mode { 14 } else { (band as u8) << 1 }
            | self.config.duration.profile()
            | if pulse_mode { 8 } else { 0 }
            | if ending { END } else { 0 };
        packet[1] = self.sequence;
        let mut writer = entropy::Encoder::new(&mut packet[HEADER..length - CHECKSUM]);
        if harmonic_mode {
            writer.raw((harmonic_f0 - crate::harmonic::MIN_F0) as u32, 9)?;
            for code in pulse_codes {
                writer.raw(u32::from(code / gain_step), gain_bits)?;
            }
            for group in 0..4 {
                let range =
                    harmonic_edges[group]..harmonic_edges[group] + harmonic_dimensions[group];
                crate::pulse::encode(&mut writer, &best_pulses[range], pulse_allocation[group])?;
            }
        } else if pulse_mode {
            for code in pulse_codes {
                writer.raw(u32::from(code / gain_step), gain_bits)?;
            }
            let edges = crate::pulse::edges(samples, coded_bins);
            for group in 0..4 {
                crate::pulse::encode(
                    &mut writer,
                    &best_pulses[edges[group]..edges[group + 1]],
                    pulse_allocation[group],
                )?;
            }
        } else {
            writer.raw(u32::from(code), 8)?;
            writer.raw(u32::from(bits == 6), 1)?;
            let mut selected = 0;
            for index in 0..coded_bins {
                let remaining = count - selected;
                if remaining == 0 {
                    break;
                }
                let available = coded_bins - index;
                let present = selected_indices[selected] == index;
                if remaining != available {
                    let probabilities = [0, (available - remaining) as u16, available as u16];
                    writer.symbol(usize::from(present), entropy::Model::new(&probabilities)?)?;
                }
                if present {
                    let radius = (1 << (bits - 1)) - 1;
                    writer.raw(
                        (quantize(self.coefficients[index], scale, bits) + radius) as u32,
                        bits as u32,
                    )?;
                    selected += 1;
                }
            }
        }
        writer.finish_padded()?;
        let crc = checksum(&packet[..length - CHECKSUM]);
        packet[length - CHECKSUM..length].copy_from_slice(&crc.to_le_bytes());
        output[..length].copy_from_slice(&packet[..length]);
        self.previous
            .copy_from_slice(&input[samples - ALGORITHMIC_DELAY..]);
        self.quota = quota % self.config.duration.divisor();
        self.sequence = self.sequence.wrapping_add(1);
        self.finished = ending;
        Ok(length)
    }
}

/// Result of decoding or concealing one configured frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameInfo {
    /// Decoded or concealed frame sequence modulo 256.
    pub sequence: u8,
    /// Encoded bytes consumed; zero for concealment.
    pub bytes: usize,
    /// Whether this frame was synthesized for a missing packet.
    pub concealed: bool,
    /// Whether the stream has finished after this frame.
    pub finished: bool,
    /// Valid output prefix; the final drain contains only the delayed tail.
    pub valid_samples: usize,
}

/// Stateful native audio decoder with causal waveform concealment.
///
/// Discard the first `ALGORITHMIC_DELAY` samples once per stream. Decode the
/// final drain packet and retain only its valid prefix to recover the input
/// length. Construction allocates; decoding, concealment and reset do not.
pub struct Decoder {
    duration: FrameDuration,
    neural_postfilter: bool,
    plan: MdctPlan,
    workspace: MdctWorkspace,
    overlap: [f32; ALGORITHMIC_DELAY],
    concealment: crate::concealment::Concealment,
    block: [f32; 2 * FRAME_SAMPLES],
    sequence: u8,
    finished: bool,
}
impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}
impl Decoder {
    /// Allocate an independent mono decoding stream.
    pub fn new() -> Self {
        Self::with_frame_duration(FrameDuration::default())
    }
    /// Allocate an independent stream for the negotiated frame duration.
    pub fn with_frame_duration(duration: FrameDuration) -> Self {
        let plan = MdctPlan::new(duration.samples(), ALGORITHMIC_DELAY)
            .expect("fixed transform dimensions");
        let workspace = MdctWorkspace::new(&plan);
        Self {
            duration,
            neural_postfilter: true,
            plan,
            workspace,
            overlap: [0.0; ALGORITHMIC_DELAY],
            concealment: crate::concealment::Concealment::new(),
            block: [0.0; 2 * FRAME_SAMPLES],
            sequence: 0,
            finished: false,
        }
    }
    /// Enable or disable the neural MDCT refinement and denoiser postfilter.
    pub const fn with_neural_postfilter(mut self, enabled: bool) -> Self {
        self.neural_postfilter = enabled;
        self
    }
    /// Whether the neural MDCT refinement and denoiser postfilter is enabled.
    pub const fn neural_postfilter(&self) -> bool {
        self.neural_postfilter
    }
    /// Decode one packet to the configured number of normalized PCM samples.
    ///
    /// Invalid packets, sequence mismatches and incorrect output lengths leave
    /// output and persistent history unchanged. Conceal each known missing frame
    /// before decoding the next packet, or reset for a new stream.
    pub fn decode(&mut self, bytes: &[u8], output: &mut [f32]) -> Result<FrameInfo, Error> {
        if self.finished {
            return Err(Error::Finished);
        }
        if output.len() != self.duration.samples() {
            return Err(Error::Output);
        }
        let packet = Packet::parse(bytes)?;
        if packet.frame_duration() != self.duration {
            return Err(Error::Profile);
        }
        if packet.sequence() != self.sequence {
            return Err(Error::Sequence {
                expected: self.sequence,
                actual: packet.sequence(),
            });
        }
        let payload = packet.payload();
        let mut reader = entropy::Decoder::new(payload);
        let mut coefficients = [0.0; FRAME_SAMPLES];
        let coded_bins = packet.coded_bins();
        let mut neural_meta = None;
        if packet.is_harmonic() {
            let f0 = reader.raw(9)? as usize + crate::harmonic::MIN_F0;
            if f0 > crate::harmonic::MAX_F0 {
                return Err(Error::Packet);
            }
            let samples = self.duration.samples();
            let gain_bits = crate::pulse::gain_bits(samples, payload.len());
            let gain_step = 1u8 << (8 - gain_bits);
            let mut codes = [0u8; 4];
            for code in &mut codes {
                *code = reader.raw(gain_bits)? as u8 * gain_step;
                if *code > 208 {
                    return Err(Error::Packet);
                }
            }
            let mut positions = [usize::MAX; FRAME_SAMPLES];
            let (dimensions, edges) = crate::harmonic::layout(f0, &mut positions);
            let allocation = crate::pulse::allocate_dimensions(
                dimensions,
                &codes,
                payload.len() * 8 - 9,
                4 * gain_bits as usize,
            );
            let mut shape = [0i16; FRAME_SAMPLES];
            let mut decoded_values = [0.0f32; FRAME_SAMPLES];
            for group in 0..4 {
                let range = edges[group]..edges[group + 1];
                let norm = crate::pulse::decode(
                    &mut reader,
                    &mut shape[range.clone()],
                    allocation[group],
                )?;
                let scale = if norm == 0.0 {
                    0.0
                } else {
                    gain(codes[group]) / norm
                };
                for index in range {
                    if positions[index] == usize::MAX {
                        if shape[index] != 0 {
                            return Err(Error::Packet);
                        }
                    } else {
                        decoded_values[index] = f32::from(shape[index]) * scale;
                    }
                }
            }
            let mut waveform = [0.0f32; 2 * FRAME_SAMPLES];
            crate::harmonic::synthesize(&mut waveform, samples, f0, &positions, &decoded_values);
            self.plan
                .forward(
                    &waveform[..2 * samples],
                    &mut coefficients[..samples],
                    &mut self.workspace,
                )
                .map_err(|_| Error::Packet)?;
            neural_meta = Some(crate::neural::FrameMetadata {
                samples,
                coded_bins,
                payload_bytes: payload.len(),
                is_harmonic: true,
                f0,
                codes,
                allocation,
            });
        } else if packet.gain_bands() == 4 {
            let mut codes = [0u8; 4];
            let gain_bits = crate::pulse::gain_bits(self.duration.samples(), payload.len());
            let gain_step = 1u8 << (8 - gain_bits);
            for code in &mut codes {
                *code = reader.raw(gain_bits)? as u8 * gain_step;
                if *code > 208 {
                    return Err(Error::Packet);
                }
            }
            let samples = self.duration.samples();
            let allocation = crate::pulse::allocate(samples, coded_bins, &codes, payload.len());
            let edges = crate::pulse::edges(samples, coded_bins);
            let mut shape = [0i16; FRAME_SAMPLES];
            for group in 0..4 {
                let range = edges[group]..edges[group + 1];
                let norm = crate::pulse::decode(
                    &mut reader,
                    &mut shape[range.clone()],
                    allocation[group],
                )?;
                let scale = if norm == 0.0 {
                    0.0
                } else {
                    gain(codes[group]) / norm
                };
                for (value, &pulse) in coefficients[range.clone()].iter_mut().zip(&shape[range]) {
                    *value = f32::from(pulse) * scale;
                }
            }
            neural_meta = Some(crate::neural::FrameMetadata {
                samples,
                coded_bins,
                payload_bytes: payload.len(),
                is_harmonic: false,
                f0: 0,
                codes,
                allocation,
            });
        } else {
            let code = reader.raw(8)? as u8;
            if code > 208 {
                return Err(Error::Packet);
            }
            let bits = 4 + 2 * reader.raw(1)? as usize;
            let scale = gain(code);
            let mut remaining = coefficient_count(payload.len(), coded_bins, bits);
            for (index, coefficient) in coefficients[..coded_bins].iter_mut().enumerate() {
                if remaining == 0 {
                    break;
                }
                let available = coded_bins - index;
                let present = if remaining == available {
                    true
                } else {
                    let probabilities = [0, (available - remaining) as u16, available as u16];
                    reader.symbol(entropy::Model::new(&probabilities)?) != 0
                };
                if present {
                    let value = reader.raw(bits as u32)?;
                    let radius = (1 << (bits - 1)) - 1;
                    if value == (1 << bits) - 1 || (code == 0 && value != radius as u32) {
                        return Err(Error::Packet);
                    }
                    let quantized = value as i32 - radius;
                    *coefficient = reconstruct(quantized, scale, bits);
                    remaining -= 1;
                }
            }
        }
        if reader.tell() > (payload.len() * 8) as u64 {
            return Err(Error::Packet);
        }
        if self.neural_postfilter
            && self.sequence != 0
            && let Some(meta) = &neural_meta
        {
            crate::neural::refine_spectrum(&mut coefficients[..self.duration.samples()], meta);
        }
        self.synthesize(&coefficients, output)?;
        self.concealment.observe(output);
        self.sequence = self.sequence.wrapping_add(1);
        self.finished = packet.is_final();
        Ok(FrameInfo {
            sequence: packet.sequence(),
            bytes: bytes.len(),
            concealed: false,
            finished: self.finished,
            valid_samples: if self.finished {
                ALGORITHMIC_DELAY
            } else {
                self.duration.samples()
            },
        })
    }
    /// Extract unrefined decoded MDCT coefficients and frame features for neural training.
    #[doc(hidden)]
    #[allow(clippy::too_many_arguments)]
    pub fn decode_unrefined_training_frame(
        &mut self,
        bytes: &[u8],
        coefficients: &mut [f32; FRAME_SAMPLES],
        global_features: &mut [f32; 64],
        group_sigma: &mut [f32; 4],
        edges: &mut [usize; 5],
        codes_out: &mut [u8; 4],
        is_harmonic_out: &mut bool,
    ) -> Result<usize, Error> {
        let packet = Packet::parse(bytes)?;
        if packet.frame_duration() != self.duration {
            return Err(Error::Profile);
        }
        let payload = packet.payload();
        let mut reader = entropy::Decoder::new(payload);
        coefficients.fill(0.0);
        let coded_bins = packet.coded_bins();
        let samples = self.duration.samples();
        let meta = if packet.is_harmonic() {
            let f0 = reader.raw(9)? as usize + crate::harmonic::MIN_F0;
            if f0 > crate::harmonic::MAX_F0 {
                return Err(Error::Packet);
            }
            let gain_bits = crate::pulse::gain_bits(samples, payload.len());
            let gain_step = 1u8 << (8 - gain_bits);
            let mut codes = [0u8; 4];
            for code in &mut codes {
                *code = reader.raw(gain_bits)? as u8 * gain_step;
                if *code > 208 {
                    return Err(Error::Packet);
                }
            }
            let mut positions = [usize::MAX; FRAME_SAMPLES];
            let (dimensions, h_edges) = crate::harmonic::layout(f0, &mut positions);
            let allocation = crate::pulse::allocate_dimensions(
                dimensions,
                &codes,
                payload.len() * 8 - 9,
                4 * gain_bits as usize,
            );
            let mut shape = [0i16; FRAME_SAMPLES];
            let mut decoded_values = [0.0f32; FRAME_SAMPLES];
            for group in 0..4 {
                let range = h_edges[group]..h_edges[group + 1];
                let norm = crate::pulse::decode(
                    &mut reader,
                    &mut shape[range.clone()],
                    allocation[group],
                )?;
                let scale = if norm == 0.0 {
                    0.0
                } else {
                    gain(codes[group]) / norm
                };
                for index in range {
                    if positions[index] == usize::MAX {
                        if shape[index] != 0 {
                            return Err(Error::Packet);
                        }
                    } else {
                        decoded_values[index] = f32::from(shape[index]) * scale;
                    }
                }
            }
            let mut waveform = [0.0f32; 2 * FRAME_SAMPLES];
            crate::harmonic::synthesize(&mut waveform, samples, f0, &positions, &decoded_values);
            self.plan
                .forward(
                    &waveform[..2 * samples],
                    &mut coefficients[..samples],
                    &mut self.workspace,
                )
                .map_err(|_| Error::Packet)?;
            crate::neural::FrameMetadata {
                samples,
                coded_bins,
                payload_bytes: payload.len(),
                is_harmonic: true,
                f0,
                codes,
                allocation,
            }
        } else if packet.gain_bands() == 4 {
            let mut codes = [0u8; 4];
            let gain_bits = crate::pulse::gain_bits(samples, payload.len());
            let gain_step = 1u8 << (8 - gain_bits);
            for code in &mut codes {
                *code = reader.raw(gain_bits)? as u8 * gain_step;
                if *code > 208 {
                    return Err(Error::Packet);
                }
            }
            let allocation = crate::pulse::allocate(samples, coded_bins, &codes, payload.len());
            let p_edges = crate::pulse::edges(samples, coded_bins);
            let mut shape = [0i16; FRAME_SAMPLES];
            for group in 0..4 {
                let range = p_edges[group]..p_edges[group + 1];
                let norm = crate::pulse::decode(
                    &mut reader,
                    &mut shape[range.clone()],
                    allocation[group],
                )?;
                let scale = if norm == 0.0 {
                    0.0
                } else {
                    gain(codes[group]) / norm
                };
                for (value, &pulse) in coefficients[range.clone()].iter_mut().zip(&shape[range]) {
                    *value = f32::from(pulse) * scale;
                }
            }
            crate::neural::FrameMetadata {
                samples,
                coded_bins,
                payload_bytes: payload.len(),
                is_harmonic: false,
                f0: 0,
                codes,
                allocation,
            }
        } else {
            self.sequence = self.sequence.wrapping_add(1);
            self.finished = packet.is_final();
            return Ok(0);
        };
        if reader.tell() > (payload.len() * 8) as u64 {
            return Err(Error::Packet);
        }
        *edges = crate::pulse::edges(samples, coded_bins);
        *codes_out = meta.codes;
        *is_harmonic_out = meta.is_harmonic;
        crate::neural::compute_frame_features(
            &coefficients[..samples],
            &meta,
            edges,
            group_sigma,
            global_features,
        );
        self.sequence = self.sequence.wrapping_add(1);
        self.finished = packet.is_final();
        Ok(coded_bins)
    }
    /// Synthesize exactly one missing frame and advance the sequence.
    ///
    /// Repeated losses extrapolate recent PCM with a bounded fade to silence.
    pub fn conceal(&mut self, output: &mut [f32]) -> Result<FrameInfo, Error> {
        if self.finished {
            return Err(Error::Finished);
        }
        if output.len() != self.duration.samples() {
            return Err(Error::Output);
        }
        self.concealment.begin();
        let samples = self.duration.samples();
        let start = (samples - ALGORITHMIC_DELAY) / 2;
        self.block.fill(0.0);
        // Predict the overlap tail too, without waiting for another input frame.
        for (ahead, value) in self.block[start..start + samples + ALGORITHMIC_DELAY]
            .iter_mut()
            .enumerate()
        {
            *value = self.concealment.sample(ahead);
        }
        let mut coefficients = [0.0; FRAME_SAMPLES];
        self.plan
            .forward(
                &self.block[..2 * samples],
                &mut coefficients[..samples],
                &mut self.workspace,
            )
            .map_err(|_| Error::Packet)?;
        self.synthesize(&coefficients, output)?;
        self.concealment.advance(samples);
        let sequence = self.sequence;
        self.sequence = self.sequence.wrapping_add(1);
        Ok(FrameInfo {
            sequence,
            bytes: 0,
            concealed: true,
            finished: false,
            valid_samples: self.duration.samples(),
        })
    }
    /// Reset overlap, waveform history and sequence without allocating.
    pub fn reset(&mut self) {
        self.overlap.fill(0.0);
        self.concealment.reset();
        self.sequence = 0;
        self.finished = false;
    }
    fn synthesize(
        &mut self,
        coefficients: &[f32; FRAME_SAMPLES],
        output: &mut [f32],
    ) -> Result<(), Error> {
        self.plan
            .inverse(
                &coefficients[..self.duration.samples()],
                &mut self.block[..2 * self.duration.samples()],
                &mut self.workspace,
            )
            .map_err(|_| Error::Packet)?;
        let samples = self.duration.samples();
        let start = (samples - ALGORITHMIC_DELAY) / 2;
        for (index, value) in output.iter_mut().enumerate() {
            let overlap = if index < ALGORITHMIC_DELAY {
                self.overlap[index]
            } else {
                0.0
            };
            *value = (overlap + self.block[start + index]).clamp(-1.0, 1.0);
        }
        self.overlap
            .copy_from_slice(&self.block[start + samples..start + samples + ALGORITHMIC_DELAY]);
        Ok(())
    }
}

#[cfg(test)]
mod harmonic_protocol_tests {
    use super::*;

    fn fixture(
        duration: FrameDuration,
        length: usize,
        f0: usize,
        codes: [u8; 4],
        bad_padding: bool,
        sequence: u8,
    ) -> (Vec<u8>, Vec<(usize, f64, f64)>) {
        let samples = duration.samples();
        let frequency_edges = [0, 1000, 2000, 4000, 8001];
        let mut dimensions = [0usize; 4];
        let mut harmonic_bands = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
        for group in 0..4 {
            harmonic_bands[group] = (1..=8000 / f0)
                .filter(|&h| {
                    h * f0 >= frequency_edges[group] && h * f0 < frequency_edges[group + 1]
                })
                .collect();
            dimensions[group] = [2, 4, 8, 16, 20, 40, 80, 160]
                .into_iter()
                .find(|&d| d >= 2 * harmonic_bands[group].len())
                .unwrap();
        }
        let gain_bits = crate::pulse::gain_bits(samples, length - 4);
        let gain_step = 1u8 << (8 - gain_bits);
        let allocation = crate::pulse::allocate_dimensions(
            dimensions,
            &codes,
            (length - 4) * 8 - 9,
            4 * gain_bits as usize,
        );
        let mut packet = vec![0u8; length];
        packet[0] = (VERSION << 5) | 14 | duration.profile();
        packet[1] = sequence;
        let mut writer = entropy::Encoder::new(&mut packet[2..length - 2]);
        writer.raw((f0 - 75) as u32, 9).unwrap();
        for code in codes {
            writer.raw(u32::from(code / gain_step), gain_bits).unwrap();
        }
        let mut tones = Vec::new();
        let mut padding_inserted = false;
        for group in 0..4 {
            let k = allocation[group];
            let mut shape = vec![0i16; dimensions[group]];
            if k != 0 {
                shape[0] = (k.saturating_sub(1)) as i16;
                shape[1] = -1;
                if bad_padding
                    && !padding_inserted
                    && 2 * harmonic_bands[group].len() < dimensions[group]
                {
                    shape[0] = 0;
                    shape[dimensions[group] - 1] = k.saturating_sub(1) as i16;
                    padding_inserted = true;
                }
            }
            let norm = shape
                .iter()
                .map(|&q| f64::from(q).powi(2))
                .sum::<f64>()
                .sqrt();
            let code = codes[group];
            let exponent = if code <= 64 {
                -16.0 + f64::from(code) / 8.0
            } else {
                -8.0 + f64::from(code - 64) / 16.0
            };
            let scale = if norm == 0.0 {
                0.0
            } else {
                2.0f64.powf(exponent) / norm
            };
            for (index, &harmonic) in harmonic_bands[group].iter().enumerate() {
                tones.push((
                    harmonic,
                    f64::from(shape[2 * index]) * scale,
                    f64::from(shape[2 * index + 1]) * scale,
                ));
            }
            crate::pulse::encode(&mut writer, &shape, k).unwrap();
        }
        if bad_padding {
            assert!(padding_inserted);
        }
        writer.finish_padded().unwrap();
        let crc = checksum(&packet[..length - 2]);
        packet[length - 2..].copy_from_slice(&crc.to_le_bytes());
        (packet, tones)
    }

    fn independent_window(position: usize, samples: usize) -> f64 {
        let start = (samples - 120) / 2;
        let reflected = position.min(2 * samples - 1 - position);
        if reflected < start {
            0.0
        } else if reflected >= start + 120 {
            1.0
        } else {
            let angle = std::f64::consts::FRAC_PI_2 * ((reflected - start) as f64 + 0.5) / 120.0;
            (std::f64::consts::FRAC_PI_2 * angle.sin().powi(2)).sin()
        }
    }

    #[test]
    fn harmonic_packets_match_independent_sinusoidal_and_cosine_equations() {
        for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
            let samples = duration.samples();
            let start = (samples - 120) / 2;
            for f0 in [75, 137, 200, 400] {
                for length in [
                    12_000 / duration.divisor() as usize,
                    32_000 / duration.divisor() as usize,
                ] {
                    let (packet, tones) =
                        fixture(duration, length, f0, [128, 128, 160, 160], false, 0);
                    let wave: Vec<_> = (0..2 * samples)
                        .map(|position| {
                            if position < start || position >= start + samples + 120 {
                                return 0.0;
                            }
                            tones
                                .iter()
                                .map(|&(h, cosine, sine)| {
                                    let angle = std::f64::consts::TAU
                                        * (h * f0) as f64
                                        * (position - start) as f64
                                        / 48000.0;
                                    cosine * angle.cos() + sine * angle.sin()
                                })
                                .sum::<f64>()
                                * independent_window(position, samples)
                        })
                        .collect();
                    let coefficients: Vec<_> = (0..samples)
                        .map(|frequency| {
                            wave.iter()
                                .enumerate()
                                .map(|(position, &value)| {
                                    let angle = std::f64::consts::PI / samples as f64
                                        * (position as f64 + 0.5 + samples as f64 / 2.0)
                                        * (frequency as f64 + 0.5);
                                    value * angle.cos()
                                })
                                .sum::<f64>()
                                * 2.0
                                / samples as f64
                        })
                        .collect();
                    let mut output = vec![0.0f32; samples];
                    Decoder::with_frame_duration(duration)
                        .decode(&packet, &mut output)
                        .unwrap();
                    for (index, &actual) in output.iter().enumerate() {
                        let position = start + index;
                        let expected = coefficients
                            .iter()
                            .enumerate()
                            .map(|(frequency, &value)| {
                                let angle = std::f64::consts::PI / samples as f64
                                    * (position as f64 + 0.5 + samples as f64 / 2.0)
                                    * (frequency as f64 + 0.5);
                                value * angle.cos()
                            })
                            .sum::<f64>()
                            * independent_window(position, samples);
                        assert!(
                            (f64::from(actual) - expected.clamp(-1.0, 1.0)).abs() < 2e-5,
                            "{duration:?} f0={f0} length={length} sample={index}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn invalid_harmonic_pitch_gains_and_padding_preserve_nonzero_history() {
        for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
            let length = 32_000 / duration.divisor() as usize;
            let samples = duration.samples();
            let (valid, _) = fixture(duration, length, 137, [128, 128, 160, 160], false, 0);
            let (next, _) = fixture(duration, length, 137, [128, 128, 160, 160], false, 1);
            let (bad_padding, _) = fixture(duration, length, 137, [128, 128, 160, 160], true, 1);
            let bad_pitch = bad_header_fields(duration, length, 511, 0, 1);
            let bad_gain = bad_header_fields(duration, length, 62, 255, 1);
            for invalid in [&bad_padding, &bad_pitch, &bad_gain] {
                let mut reference = Decoder::with_frame_duration(duration);
                let mut tested = Decoder::with_frame_duration(duration);
                let mut expected = vec![0.0; samples];
                let mut actual = vec![0.0; samples];
                reference.decode(&valid, &mut expected).unwrap();
                tested.decode(&valid, &mut actual).unwrap();
                assert!(actual.iter().any(|&x| x != 0.0));
                actual.fill(0.31415);
                assert_eq!(tested.decode(invalid, &mut actual), Err(Error::Packet));
                assert!(actual.iter().all(|&x| x == 0.31415));
                reference.decode(&next, &mut expected).unwrap();
                tested.decode(&next, &mut actual).unwrap();
                assert_eq!(actual, expected);
                reference.conceal(&mut expected).unwrap();
                tested.conceal(&mut actual).unwrap();
                assert_eq!(actual, expected);
            }
        }
    }

    #[test]
    fn harmonic_entropy_fits_every_supported_pitch_and_packet_budget() {
        for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
            let mut decoder = Decoder::with_frame_duration(duration);
            let mut output = vec![0.0f32; duration.samples()];
            for length in
                12_000 / duration.divisor() as usize..=32_000 / duration.divisor() as usize
            {
                for f0 in 75..=400 {
                    for codes in [[0; 4], [208; 4]] {
                        let (packet, _) = fixture(duration, length, f0, codes, false, 0);
                        decoder.reset();
                        decoder.decode(&packet, &mut output).unwrap();
                        assert!(output.iter().all(|&x| x.is_finite() && x.abs() <= 1.0));
                        if codes == [0; 4] {
                            assert!(output.iter().all(|&x| x == 0.0));
                        }
                    }
                }
            }
        }
    }

    fn bad_header_fields(
        duration: FrameDuration,
        length: usize,
        pitch: u32,
        gain: u32,
        sequence: u8,
    ) -> Vec<u8> {
        let mut packet = vec![0u8; length];
        packet[0] = (VERSION << 5) | 14 | duration.profile();
        packet[1] = sequence;
        let mut writer = entropy::Encoder::new(&mut packet[2..length - 2]);
        writer.raw(pitch, 9).unwrap();
        let bits = crate::pulse::gain_bits(duration.samples(), length - 4);
        for _ in 0..4 {
            writer.raw(gain >> (8 - bits), bits).unwrap();
        }
        writer.finish_padded().unwrap();
        let crc = checksum(&packet[..length - 2]);
        packet[length - 2..].copy_from_slice(&crc.to_le_bytes());
        packet
    }
}
