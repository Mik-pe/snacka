// SPDX-License-Identifier: MIT
//! RFC 8486 family-3 Ambisonics mixing over owned Opus multistream codecs.
//!
//! On-wire demixing matrices use signed little-endian Q15 in column-major order.
//! Encoder projections are explicit floating-point policies. Output retains
//! ACN/SN3D Ambisonics channels and optional non-diegetic left/right channels.
//! Header gain is a separate playback operation; mixing never clips PCM.

use super::multistream::{ChannelMapping, Error, MultistreamDecoder, MultistreamEncoder};
use super::{EncodeError, Encoder, EncoderConfig, Recovery};

/// Whether a count represents full Ambisonic order, optionally followed by stereo.
pub(crate) fn valid_channels(channels: u8) -> bool {
    (1..=15)
        .any(|order| channels as usize == order * order || channels as usize == order * order + 2)
}

/// Validated family-3 stream counts and a Q15 column-major demixing matrix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectionMapping {
    layout: ChannelMapping,
    channels: u8,
    matrix: Vec<i16>,
}

impl ProjectionMapping {
    /// Validate counts, output Ambisonic geometry and matrix dimensions.
    ///
    /// The matrix has `channels` rows and `streams + coupled` columns. Its
    /// header representation must fit the identification packet's single page.
    pub fn new(streams: u8, coupled: u8, channels: u8, matrix: Vec<i16>) -> Result<Self, Error> {
        let decoded = usize::from(streams) + usize::from(coupled);
        if streams == 0 || coupled > streams || decoded > 255 {
            return Err(Error::Streams);
        }
        if !valid_channels(channels)
            || matrix.len() != usize::from(channels) * decoded
            || 21 + 2 * matrix.len() > 65_024
        {
            return Err(Error::Mapping);
        }
        let identity: Vec<_> = (0..decoded).map(|index| index as u8).collect();
        Ok(Self {
            layout: ChannelMapping::new(streams, coupled, &identity)?,
            channels,
            matrix,
        })
    }

    /// Identity mapping for the coded channels before demixing.
    pub const fn coded_layout(&self) -> &ChannelMapping {
        &self.layout
    }
    /// Number of output ACN/SN3D and optional stereo channels.
    pub const fn channels(&self) -> usize {
        self.channels as usize
    }
    /// Signed Q15 coefficients, column-major with `channels` entries per column.
    pub fn matrix(&self) -> &[i16] {
        &self.matrix
    }
}

/// Stateful family-3 encoder with an explicit projection and demixing pair.
///
/// The input is normalized interleaved ACN/SN3D Ambisonics at the configured
/// standard Opus input rate, followed by optional non-diegetic stereo. For `C`
/// input channels and `K = streams + coupled` coded channels, encoding applies
/// `x = A * s`: `A` has `K` rows and
/// `C` columns. The associated [`ProjectionMapping`] supplies the on-wire `D`
/// matrix for decoding `s = D * x`. RFC 8486 standardizes `D`, not the choice
/// of `A`; callers select their projection, including lossy rectangular pairs.
/// Header gain remains a separate playback operation.
///
/// Construction allocates all codec, matrix and PCM memory. Encoding, status
/// inspection and reset do not allocate. Matrix sums use f64 before conversion
/// to the coded channels' f32 PCM. Overloaded mixtures are rejected before any
/// codec history advances; the matrix stage never clips PCM.
///
/// ```
/// use snacka::opus::EncoderConfig;
/// use snacka::opus::projection::{ProjectionEncoder, ProjectionMapping};
///
/// // Zeroth-order Ambisonics: invert polarity when coding and restore it
/// // with an exactly representable signed Q15 demixing coefficient.
/// let mapping = ProjectionMapping::new(1, 0, 1, vec![-32768]).unwrap();
/// let mut encoder = ProjectionEncoder::new(mapping, vec![-1.0],
///     &[EncoderConfig::default()]).unwrap();
/// let input = vec![0.1; encoder.input_samples()];
/// let mut packet = vec![0; encoder.max_packet_bytes()];
/// let length = encoder.encode(&input, &mut packet).unwrap();
/// assert!(length <= packet.len());
/// ```
pub struct ProjectionEncoder {
    encoder: MultistreamEncoder,
    mapping: ProjectionMapping,
    mixing: Vec<f32>,
    sources: Vec<Vec<(usize, f64)>>,
    samples: usize,
    pcm: Vec<f32>,
}

