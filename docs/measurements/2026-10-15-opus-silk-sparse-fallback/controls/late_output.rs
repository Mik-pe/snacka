use snacka::{entropy,opus::silk};
#[path="fallback.rs"] #[allow(dead_code)] mod shared;
fn proof() {
    let frame=silk::Frame{bandwidth:silk::Bandwidth::Narrow,duration:silk::FrameDuration::Ms10};
    let signal:[f32;80]=std::array::from_fn(|i|if i/4%2==0{1.0}else{-1.0});
    let mut encoder=silk::SilkEncoder::new();let mut tiny=[0xa5;2];
    let mut range=entropy::Encoder::new(&mut tiny);range.uniform(3,7).unwrap();
    assert_eq!(shared::no_alloc(||encoder.encode_layer(&mut range,frame,silk::PacketDuration::Ms10,1,&signal,104)),Err(entropy::Error::Output));
    assert_eq!(range.finish(),Err(entropy::Error::Output),"started writer must remain terminal; no retry");
    shared::no_alloc(||encoder.reset());
    let mut bytes=[0;13];let mut range=entropy::Encoder::new(&mut bytes);
    let stats=shared::no_alloc(||encoder.encode_layer(&mut range,frame,silk::PacketDuration::Ms10,1,&signal,104)).unwrap();
    assert!(stats.bits<=104);let final_range=range.range();let length=range.finish().unwrap();
    let mut decoder=silk::SilkDecoder::new();let mut range=entropy::Decoder::new(&bytes[..length]);let mut output=[0.0;80];
    assert_eq!(shared::no_alloc(||decoder.decode(&mut range,frame,silk::PacketDuration::Ms10,1,&mut output)).unwrap(),80);
    assert_eq!(range.range(),final_range);assert!(output.iter().all(|v|v.is_finite()));
    println!("{{\"started_writer\":\"terminal Output propagated\",\"reset_then_sparse_layer\":\"decoded exact range\",\"api_allocations\":0,\"stack_bytes\":1048576}}");
}
fn main() {std::thread::Builder::new().stack_size(1<<20).spawn(proof).unwrap().join().unwrap();}
