// SPDX-License-Identifier: MIT
//! RFC 8486 family-3 Ambisonics demixing over the owned Opus multistream decoder.
//!
//! The matrix is signed little-endian Q15 in column-major order. Output retains
//! ACN/SN3D Ambisonics channels and optional non-diegetic left/right channels.
//! Header gain is a separate playback operation; mixing never clips PCM.

use super::Recovery;
use super::multistream::{ChannelMapping, Error, MultistreamDecoder};

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

    /// Clear coded prediction state and scratch samples, preserving the matrix.
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
