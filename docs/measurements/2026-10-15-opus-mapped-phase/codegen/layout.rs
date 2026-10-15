fn main() {
    println!("{{\"ordinary\":{},\"mapped\":{},\"projection\":{},\"align\":[{},{},{}]}}",
        size_of::<snacka::opus::Decoder>(), size_of::<snacka::opus::multistream::MultistreamDecoder>(), size_of::<snacka::opus::projection::ProjectionDecoder>(),
        align_of::<snacka::opus::Decoder>(), align_of::<snacka::opus::multistream::MultistreamDecoder>(), align_of::<snacka::opus::projection::ProjectionDecoder>());
}
