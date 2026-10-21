// SPDX-License-Identifier: MIT
//! Report stream state and the exact typed quantized model constant payload.
#[path = "../src/neural_plc_weights.rs"]
mod weights;

fn main() {
    let model_bytes = std::mem::size_of_val(&weights::W1)
        + std::mem::size_of_val(&weights::S1)
        + std::mem::size_of_val(&weights::B1)
        + std::mem::size_of_val(&weights::W2)
        + std::mem::size_of_val(&weights::S2)
        + std::mem::size_of_val(&weights::B2)
        + std::mem::size_of_val(&weights::W3)
        + std::mem::size_of_val(&weights::S3)
        + std::mem::size_of_val(&weights::B3);
    println!(
        "decoder_bytes={},encoder_bytes={},model_constant_bytes={},enabled={},strength={}",
        std::mem::size_of::<snacka::Decoder>(),
        std::mem::size_of::<snacka::Encoder>(),
        model_bytes,
        weights::ENABLED,
        weights::STRENGTH
    );
}
