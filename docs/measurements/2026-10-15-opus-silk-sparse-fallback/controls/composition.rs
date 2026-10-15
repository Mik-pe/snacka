#[path="fallback.rs"] #[allow(dead_code)] mod shared;
use shared::no_alloc;
use snacka::opus::{Application, Bandwidth, Decoder, Encoder, EncoderConfig, FrameDuration, Mode, RateControl,
    multistream::{ChannelMapping, MultistreamEncoder, MultistreamDecoder, MultistreamPacket},
    projection::{ProjectionMapping, ProjectionEncoder, ProjectionDecoder}};
use std::{error::Error, fs::{self, File}, io::Write, path::Path};
const N: usize = 480;
const ROWS: [[f64;4];4] = [[0.0,1.0,0.0,0.0],[0.0,0.0,-1.0,0.0],[1.0,0.0,0.0,0.0],[0.0,0.0,0.0,-1.0]];
fn config(bitrate:u32, channels:usize)->EncoderConfig {
    EncoderConfig::new(bitrate,channels,FrameDuration::Ms10,Bandwidth::Narrowband,Application::Voice).unwrap()
        .with_mode(Mode::Silk).unwrap().with_rate_control(RateControl::Variable)
}
fn signal(channels:usize,index:usize)->Vec<f32> {
    (0..N*channels).map(|position|{
        let sample=position/channels;let channel=position%channels;
        match(sample/24+index+channel*3+sample/(24*(5+channel*2)))%8 {0|1=>1.0,2|3=>-1.0,4=>f32::from_bits(0x3f7fffff),5=>-f32::from_bits(0x3f7fffff),6=>0.0,_=>-0.0}
    }).collect()
}
fn failed(summary:&mut File,name:&str,index:usize,error:impl std::fmt::Debug,projection:bool)->Result<(),Box<dyn Error>> {
    let (coupled,mapping,channels)=if projection {(1,"[0,1,2,3]",4)}else{(0,"[2,0,1,0,255]",5)};
    writeln!(summary,"{{\"name\":\"{name}\",\"packets\":{index},\"failed_at\":{index},\"error\":\"{error:?}\",\"streams\":3,\"coupled\":{coupled},\"mapping\":{mapping},\"output_channels\":{channels}}}")?;
    Ok(())
}
fn same_pcm(a:&[f32],b:&[f32]) {
    assert_eq!(a.len(),b.len());
    assert!(a.iter().zip(b).all(|(x,y)|x.to_bits()==y.to_bits()));
}
fn word(file:&mut File,value:u32) {file.write_all(&value.to_le_bytes()).unwrap();}
fn record(file:&mut File,packet:&[u8],range:u32,reset:bool,channels:usize) {
    for value in [packet.len() as u32,range,N as u32,u32::from(reset),channels as u32] {word(file,value);}
    file.write_all(packet).unwrap();
}
fn reference_packets(input:&[Vec<f32>], encoders:&mut [Encoder], decoders:&mut [Decoder])->Result<(Vec<Vec<u8>>,Vec<Vec<f32>>,u32),Box<dyn Error>> {
    let mut packets=Vec::new(); let mut pcm=Vec::new(); let mut range=0;
    for ((input,encoder),decoder) in input.iter().zip(encoders).zip(decoders) {
        let mut packet=vec![0;encoder.config().max_packet_bytes()];
        let bytes=no_alloc(||encoder.encode(input,&mut packet))?; packet.truncate(bytes);
        let mut output=vec![0.0;N*encoder.config().channels()];
        assert_eq!(no_alloc(||decoder.decode(&packet,&mut output))?,N);
        assert_eq!(decoder.final_range(),encoder.final_range());
        range^=encoder.final_range(); packets.push(packet);pcm.push(output);
    }
    Ok((packets,pcm,range))
}
fn mapped(path:&Path,bits:u32,summary:&mut File)->Result<(),Box<dyn Error>> {
    let name=format!("mapped-fallback-{bits}");
    let mapping=ChannelMapping::new(3,0,&[2,0,1,0,255])?;
    let configs=[config(bits,1);3];
    let mut actual=MultistreamEncoder::new(mapping,&configs)?;
    let mut decoder=MultistreamDecoder::new(mapping)?;
    let mut independent:Vec<_>=configs.into_iter().map(|c|Encoder::new(c).unwrap()).collect();
    let mut independent_decoders:Vec<_>=(0..3).map(|_|Decoder::new(1).unwrap()).collect();
    let mut packet=vec![0;actual.max_packet_bytes()];
    let mut expected=packet.clone(); let mut output=vec![0.0;N*5];
    let mut archive=File::create(path.join(format!("{name}.packets")))?;
    for index in 0..16 {
        let reset=index==8;
        if reset {no_alloc(||{actual.reset();decoder.reset();for e in &mut independent {e.reset();}for d in &mut independent_decoders {d.reset();}});}
        let input=signal(5,index);
        let bytes=match no_alloc(||actual.encode(&input,&mut packet)) {Ok(n)=>n,Err(e)=>{failed(summary,&name,index,e,false)?;return Ok(());}};
        // Literal first-source gather: coded0 <- input1, coded1 <- input2, coded2 <- input0.
        let gathered:Vec<Vec<f32>>=[1,2,0].into_iter().map(|channel|input.chunks_exact(5).map(|f|f[channel]).collect()).collect();
        let (packets,pcm,range)=reference_packets(&gathered,&mut independent,&mut independent_decoders)?;
        let references:Vec<_>=packets.iter().map(Vec::as_slice).collect();
        let expected_bytes=no_alloc(||MultistreamPacket::write(&references,&mut expected))?;
        
        assert_eq!(&packet[..bytes],&expected[..expected_bytes]);
        assert_eq!(actual.final_range(),range);
        for (stream,e) in independent.iter().enumerate() {assert_eq!(actual.stream_encoder(stream).unwrap().final_range(),e.final_range());}
        assert_eq!(no_alloc(||decoder.decode(&packet[..bytes],&mut output))?,N);assert_eq!(decoder.final_range(),range);
        let mut expected_pcm=Vec::with_capacity(N*5);
        for sample in 0..N {expected_pcm.extend_from_slice(&[pcm[2][sample],pcm[0][sample],pcm[1][sample],pcm[0][sample],0.0]);}
        same_pcm(&output,&expected_pcm);
        record(&mut archive,&packet[..bytes],range,reset,5);
    }
    writeln!(summary,"{{\"name\":\"{name}\",\"packets\":16,\"failed_at\":null,\"api_allocations\":0,\"stack_bytes\":1048576,\"streams\":3,\"coupled\":0,\"mapping\":[2,0,1,0,255],\"output_channels\":5,\"exact_transport\":true,\"exact_stream_ranges\":true,\"exact_pcm\":true}}")?;
    Ok(())
}
fn projected(path:&Path,bits:u32,summary:&mut File)->Result<(),Box<dyn Error>> {
    let name=format!("projection-fallback-{bits}");
    let demixing:Vec<i16>=ROWS.iter().flat_map(|row|row.iter().map(|&value|(value*32768.0).clamp(-32768.0,32767.0) as i16)).collect();
    let mapping=ProjectionMapping::new(3,1,4,demixing)?;
    let mixing:Vec<f32>=(0..4).flat_map(|input|ROWS.iter().map(move |row|row[input] as f32)).collect();
    let configs=[config(24_000,2),config(bits,1),config(bits,1)];
    let mut actual=ProjectionEncoder::new(mapping.clone(),mixing,&configs)?;
    let mut decoder=ProjectionDecoder::new(mapping)?;
    let mut independent:Vec<_>=configs.into_iter().map(|c|Encoder::new(c).unwrap()).collect();
    let mut independent_decoders:Vec<_>=[2,1,1].into_iter().map(|c|Decoder::new(c).unwrap()).collect();
    let mut packet=vec![0;actual.max_packet_bytes()]; let mut expected=packet.clone(); let mut output=vec![0.0;N*4];
    let mut archive=File::create(path.join(format!("{name}.packets")))?;
    for index in 0..16 {
        let reset=index==8;
        if reset {no_alloc(||{actual.reset();decoder.reset();for e in &mut independent {e.reset();}for d in &mut independent_decoders {d.reset();}});}
        let input=signal(4,index);
        let bytes=match no_alloc(||actual.encode(&input,&mut packet)) {Ok(n)=>n,Err(e)=>{failed(summary,&name,index,e,true)?;return Ok(());}};let mut gathered=vec![Vec::with_capacity(N*2),Vec::with_capacity(N),Vec::with_capacity(N)];
        for frame in input.chunks_exact(4) {
            let mixed:[f32;4]=std::array::from_fn(|row|ROWS[row].iter().zip(frame).map(|(&coefficient,&sample)|coefficient*f64::from(sample)).sum::<f64>() as f32);
            gathered[0].extend_from_slice(&mixed[..2]);gathered[1].push(mixed[2]);gathered[2].push(mixed[3]);
        }
        let (packets,pcm,range)=reference_packets(&gathered,&mut independent,&mut independent_decoders)?;
        let references:Vec<_>=packets.iter().map(Vec::as_slice).collect();
        let expected_bytes=no_alloc(||MultistreamPacket::write(&references,&mut expected))?;
        assert_eq!(&packet[..bytes],&expected[..expected_bytes]);
        assert_eq!(actual.final_range(),range);
        for (stream,e) in independent.iter().enumerate() {assert_eq!(actual.stream_encoder(stream).unwrap().final_range(),e.final_range());}
        assert_eq!(no_alloc(||decoder.decode(&packet[..bytes],&mut output))?,N);assert_eq!(decoder.final_range(),range);
        let mut expected_pcm=Vec::with_capacity(N*4);
        for sample in 0..N {
            let coded=[pcm[0][sample*2],pcm[0][sample*2+1],pcm[1][sample],pcm[2][sample]];
            for destination in 0..4 {
                let mut value=0.0f32;
                for source in 0..4 {value+=coded[source]*((ROWS[source][destination]*32768.0).clamp(-32768.0,32767.0) as f32/32768.0);}
                expected_pcm.push(value);
            }
        }
        same_pcm(&output,&expected_pcm);
        record(&mut archive,&packet[..bytes],range,reset,4);
    }
    writeln!(summary,"{{\"name\":\"{name}\",\"packets\":16,\"failed_at\":null,\"api_allocations\":0,\"stack_bytes\":1048576,\"streams\":3,\"coupled\":1,\"mapping\":[0,1,2,3],\"output_channels\":4,\"exact_transport\":true,\"exact_stream_ranges\":true,\"exact_pcm\":true}}")?;
    Ok(())
}
fn run()->Result<(),Box<dyn Error>> {
    let directory=std::env::args().nth(1).ok_or("output required")?;let path=Path::new(&directory);fs::create_dir_all(path)?;
    let mut summary=File::create(path.join("streams.jsonl"))?;
    for bitrate in [12_000,16_000] {mapped(path,bitrate,&mut summary)?;projected(path,bitrate,&mut summary)?;}
    Ok(())
}

fn main()->Result<(),Box<dyn Error>> { std::thread::Builder::new().stack_size(1<<20).spawn(||run().map_err(|e|e.to_string()))?.join().unwrap().map_err(Into::into) }
