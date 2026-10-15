// SPDX-License-Identifier: MIT
//! Snacka-owned Opus encoding, decoding and Ogg file support (RFC 6716).
//!
//! Enabled by the dependency-free `opus` feature. [`Encoder`] writes standard
//! mono/stereo SILK, Hybrid and CELT packets from 8/12/16/24/48 kHz PCM. [`Decoder`] reads
//! all three modes at the five standard output rates, with loss/FEC recovery.
//! Encoding and decoding allocate only during construction. [`ogg`] provides
//! bounded streaming file handling outside the audio callback.
//!
//! Packet inspection is borrowed and allocation-free. A parsed packet has valid
//! framing; its compressed audio still needs decoding. The native Snacka format
//! and this standard format have separate APIs and bitstreams.
//!
//! ```
//! use snacka::opus::{Decoder, Encoder, EncoderConfig};
//!
//! let mut encoder = Encoder::new(EncoderConfig::default()).unwrap();
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
mod dtx;
mod encoder;
mod encoder_transition;
mod energy;
mod flags;
mod input_delay;
mod input_resample;
pub mod multistream;
pub mod ogg;
pub mod pcm;
mod plc;
pub mod projection;
mod pvq;
mod range;
pub mod repacketize;
pub mod silk;
mod transform;

pub use celt::{CeltConfig, CeltDecoder, CeltEncoder, CeltFrameDuration, Error as CeltError};
pub use decoder::{Decoder, Error as DecodeError, Recovery};
pub use encoder::{
    Application, Encoder, EncoderConfig, Error as EncodeError, FrameDuration, RateControl,
};
