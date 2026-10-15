use snacka::opus::{Application, Bandwidth, Decoder, Encoder, EncoderConfig, FrameDuration, Mode, Packet, RateControl, Recovery, silk};
use std::{alloc::{GlobalAlloc, Layout, System}, cell::Cell, error::Error, fs::{self, File}, io::Write, path::Path};
thread_local! {static ALLOCS: Cell<Option<usize>> = const {Cell::new(None)};}
fn allocation() {let _=ALLOCS.try_with(|count|if let Some(value)=count.get(){count.set(Some(value+1));});}
struct Allocator;
unsafe impl GlobalAlloc for Allocator {
    unsafe fn alloc(&self,layout:Layout)->*mut u8 {allocation();unsafe {System.alloc(layout)}}
    unsafe fn alloc_zeroed(&self,layout:Layout)->*mut u8 {allocation();unsafe {System.alloc_zeroed(layout)}}
    unsafe fn realloc(&self,p:*mut u8,l:Layout,n:usize)->*mut u8 {allocation();unsafe {System.realloc(p,l,n)}}
    unsafe fn dealloc(&self,p:*mut u8,l:Layout){unsafe {System.dealloc(p,l)}}
}
#[global_allocator] static ALLOCATOR:Allocator=Allocator;
pub fn no_alloc<T>(f:impl FnOnce()->T)->T {ALLOCS.with(|c|c.set(Some(0)));let result=f();let count=ALLOCS.with(|c|c.replace(None)).unwrap();assert_eq!(count,0);result}
fn config(rate:u32,bits:u32,channels:usize,duration:FrameDuration,bw:Bandwidth,fec:bool)->EncoderConfig {
    EncoderConfig::new(bits,channels,duration,bw,Application::Voice).unwrap().with_sample_rate(rate).unwrap().with_mode(Mode::Silk).unwrap().with_rate_control(RateControl::Constant).with_fec(fec).unwrap()
}
fn input(config:EncoderConfig,packet:usize,clip:bool)->Vec<f32> {
    let n=config.input_samples();let channels=config.channels();let rate=config.sample_rate() as usize;
    (0..n*channels).map(|position| {
        let channel=position%channels;let sample=position/channels;
        if clip {
            let width=(rate/2000).max(1);
            let cell=(sample/width+packet+channel*3+sample/(width*(5+channel*2)))%8;
            match cell {0|1=>1.0,2|3=>-1.0,4=>f32::from_bits(0x3f7fffff),5=>-f32::from_bits(0x3f7fffff),6=>0.0,_=>-0.0}
        } else {
            let t=(packet*n+sample) as f64/rate as f64;
            (0.03*(std::f64::consts::TAU*(137.0+channel as f64*43.0)*t).sin()) as f32
        }
    }).collect()
}
fn word(file:&mut File,n:u32) {file.write_all(&n.to_le_bytes()).unwrap();}
fn wire_flags(packet:&Packet<'_>)->Result<(bool,bool,bool,u8),Box<dyn Error>> {
    let duration=match packet.frame_samples(48000)? {480=>silk::PacketDuration::Ms10,960=>silk::PacketDuration::Ms20,1920=>silk::PacketDuration::Ms40,2880=>silk::PacketDuration::Ms60,_=>panic!("SILK geometry")};
    let channels=packet.toc().channels();let mut first_fec=false;let mut active=true;let mut any_fec=false;let mut first_kind=0;
    for (index,bytes) in packet.frames().enumerate() {
        let mut range=snacka::entropy::Decoder::new(bytes);let header=silk::Header::decode(&mut range,duration,channels)?;
        let mut frame_fec=false;
        for channel in 0..channels as usize {for frame in 0..3 {if let Some(value)=header.voice_activity(channel,frame){active&=value;}frame_fec|=header.redundancy(channel,frame)==Some(true);}}
        if index==0 {first_fec=frame_fec;}
        any_fec|=frame_fec;
        if index==0 && channels==1 && !frame_fec {
            first_kind=match silk::FrameType::decode(&mut range,header.voice_activity(0,0).unwrap()).signal {silk::SignalType::Inactive=>1,silk::SignalType::Unvoiced=>2,silk::SignalType::Voiced=>3};
        }
    }
    Ok((first_fec,any_fec,active,first_kind))
}
fn run(path:&Path,name:&str,config:EncoderConfig,carrier:bool,original:Option<&[f32]>,summary:&mut File)->Result<(),Box<dyn Error>> {
    let mut encoder=Encoder::new(config)?;let mut decoder=Decoder::new(config.channels())?;
    let mut recovery=Decoder::new(config.channels())?;
    let mut packet=vec![0xa5;config.max_packet_bytes()+1];let mut output=vec![0.0;config.duration().samples()*config.channels()];let mut recovered=output.clone();
    let mut packets=File::create(path.join(format!("{name}.packets")))?;let mut pcm=File::create(path.join(format!("{name}.pcm")))?;let mut events=File::create(path.join(format!("{name}.events.jsonl")))?;
    let limit=original.map_or(if carrier {14}else{8},|v|v.len()/config.input_samples());
    let mut accepted=0;let mut failed=None;let mut error=String::new();let mut fec=0;let mut carrier_fec=false;let mut recovery_fec=false;let mut all_active=true;
    for index in 0..limit {
        let clip=original.is_none() && (!carrier || index==6);
        let generated;
        let input=if let Some(original)=original {&original[index*config.input_samples()..(index+1)*config.input_samples()]}else{generated=input(config,index,clip);&generated};
        let result=no_alloc(||encoder.encode(input,&mut packet));
        let length=match result {Ok(length)=>length,Err(reason)=>{failed=Some(index);error=format!("{reason:?}");break;}};
        assert_eq!(packet[length],0xa5);assert_eq!(length,(u64::from(config.bitrate())*config.duration().samples() as u64/384000) as usize);
        let parsed=Packet::parse(&packet[..length])?;assert_eq!(parsed.toc().mode(),Mode::Silk);assert_eq!(parsed.samples(48000)?,config.duration().samples());
        let (first_fec,any_fec,active,kind)=wire_flags(&parsed)?;assert_eq!(first_fec,encoder.has_fec());all_active&=active;
        if original.is_none() {assert!(active,"{name} packet{index} changed active VAD");assert_ne!(kind,1);}
        assert_eq!(no_alloc(||decoder.decode(&packet[..length],&mut output))?,config.duration().samples());assert_eq!(decoder.final_range(),encoder.final_range());assert!(output.iter().all(|x|x.is_finite()));
        if carrier && index==6 {
            carrier_fec=first_fec;
            let outcome=no_alloc(||recovery.recover(&packet[..length],config.duration().samples(),&mut recovered))?;
            recovery_fec=outcome==Recovery::Fec;assert_eq!(recovery_fec,first_fec);assert!(recovered.iter().all(|x|x.is_finite()));
        }
        if !carrier || index!=5 {assert_eq!(no_alloc(||recovery.decode(&packet[..length],&mut recovered))?,config.duration().samples());assert_eq!(recovery.final_range(),encoder.final_range());}
        let flags=(u32::from(first_fec)<<1)|(u32::from(any_fec)<<2)|(u32::from(active)<<3)|(u32::from(carrier&&index==6)<<4);
        for n in [length as u32,encoder.final_range(),config.duration().samples() as u32,flags,config.channels() as u32] {word(&mut packets,n);}packets.write_all(&packet[..length])?;
        word(&mut pcm,output.len() as u32);for sample in &output {word(&mut pcm,sample.to_bits());}
        writeln!(events,"{{\"index\":{index},\"bytes\":{length},\"range\":{},\"first_fec\":{first_fec},\"any_fec\":{any_fec},\"all_vad_active\":{active},\"first_mono_kind\":{kind},\"allocations\":0}}",encoder.final_range())?;
        fec+=usize::from(first_fec);accepted+=1;
    }
    let failed=failed.map_or("null".to_string(),|x|x.to_string());
    writeln!(summary,"{{\"name\":\"{name}\",\"input_rate\":{},\"bitrate\":{},\"channels\":{},\"samples\":{},\"fec\":{},\"carrier\":{carrier},\"planned\":{limit},\"accepted\":{accepted},\"failed_at\":{failed},\"error\":\"{error}\",\"fec_packets\":{fec},\"carrier_fec\":{carrier_fec},\"recovery_fec\":{recovery_fec},\"all_vad_active\":{all_active},\"api_allocations\":0}}",config.sample_rate(),config.bitrate(),config.channels(),config.duration().samples(),config.fec())?;
    Ok(())
}
fn run_all(directory:&str)->Result<(),Box<dyn Error>> {
    let path=Path::new(directory);fs::create_dir_all(path)?;let mut summary=File::create(path.join("streams.jsonl"))?;
    for (bw,label,native) in [(Bandwidth::Narrowband,"nb",8000),(Bandwidth::Wideband,"wb",16000)] {
        for rate in [48000,native] {
            for channels in [1,2] {
                for duration in [FrameDuration::Ms10,FrameDuration::Ms20,FrameDuration::Ms40,FrameDuration::Ms60,FrameDuration::Ms80,FrameDuration::Ms100,FrameDuration::Ms120] {
                    let bits=12000*channels as u32;let name=format!("clip-{label}-{}ms-{channels}ch-input{rate}-{bits}",duration.samples()/48);
                    run(path,&name,config(rate,bits,channels,duration,bw,false),false,None,&mut summary)?;
                }
            }
        }
    }
    // Exact existing refusal reproductions are functional regressions, not tuning data.
    if std::env::var_os("SPARSE_RUN_KNOWN_REFUSALS").is_some() {
    let nb=config(48000,12000,1,FrameDuration::Ms10,Bandwidth::Narrowband,false);
    let original:Vec<f32>=(0..96*480).map(|n|match(n/24+n/480)%8{0|1=>1.0,2|3=>-1.0,4=>f32::from_bits(0x3f7fffff),5=>-f32::from_bits(0x3f7fffff),6=>0.0,_=>-0.0}).collect();
    run(path,"known-nb-first-refusal",nb,false,Some(&original),&mut summary)?;
    let wb=config(48000,6000,1,FrameDuration::Ms20,Bandwidth::Wideband,false);
    let values=fs::read("/tmp/snacka-opus-goal-speech-diagnostics-20261015/cen2-fjam-b.input.f32")?;
    let original:Vec<f32>=values.chunks_exact(4).map(|v|f32::from_le_bytes(v.try_into().unwrap())).collect();
    run(path,"known-wb-frame22-refusal",wb,false,Some(&original),&mut summary)?;
    }
    run(path,"fec-carrier-nb10-20k",config(48000,20000,1,FrameDuration::Ms10,Bandwidth::Narrowband,true),true,None,&mut summary)?;
    Ok(())
}
fn main()->Result<(),Box<dyn Error>> {
    let directory=std::env::args().nth(1).ok_or("output directory required")?;
    let result=std::thread::Builder::new().name("bounded-one-mib-proof".into()).stack_size(1<<20).spawn(move||run_all(&directory).map_err(|e|e.to_string()))?.join().unwrap();
    result.map_err(Into::into)
}
