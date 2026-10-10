// SPDX-License-Identifier: MIT
//! Independent Rust speech codec with an experimental native bitstream.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

mod codec;
mod concealment;
pub mod entropy;
pub use codec::{
    ALGORITHMIC_DELAY, Config, Decoder, Encoder, Error, FRAME_SAMPLES, FrameDuration, FrameInfo,
    MAX_PACKET_BYTES, Packet, SAMPLE_RATE,
};
pub mod transform;
