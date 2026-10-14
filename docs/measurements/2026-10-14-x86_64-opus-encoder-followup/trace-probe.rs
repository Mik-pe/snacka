use snacka::opus::{Application, Bandwidth, Decoder, Encoder, EncoderConfig, FrameDuration, Mode, RateControl};
use std::{env, hint::black_box, time::Instant};

fn main() {
    let args: Vec<_> = env::args().skip(1).collect();
    let mode = match args[0].as_str() { "silk" => Mode::Silk, "hybrid" => Mode::Hybrid, "celt" => Mode::Celt, _ => panic!("mode") };
    let bandwidth = match args[1].as_str() { "NB" => Bandwidth::Narrowband, "MB" => Bandwidth::Mediumband, "WB" => Bandwidth::Wideband, "SWB" => Bandwidth::Superwideband, "FB" => Bandwidth::Fullband, _ => panic!("bandwidth") };
    let duration = match args[2].as_str() { "2.5" => FrameDuration::Ms2_5, "5" => FrameDuration::Ms5, "10" => FrameDuration::Ms10, "20" => FrameDuration::Ms20, "40" => FrameDuration::Ms40, "60" => FrameDuration::Ms60, _ => panic!("duration") };
    let channels: usize = args[3].parse().unwrap();
    let frames: usize = args[4].parse().unwrap();
    let batches: usize = args[5].parse().unwrap();
    let signal = args.get(6).map(String::as_str).unwrap_or("voice");
    let mono_rate = match (mode, bandwidth) { (Mode::Silk, Bandwidth::Narrowband) => 12000, (Mode::Silk, Bandwidth::Mediumband) => 16000, (Mode::Silk, _) => 24000, _ => 32000 };
    let bitrate = ((mono_rate * channels * duration.samples() + 192000) / 384000 * 384000 / duration.samples()) as u32;
    let config = EncoderConfig::new(bitrate, channels, duration, bandwidth, if mode == Mode::Celt {Application::LowDelay} else {Application::Voice}).unwrap().with_mode(mode).unwrap().with_rate_control(RateControl::Constant);
    let mut random = 730193u32;
    let inputs: Vec<Vec<f32>> = (0..64).map(|frame| (0..duration.samples()).flat_map(|index| {
        let time = (frame * duration.samples() + index) as f64 / 48000.0;
        let voice = 0.3 * (std::f64::consts::TAU * (137.0 * time + 4.0 * time * time)).sin();
        (0..channels).map(move |channel| (voice * (1.0 - channel as f64 * 0.2)) as f32)
    }).map(|sample| {
        random ^= random << 13; random ^= random >> 17; random ^= random << 5;
        match signal { "silence" => 0.0, "noise" => (random as f64 / u32::MAX as f64 - 0.5) as f32 * 0.7, "transient" => if random % 127 == 0 {0.7} else {0.0}, _ => sample }
    }).collect()).collect();
    let mut encoder = Encoder::new(config).unwrap();
    let mut decoder = Decoder::new(channels).unwrap();
    let mut packet = [0; 8000];
    let mut output = vec![0.0; duration.samples() * channels];
    let mut packet_hash = 0xcbf29ce484222325u64;
    let mut pcm_hash = packet_hash;
    for frame in 0..frames {
        let len = encoder.encode(&inputs[frame % 64], &mut packet).unwrap();
        packet_hash = (packet_hash ^ len as u64).wrapping_mul(0x100000001b3);
        for &byte in &packet[..len] { packet_hash = (packet_hash ^ u64::from(byte)).wrapping_mul(0x100000001b3); }
        decoder.decode(&packet[..len], &mut output).unwrap();
        for &sample in &output { pcm_hash = (pcm_hash ^ u64::from(sample.to_bits())).wrapping_mul(0x100000001b3); }
    }
    let mut times = Vec::with_capacity(batches);
    for _ in 0..batches {
        let start = Instant::now();
        for frame in 0..frames {
            let len = encoder.encode(black_box(&inputs[frame % 64]), black_box(&mut packet)).unwrap();
            black_box(&packet[..len]);
        }
        times.push(start.elapsed().as_nanos() as f64 / frames as f64);
    }
    times.sort_by(f64::total_cmp);
    let p50 = if batches == 0 {0.0} else {times[(batches-1)/2]};
    println!("{{\"packet_hash\":\"{packet_hash:016x}\",\"pcm_hash\":\"{pcm_hash:016x}\",\"frames\":{frames},\"batches\":{batches},\"p50_ns_per_frame\":{p50}}}");
}
