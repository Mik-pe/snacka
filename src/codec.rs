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

const HEADER: usize = 4;
const CHECKSUM: usize = 2;
const VERSION: u8 = 1;
const END: u8 = 16;
const MIN_PACKET_BYTES: usize = 15;
const GAIN_STEPS: [f32; 9] = [
    1.0,
    1.090_507_7,
    1.189_207_1,
    1.296_839_6,
    std::f32::consts::SQRT_2,
    1.542_210_8,
    1.681_792_9,
    1.834_008_1,
    2.0,
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
        })
    }
    /// Select a frame duration before constructing the stream.
    pub fn with_frame_duration(mut self, duration: FrameDuration) -> Self {
        self.duration = duration;
        self
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

// ceil(log2(binomial(N, K))) for exact integer subset cardinalities.
const SUBSET_BITS_480: [usize; 81] = [
    0, 9, 17, 25, 32, 38, 44, 50, 56, 62, 68, 73, 78, 84, 89, 94, 98, 103, 108, 112, 117, 121, 126,
    130, 134, 139, 143, 147, 151, 155, 159, 163, 166, 170, 174, 177, 181, 185, 188, 192, 195, 199,
    202, 205, 209, 212, 215, 218, 222, 225, 228, 231, 234, 237, 240, 243, 246, 249, 252, 254, 257,
    260, 263, 265, 268, 271, 274, 276, 279, 281, 284, 286, 289, 291, 294, 296, 299, 301, 303, 306,
    308,
];
const SUBSET_BITS_960: [usize; 81] = [
    0, 10, 19, 28, 36, 43, 50, 58, 64, 71, 78, 84, 90, 97, 103, 109, 115, 120, 126, 132, 137, 143,
    148, 154, 159, 164, 169, 174, 179, 184, 189, 194, 199, 204, 209, 213, 218, 223, 227, 232, 236,
    241, 245, 250, 254, 259, 263, 267, 271, 276, 280, 284, 288, 292, 296, 300, 304, 308, 312, 316,
    320, 324, 328, 332, 335, 339, 343, 347, 350, 354, 358, 361, 365, 369, 372, 376, 379, 383, 386,
    390, 393,
];
fn coefficient_count(payload_bytes: usize, samples: usize) -> usize {
    let subset_bits = if samples == 480 {
        &SUBSET_BITS_480
    } else {
        &SUBSET_BITS_960
    };
    let budget = payload_bytes * 8;
    // Gain uses eight raw bits; three bits cover interval rounding and termination.
    subset_bits
        .iter()
        .enumerate()
        .take_while(|&(count, &bits)| bits + 6 * count + 11 <= budget)
        .last()
        .map_or(0, |(count, _)| count)
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
    /// Validate packet length, magic, version, profile and CRC-16/CCITT-FALSE.
    pub fn parse(bytes: &'a [u8]) -> Result<Self, Error> {
        if !(MIN_PACKET_BYTES..=MAX_PACKET_BYTES).contains(&bytes.len()) || bytes[..2] != *b"SN" {
            return Err(Error::Packet);
        }
        if bytes[2] >> 5 != VERSION {
            return Err(Error::Version);
        }
        let duration = match bytes[2] & 15 {
            0 => FrameDuration::Ms20,
            1 => FrameDuration::Ms10,
            _ => return Err(Error::Profile),
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
        if self.bytes[2] & 15 == 0 {
            FrameDuration::Ms20
        } else {
            FrameDuration::Ms10
        }
    }
    /// Frame sequence number modulo 256.
    pub fn sequence(&self) -> u8 {
        self.bytes[3]
    }
    /// Whether this packet drains and finishes the stream.
    pub fn is_final(&self) -> bool {
        self.bytes[2] & END != 0
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
    let exponent = i32::from(code) - 128;
    let scale = f32::from_bits(((exponent.div_euclid(8) + 127) as u32) << 23);
    scale * GAIN_STEPS[exponent.rem_euclid(8) as usize]
}
fn gain_code(peak: f32) -> u8 {
    if peak <= 1.0 / 65_536.0 {
        return 0;
    }
    let exponent = ((peak.to_bits() >> 23) & 255) as i32 - 127;
    let mantissa = f32::from_bits((peak.to_bits() & 0x7f_ffff) | (127 << 23));
    let step = GAIN_STEPS.partition_point(|&threshold| threshold < mantissa);
    (128 + 8 * exponent + step as i32) as u8
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
        let count = coefficient_count(payload_bytes, samples);
        let mut indices: [usize; FRAME_SAMPLES] = std::array::from_fn(|index| index);
        indices[..samples].select_nth_unstable_by(count, |&left, &right| {
            self.coefficients[right]
                .abs()
                .total_cmp(&self.coefficients[left].abs())
                .then(left.cmp(&right))
        });
        indices[..count].sort_unstable();
        let peak = self.coefficients[..samples]
            .iter()
            .map(|value| value.abs())
            .fold(0.0f32, f32::max);
        let code = gain_code(peak);
        let scale = gain(code);
        let mut packet = [0; MAX_PACKET_BYTES];
        packet[..2].copy_from_slice(b"SN");
        packet[2] = (VERSION << 5) | self.config.duration.profile() | if ending { END } else { 0 };
        packet[3] = self.sequence;
        let mut writer = entropy::Encoder::new(&mut packet[HEADER..length - CHECKSUM]);
        writer.raw(u32::from(code), 8)?;
        let mut selected = 0;
        for index in 0..samples {
            let remaining = count - selected;
            if remaining == 0 {
                break;
            }
            let available = samples - index;
            let present = indices[selected] == index;
            if remaining != available {
                let probabilities = [0, (available - remaining) as u16, available as u16];
                writer.symbol(usize::from(present), entropy::Model::new(&probabilities)?)?;
            }
            if present {
                let quantized = if code == 0 {
                    0
                } else {
                    (self.coefficients[index] / scale * 31.0)
                        .round()
                        .clamp(-31.0, 31.0) as i32
                };
                writer.raw((quantized + 31) as u32, 6)?;
                selected += 1;
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

/// Stateful native audio decoder with bounded spectral concealment.
///
/// Discard the first `ALGORITHMIC_DELAY` samples once per stream. Decode the
/// final drain packet and retain only its valid prefix to recover the input
/// length. Construction allocates; decoding, concealment and reset do not.
pub struct Decoder {
    duration: FrameDuration,
    plan: MdctPlan,
    workspace: MdctWorkspace,
    overlap: [f32; ALGORITHMIC_DELAY],
    coefficients: [f32; FRAME_SAMPLES],
    block: [f32; 2 * FRAME_SAMPLES],
    sequence: u8,
    losses: u8,
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
            plan,
            workspace,
            overlap: [0.0; ALGORITHMIC_DELAY],
            coefficients: [0.0; FRAME_SAMPLES],
            block: [0.0; 2 * FRAME_SAMPLES],
            sequence: 0,
            losses: 0,
            finished: false,
        }
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
        let code = reader.raw(8)? as u8;
        if code > 136 {
            return Err(Error::Packet);
        }
        let scale = gain(code);
        let mut coefficients = [0.0; FRAME_SAMPLES];
        let mut remaining = coefficient_count(payload.len(), self.duration.samples());
        for (index, coefficient) in coefficients[..self.duration.samples()]
            .iter_mut()
            .enumerate()
        {
            if remaining == 0 {
                break;
            }
            let available = self.duration.samples() - index;
            let present = if remaining == available {
                true
            } else {
                let probabilities = [0, (available - remaining) as u16, available as u16];
                reader.symbol(entropy::Model::new(&probabilities)?) != 0
            };
            if present {
                let value = reader.raw(6)?;
                if value == 63 || (code == 0 && value != 31) {
                    return Err(Error::Packet);
                }
                *coefficient = (value as i32 - 31) as f32 * (scale / 31.0);
                remaining -= 1;
            }
        }
        if reader.tell() > (payload.len() * 8) as u64 {
            return Err(Error::Packet);
        }
        self.synthesize(&coefficients, output)?;
        self.coefficients = coefficients;
        self.sequence = self.sequence.wrapping_add(1);
        self.losses = 0;
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
    /// Synthesize exactly one missing frame and advance the sequence.
    ///
    /// Repeated losses decay spectral history; after 32 losses it becomes zero.
    /// This is an initial concealment baseline, not a trained speech predictor.
    pub fn conceal(&mut self, output: &mut [f32]) -> Result<FrameInfo, Error> {
        if self.finished {
            return Err(Error::Finished);
        }
        if output.len() != self.duration.samples() {
            return Err(Error::Output);
        }
        let losses = self.losses.saturating_add(1);
        let mut coefficients = self.coefficients;
        for value in &mut coefficients {
            *value *= if losses >= 32 { 0.0 } else { 0.8 };
        }
        self.synthesize(&coefficients, output)?;
        self.coefficients = coefficients;
        self.losses = losses;
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
    /// Reset overlap, spectral history and sequence without allocating.
    pub fn reset(&mut self) {
        self.overlap.fill(0.0);
        self.coefficients.fill(0.0);
        self.sequence = 0;
        self.losses = 0;
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