impl ProjectionEncoder {
    /// Prepare an explicit mixing/demixing pair and one policy per coded stream.
    ///
    /// `mixing` is finite column-major `K`-by-`C` data, with `K` coefficients
    /// per input channel. The mapping's signed Q15 demixing matrix is `C`-by-`K`.
    /// The matrices need not be inverses or square. All configurations must have
    /// the same input sample rate and packet duration. Initial coupled streams
    /// require stereo configurations; remaining streams require mono.
    pub fn new(
        mapping: ProjectionMapping,
        mixing: Vec<f32>,
        configs: &[EncoderConfig],
    ) -> Result<Self, Error> {
        let coded = mapping.coded_layout().decoded_channels();
        if mixing.len() != coded * mapping.channels()
            || mixing.iter().any(|coefficient| !coefficient.is_finite())
        {
            return Err(Error::Mapping);
        }
        let encoder = MultistreamEncoder::new(*mapping.coded_layout(), configs)?;
        let samples = encoder.input_samples();
        let mut sources = vec![Vec::new(); coded];
        for (input, coefficients) in mixing.chunks_exact(coded).enumerate() {
            for (row, &coefficient) in coefficients.iter().enumerate() {
                if coefficient != 0.0 {
                    sources[row].push((input, f64::from(coefficient)));
                }
            }
        }
        Ok(Self {
            encoder,
            mapping,
            mixing,
            sources,
            samples,
            pcm: vec![0.0; samples * coded],
        })
    }

    /// Input ACN/SN3D and optional stereo channel count.
    pub const fn channels(&self) -> usize {
        self.mapping.channels()
    }

    /// Configured input sample rate in hertz.
    pub fn sample_rate(&self) -> u32 {
        self.encoder.sample_rate()
    }

    /// Required input samples per channel for one complete projected packet.
    pub const fn input_samples(&self) -> usize {
        self.samples
    }

    /// Nominal encoder delay in samples at the configured input rate.
    pub fn algorithmic_delay_samples(&self) -> usize {
        self.encoder.algorithmic_delay_samples()
    }

    /// On-wire demixing metadata for [`super::ogg::OpusIdentification::projected`].
    pub const fn mapping(&self) -> &ProjectionMapping {
        &self.mapping
    }

    /// Original column-major mixing matrix, with `K` entries per input channel.
    pub fn mixing_matrix(&self) -> &[f32] {
        &self.mixing
    }

    /// Inspect a constituent encoder without changing its geometry or history.
    pub fn stream_encoder(&self, stream: usize) -> Option<&Encoder> {
        self.encoder.stream_encoder(stream)
    }

    /// Required output capacity, including every stream's self-delimiter ceiling.
    pub const fn max_packet_bytes(&self) -> usize {
        self.encoder.max_packet_bytes()
    }

    /// XOR of constituent final entropy ranges, matching projected decoding.
    pub fn final_range(&self) -> u32 {
        self.encoder.final_range()
    }

    /// Whether every coded stream suppressed its previous quiet interval.
    pub fn was_dtx(&self) -> bool {
        self.encoder.was_dtx()
    }

    /// Project and encode one configured interval from normalized interleaved PCM.
    ///
    /// Invalid input geometry, non-finite samples, insufficient output capacity
    /// or any mixed sample outside `[-1, 1]` leave output and codec history
    /// untouched. A subsequent codec failure can advance constituent history;
    /// reset before continuing, as with [`MultistreamEncoder::encode`].
    pub fn encode(&mut self, input: &[f32], output: &mut [u8]) -> Result<usize, Error> {
        if input.len() != self.samples * self.channels()
            || input
                .iter()
                .any(|sample| !sample.is_finite() || !(-1.0..=1.0).contains(sample))
        {
            return Err(EncodeError::Input.into());
        }
        if output.len() < self.max_packet_bytes() {
            return Err(Error::Output);
        }
        let coded = self.sources.len();
        for (input, output) in input
            .chunks_exact(self.channels())
            .zip(self.pcm.chunks_exact_mut(coded))
        {
            for (destination, sources) in output.iter_mut().zip(&self.sources) {
                let mut sum = 0.0;
                for &(source, coefficient) in sources {
                    sum += f64::from(input[source]) * coefficient;
                }
                if !(-1.0..=1.0).contains(&sum) {
                    return Err(EncodeError::Input.into());
                }
                *destination = sum as f32;
            }
        }
        self.encoder.encode(&self.pcm, output)
    }

