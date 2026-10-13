// SPDX-License-Identifier: MIT
//! Snacka-owned Opus encoding, decoding and Ogg file support (RFC 6716).
//!
//! Enabled by the dependency-free `opus` feature. [`CeltEncoder`] writes standard
//! mono/stereo CELT packets; [`Decoder`] reads SILK, Hybrid and CELT at 48 kHz.
//! Encoding and decoding allocate only during construction. [`ogg`] provides
//! bounded streaming file handling outside the audio callback.
//!
//! Packet inspection is borrowed and allocation-free. A parsed packet has valid
//! framing; its compressed audio still needs decoding. The native Snacka format
//! and this standard format have separate APIs and bitstreams.
//!
//! ```
//! use snacka::opus::{CeltConfig, CeltEncoder, Decoder};
//!
//! let mut encoder = CeltEncoder::new(CeltConfig::default());
//! let mut decoder = Decoder::new(1).unwrap();
//! let mut packet = [0; 1276];
//! let mut output = [0.0; 960];
//! let length = encoder.encode(&[0.0; 960], &mut packet).unwrap();
//! assert_eq!(decoder.decode(&packet[..length], &mut output).unwrap(), 960);
//! ```

pub mod packet;
pub use packet::{Bandwidth, Mode, Packet, PacketError, Toc};

mod allocation;
mod bands;
mod celt;
mod decoder;
mod energy;
mod flags;
pub mod ogg;
mod pvq;
mod range;
pub mod silk;
mod transform;

pub use celt::{CeltConfig, CeltDecoder, CeltEncoder, CeltFrameDuration, Error as CeltError};
pub use decoder::{Decoder, Error as DecodeError};
