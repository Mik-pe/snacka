// SPDX-License-Identifier: MIT
//! Actual online call timings, excluding capture/metric loops and encoding.
use snacka::{Config,Decoder,Encoder,FrameDuration,MAX_PACKET_BYTES};
use std::{env,fs,io::{self,Write},hint::black_box,time::Instant};
fn lost(i:usize)->bool {matches!(i%40,8|18|19|28..=33)}
fn main()->Result<(),Box<dyn std::error::Error>> {
 let a:Vec<_>=env::args().collect();if a.len()!=6{return Err(io::Error::other("input.f32 output.csv bitrate frame_ms harmonic").into());}
 let duration=if a[4]=="10"{FrameDuration::Ms10}else{FrameDuration::Ms20};let n=duration.samples();
 let config=Config::new(a[3].parse()?)?.with_frame_duration(duration).with_harmonic_speech(a[5]=="1");
 let raw=fs::read(&a[1])?;let input:Vec<_>=raw.chunks_exact(4).map(|x|f32::from_le_bytes(x.try_into().unwrap())).collect();
 let mut encoder=Encoder::new(config);let mut frame=vec![0.;n];let mut packet=[0;MAX_PACKET_BYTES];let mut packets=Vec::new();
 for chunk in input.chunks(n){frame.fill(0.);frame[..chunk.len()].copy_from_slice(chunk);let len=encoder.encode(&frame,&mut packet)?;packets.push(packet[..len].to_vec());}
 let mut decoder=Decoder::with_frame_duration(duration);let mut timings=Vec::new();
 for (i,p) in packets.iter().enumerate(){
  let t=Instant::now();if lost(i){decoder.conceal(black_box(&mut frame))?;}else{decoder.decode(black_box(p),black_box(&mut frame))?;}
  let elapsed=t.elapsed().as_nanos();black_box(&frame);timings.push((i,elapsed));
 }
 let mut out=io::BufWriter::new(fs::File::create(&a[2])?);writeln!(out,"frame,operation,elapsed_ns")?;
 for (i,elapsed) in timings {let kind=if lost(i){if i>0&&lost(i-1){"continued_loss"}else{"first_loss"}}else if i>0&&lost(i-1){"recovery"}else{"received"};writeln!(out,"{i},{kind},{elapsed}")?;}
 Ok(())
}
