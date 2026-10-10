// SPDX-License-Identifier: MIT
//! G.711 A-law and mu-law companding for signed 16-bit PCM.
//!
//! RTP PCMA and PCMU use an 8 kHz sample rate and one byte per sample.
//! These functions transform samples; resampling and packet timing belong
//! to the caller.

/// The two standardized G.711 companding laws.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Law {
    /// A-law, carried by RTP PCMA.
    A,
    /// Mu-law, carried by RTP PCMU.
    Mu,
}

/// Insufficient output capacity for a sample block.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferError {
    /// Number of output elements required.
    pub required: usize,
    /// Number of output elements available.
    pub available: usize,
}
impl std::fmt::Display for BufferError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            formatter,
            "sample buffer requires {} elements, has {}",
            self.required, self.available
        )
    }
}
impl std::error::Error for BufferError {}

impl Law {
    /// Encode a PCM16 block, returning the number of encoded bytes.
    ///
    /// Insufficient capacity leaves output untouched; unused output remains unchanged.
    pub fn encode_into(self, input: &[i16], output: &mut [u8]) -> Result<usize, BufferError> {
        if output.len() < input.len() {
            return Err(BufferError {
                required: input.len(),
                available: output.len(),
            });
        }
        match self {
            Self::A => {
                for (&sample, byte) in input.iter().zip(output) {
                    *byte = encode_a_law(sample);
                }
            }
            Self::Mu => {
                for (&sample, byte) in input.iter().zip(output) {
                    *byte = encode_mu_law(sample);
                }
            }
        }
        Ok(input.len())
    }

    /// Decode a G.711 block, returning the number of PCM16 samples.
    ///
    /// Insufficient capacity leaves output untouched; unused output remains unchanged.
    pub fn decode_into(self, input: &[u8], output: &mut [i16]) -> Result<usize, BufferError> {
        if output.len() < input.len() {
            return Err(BufferError {
                required: input.len(),
                available: output.len(),
            });
        }
        match self {
            Self::A => {
                for (&byte, sample) in input.iter().zip(output) {
                    *sample = decode_a_law(byte);
                }
            }
            Self::Mu => {
                for (&byte, sample) in input.iter().zip(output) {
                    *sample = decode_mu_law(byte);
                }
            }
        }
        Ok(input.len())
    }
}

fn segment(magnitude: u32) -> u8 {
    24u32.saturating_sub(magnitude.leading_zeros()).min(7) as u8
}

/// Encode one PCM sample as G.711 mu-law.
#[must_use]
pub fn encode_mu_law(sample: i16) -> u8 {
    let signed = i32::from(sample);
    let sign = if signed < 0 { 0x80 } else { 0 };
    let magnitude = signed.unsigned_abs().min(32_635) + 132;
    let exponent = segment(magnitude);
    let mantissa = ((magnitude >> (u32::from(exponent) + 3)) & 15) as u8;
    !(sign | (exponent << 4) | mantissa)
}

/// Decode one G.711 mu-law byte to PCM.
#[must_use]
pub fn decode_mu_law(code: u8) -> i16 {
    let bits = !code;
    let magnitude = ((i32::from(bits & 15) << 3) + 132) << ((bits >> 4) & 7);
    let sample = if bits & 0x80 != 0 {
        132 - magnitude
    } else {
        magnitude - 132
    };
    sample as i16
}

/// Encode one PCM sample as G.711 A-law.
#[must_use]
pub fn encode_a_law(sample: i16) -> u8 {
    let signed = i32::from(sample);
    let (magnitude, mask) = if signed < 0 {
        ((-signed - 1) as u32, 0x55)
    } else {
        (signed as u32, 0xd5)
    };
    let exponent = segment(magnitude);
    let shift = if exponent < 2 {
        4
    } else {
        u32::from(exponent) + 3
    };
    let mantissa = ((magnitude >> shift) & 15) as u8;
    ((exponent << 4) | mantissa) ^ mask
}

/// Decode one G.711 A-law byte to PCM.
#[must_use]
pub fn decode_a_law(code: u8) -> i16 {
    let bits = code ^ 0x55;
    let exponent = (bits >> 4) & 7;
    let quantized = i32::from(bits & 15) << 4;
    let magnitude = match exponent {
        0 => quantized + 8,
        1 => quantized + 264,
        _ => (quantized + 264) << (exponent - 1),
    };
    if bits & 0x80 != 0 {
        magnitude as i16
    } else {
        (-magnitude) as i16
    }
}
