use snacka::opus::{Decoder,Packet};
use std::{error::Error,fs::{self,File},io::Write,path::Path};
fn main()->Result<(),Box<dyn Error>> {
    let input=std::env::args().nth(1).ok_or("packet directory required")?;
    let output=std::env::args().nth(2).ok_or("output directory required")?;
    let input=Path::new(&input);let output=Path::new(&output);fs::create_dir_all(output)?;
    for (label,native) in [("nb",8000),("wb",16000)] {
        let data=fs::read(input.join(format!("{label}.packets")))?;
        for rate in [native,48000] {
            let mut decoder=Decoder::with_sample_rate(1,rate)?;
            let mut pcm=File::create(output.join(format!("{label}.owned-{rate}.f32")))?;
            let mut trace=File::create(output.join(format!("{label}.owned-{rate}.jsonl")))?;
            let mut offset=0;let mut index=0;let mut decoded=vec![0.0;rate as usize*120/1000];
            while offset<data.len() {
                let length=u32::from_le_bytes(data[offset..offset+4].try_into().unwrap()) as usize;offset+=4;
                let packet=&data[offset..offset+length];offset+=length;
                let parsed=Packet::parse(packet)?;assert_eq!(parsed.toc().channels(),1);
                let expected=parsed.samples(rate)?;
                let samples=decoder.decode(packet,&mut decoded)?;assert_eq!(samples,expected);
                assert!(decoded[..samples].iter().all(|x|x.is_finite()));
                for sample in &decoded[..samples] {pcm.write_all(&sample.to_bits().to_le_bytes())?;}
                writeln!(trace,"{{\"index\":{index},\"samples\":{samples},\"range\":{}}}",decoder.final_range())?;
                index+=1;
            }
            assert_eq!(offset,data.len());
            println!("{{\"label\":\"{label}\",\"rate\":{rate},\"packets\":{index}}}");
        }
    }
    Ok(())
}
