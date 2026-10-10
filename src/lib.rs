// SPDX-License-Identifier: MIT
//! Independently written Rust audio codec components.
//!
//! G.711 sample coding is complete. Opus packet framing and entropy coding are
//! building blocks, not a complete Opus audio encoder or decoder.

#![forbid(unsafe_code)]
#![deny(missing_docs)]

pub mod entropy;
pub mod g711;
pub mod opus_packet;
pub mod transform;
