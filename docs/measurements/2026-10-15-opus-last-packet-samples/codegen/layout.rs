fn main() {
    println!("{{\"size\":{},\"align\":{}}}", core::mem::size_of::<snacka::opus::Decoder>(), core::mem::align_of::<snacka::opus::Decoder>());
}
