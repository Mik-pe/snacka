use snacka::opus::{Decoder, Encoder, EncoderConfig, multistream::{MultistreamDecoder, MultistreamEncoder}, projection::{ProjectionDecoder, ProjectionEncoder}};
fn main() {
    println!("{{\"decoder\":{},\"mapped_decoder\":{},\"projection_decoder\":{},\"encoder\":{},\"mapped_encoder\":{},\"projection_encoder\":{},\"config\":{}}}",
        std::mem::size_of::<Decoder>(), std::mem::size_of::<MultistreamDecoder>(), std::mem::size_of::<ProjectionDecoder>(),
        std::mem::size_of::<Encoder>(), std::mem::size_of::<MultistreamEncoder>(), std::mem::size_of::<ProjectionEncoder>(), std::mem::size_of::<EncoderConfig>());
}