    /// Clear prediction, clocks and scratch PCM, retaining both matrices.
    pub fn reset(&mut self) {
        self.encoder.reset();
        self.pcm.fill(0.0);
    }
}

/// Stateful matrix decoder with allocation-free decode, loss recovery and reset.
///
/// Construction prepares coded PCM and nonzero matrix destinations. All stream
/// framing and output capacity checks precede codec history changes. Header gain
/// remains the caller's responsibility, as with ordinary multistream decoding.
/// A compressed-content error may advance earlier constituent streams; reset
/// before continuing after such an error.
pub struct ProjectionDecoder {
    decoder: MultistreamDecoder,
    mapping: ProjectionMapping,
    destinations: Vec<Vec<(usize, f32)>>,
    pcm: Vec<f32>,
}

impl ProjectionDecoder {
    /// Prepare the projection decoder at 48 kHz.
    pub fn new(mapping: ProjectionMapping) -> Result<Self, Error> {
        Self::with_sample_rate(mapping, 48_000)
    }

    /// Prepare demixing at any standard Opus output rate.
    pub fn with_sample_rate(mapping: ProjectionMapping, sample_rate: u32) -> Result<Self, Error> {
        let decoder = MultistreamDecoder::with_sample_rate(*mapping.coded_layout(), sample_rate)?;
        let coded = mapping.coded_layout().decoded_channels();
        let mut destinations = vec![Vec::new(); coded];
        for (column, coefficients) in mapping
            .matrix()
            .chunks_exact(mapping.channels())
            .enumerate()
        {
            for (row, &coefficient) in coefficients.iter().enumerate() {
                if coefficient != 0 {
                    destinations[column].push((row, f32::from(coefficient) / 32768.0));
                }
            }
        }
        let samples = super::packet::MAX_PACKET_SAMPLES_48KHZ * sample_rate as usize / 48_000;
        Ok(Self {
            decoder,
            mapping,
            destinations,
            pcm: vec![0.0; coded * samples],
        })
    }

    /// Output channel count after demixing.
    pub const fn channels(&self) -> usize {
        self.mapping.channels()
    }
    /// Output sample rate in hertz.
    pub fn sample_rate(&self) -> u32 {
        self.decoder.sample_rate()
    }
    /// Validated projection layout and original Q15 matrix.
    pub const fn mapping(&self) -> &ProjectionMapping {
        &self.mapping
    }
    /// XOR of every constituent stream's final entropy range.
    pub fn final_range(&self) -> u32 {
        self.decoder.final_range()
    }

    /// Set output gain in signed Q8 decibels (256 units per dB).
    ///
    /// The default is zero, and the full `i16` range is supported. Gain applies
    /// once in each owned stream after resampling and before demixing, including
    /// decoded, recovered and concealed samples. No separate gain is applied to
    /// the demixed output. Floating output is not clipped; codec history and
    /// entropy intervals are unchanged. [`Self::reset`] retains this setting.
    /// Ogg header gain remains a separate caller-owned playback choice.
    pub fn set_gain_q8(&mut self, gain: i16) {
        self.decoder.set_gain_q8(gain);
    }

    /// Uniform coded-stream gain in signed Q8 decibels (256 units per dB).
    pub fn gain_q8(&self) -> i16 {
        self.decoder.gain_q8()
    }

    /// Set every coded stream's CELT phase-inversion policy before demixing.
    ///
    /// Includes mono streams. Reset retains the setting, and the setter does not
    /// clear prediction history. No separate phase operation is applied after
    /// demixing. See [`MultistreamDecoder::set_phase_inversion`].
    pub fn set_phase_inversion(&mut self, enabled: bool) {
        self.decoder.set_phase_inversion(enabled);
    }

    /// First coded stream's phase-inversion policy.
    ///
    /// Initial policies can differ between coupled and mono streams. The setter
    /// makes them uniform, so setting this returned value can change other
    /// streams' initial policies. See [`MultistreamDecoder::phase_inversion`].
    pub fn phase_inversion(&self) -> bool {
        self.decoder.phase_inversion()
    }

