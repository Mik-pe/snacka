use snacka::opus::{Decoder, Packet};
use std::{fs, error::Error};
fn main()->Result<(),Box<dyn Error>>{
 let args:Vec<_>=std::env::args().skip(1).collect();
 let bytes=fs::read(&args[0])?;assert_eq!(&bytes[..8],b"SNKQ0001");
 let channels:usize=args[2].parse()?;let delay:usize=args[3].parse()?;let samples:usize=args[4].parse()?;
 let mut decoder=Decoder::new(channels)?;let mut at=8;let mut result=Vec::new();let mut pcm=vec![0.;5760*channels];
 while at<bytes.len(){
  let length=u32::from_le_bytes(bytes[at..at+4].try_into()?) as usize;
  let interval=u32::from_le_bytes(bytes[at+4..at+8].try_into()?);at+=8;
  let packet=&bytes[at..at+length];at+=length;let count=Packet::parse(packet)?.samples(48000)?;
  assert_eq!(decoder.decode(packet,&mut pcm[..count*channels])?,count);assert_eq!(decoder.final_range(),interval);
  result.extend_from_slice(&pcm[..count*channels]);
 }
 let result=&result[delay*channels..delay*channels+samples*channels];assert!(result.iter().all(|v|v.is_finite()));
 fs::write(&args[1],result.iter().flat_map(|v|v.to_le_bytes()).collect::<Vec<_>>())?;Ok(())
}