    fn check_output(&self, samples: usize, output: &[f32]) -> Result<(), Error> {
        let length = samples
            .checked_mul(self.channels())
            .ok_or(Error::Decode(super::DecodeError::Argument))?;
        if output.len() < length {
            return Err(Error::Decode(super::DecodeError::Output));
        }
        Ok(())
    }

    fn demix(&self, samples: usize, output: &mut [f32]) {
        let coded = self.mapping.coded_layout().decoded_channels();
        let channels = self.channels();
        output[..samples * channels].fill(0.0);
        for (input, output) in self.pcm[..samples * coded]
            .chunks_exact(coded)
            .zip(output[..samples * channels].chunks_exact_mut(channels))
        {
            for (&sample, destinations) in input.iter().zip(&self.destinations) {
                for &(row, coefficient) in destinations {
                    output[row] += sample * coefficient;
                }
            }
        }
    }

    /// Decode a complete family-3 transport packet and demix its channels.
    pub fn decode(&mut self, packet: &[u8], output: &mut [f32]) -> Result<usize, Error> {
        let parsed = super::multistream::MultistreamPacket::parse(
            packet,
            self.mapping.coded_layout().streams(),
        )?;
        let samples = parsed.samples(self.sample_rate())?;
        self.check_output(samples, output)?;
        self.decoder.decode(packet, &mut self.pcm)?;
        self.demix(samples, output);
        Ok(samples)
    }

    /// Recover the missing interval, then demix; decode the same packet afterwards.
    pub fn recover(
        &mut self,
        packet: &[u8],
        samples: usize,
        output: &mut [f32],
    ) -> Result<Recovery, Error> {
        self.check_output(samples, output)?;
        let recovery = self.decoder.recover(packet, samples, &mut self.pcm)?;
        self.demix(samples, output);
        Ok(recovery)
    }

    /// Conceal a supported missing interval and demix the predicted coded channels.
    pub fn conceal(&mut self, samples: usize, output: &mut [f32]) -> Result<usize, Error> {
        self.check_output(samples, output)?;
        self.decoder.conceal(samples, &mut self.pcm)?;
        self.demix(samples, output);
        Ok(samples)
    }

    /// Clear coded prediction state and scratch samples, retaining matrix and gain.
    pub fn reset(&mut self) {
        self.decoder.reset();
        self.pcm.fill(0.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn geometry_limits_and_column_major_signed_math() {
        for channels in 0..=255 {
            let expected = [
                1, 3, 4, 6, 9, 11, 16, 18, 25, 27, 36, 38, 49, 51, 64, 66, 81, 83, 100, 102, 121,
                123, 144, 146, 169, 171, 196, 198, 225, 227,
            ]
            .contains(&channels);
            assert_eq!(valid_channels(channels), expected);
        }
        assert!(ProjectionMapping::new(0, 0, 1, Vec::new()).is_err());
        assert!(ProjectionMapping::new(1, 2, 4, vec![0; 12]).is_err());
        assert!(ProjectionMapping::new(1, 0, 2, vec![0; 2]).is_err());
        assert!(ProjectionMapping::new(1, 0, 4, vec![0; 3]).is_err());
        assert!(ProjectionMapping::new(227, 0, 227, vec![0; 227 * 227]).is_err());
        let mapping = ProjectionMapping::new(
            1,
            1,
            4,
            vec![16384, -16384, 0, 32767, 8192, 16384, 0, -32768],
        )
        .unwrap();
        let mut decoder = ProjectionDecoder::new(mapping).unwrap();
        decoder.pcm[..4].copy_from_slice(&[0.5, 0.25, -0.25, 1.0]);
        let mut output = [99.0; 9];
        decoder.demix(2, &mut output);
        assert_eq!(
            &output[..4],
            &[0.3125, -0.125, 0.0, 0.5 * (32767.0 / 32768.0) - 0.25]
        );
        assert_eq!(
            &output[4..8],
            &[0.125, 0.625, 0.0, -0.25 * (32767.0 / 32768.0) - 1.0]
        );
        assert_eq!(output[8], 99.0);
    }
}
