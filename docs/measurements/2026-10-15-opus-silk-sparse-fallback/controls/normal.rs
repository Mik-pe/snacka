use snacka::opus::{Application, Bandwidth, Decoder, Encoder, EncoderConfig, FrameDuration, Mode, Packet, RateControl, silk};
use std::{error::Error, fs::{self, File}, io::Write, path::Path};

#[derive(Clone, Copy)]
struct Stage { config: EncoderConfig, packets: usize, reset: bool, queued: bool }
fn stage(config: EncoderConfig, packets: usize) -> Stage { Stage { config, packets, reset: false, queued: false } }
fn config(rate: u32, bitrate: u32, channels: usize, duration: FrameDuration, mode: Mode, bandwidth: Bandwidth, fec: bool) -> EncoderConfig {
    EncoderConfig::new(bitrate, channels, duration, bandwidth, Application::Voice).unwrap()
        .with_mode(mode).unwrap().with_sample_rate(rate).unwrap()
        .with_rate_control(RateControl::Constant).with_fec(fec).unwrap()
}
fn outside(c: EncoderConfig) -> bool {
    c.mode() != Mode::Silk || c.channels() != 1 || c.duration() != FrameDuration::Ms20
        || c.sample_rate() != 48_000 || c.bandwidth() != Bandwidth::Wideband
        || c.bitrate() > 16_000 || c.fec()
}
fn pcm(samples: usize, channels: usize, rate: u32, start_seconds: f64) -> Vec<f32> {
    (0..samples * channels).map(|position| {
        let t = start_seconds + (position / channels) as f64 / rate as f64;
        let ch = (position % channels) as f64;
        let phase = std::f64::consts::TAU * (137.0 * t + 3.8 * (std::f64::consts::TAU * 1.3 * t).sin()) + ch * 0.43;
        (0.16 * phase.sin() + 0.055 * (2.0 * phase).sin() + 0.024 * (7.0 * phase + ch).sin()) as f32
    }).collect()
}
fn word(file: &mut File, value: u32) { file.write_all(&value.to_le_bytes()).unwrap(); }
fn record(packet_file: &mut File, pcm_file: &mut File, packet: &[u8], range: u32, samples: usize, flags: u32, channels: usize, output: &[f32]) {
    for value in [packet.len() as u32, range, samples as u32, flags, channels as u32] { word(packet_file, value); }
    packet_file.write_all(packet).unwrap();
    word(pcm_file, output.len() as u32);
    for sample in output { pcm_file.write_all(&sample.to_bits().to_le_bytes()).unwrap(); }
}
fn run(path: &Path, name: &str, stages: &[Stage], summary: &mut File) -> Result<(), Box<dyn Error>> {
    assert!(name == "wb12k-20ms-ordinary" || stages.iter().all(|s| outside(s.config)));
    let mut encoder = Encoder::new(stages[0].config)?;
    // A fixed stereo receiver also exercises mono/stereo reconfiguration.
    let mut decoder = Decoder::new(2)?;
    let mut packets = File::create(path.join(format!("{name}.packets")))?;
    let mut decoded = File::create(path.join(format!("{name}.pcm")))?;
    let mut buffer = vec![0xa5; 32_768];
    let mut output = vec![0.0; 5760 * 2];
    let mut total = 0;
    let mut fec = 0;
    let mut transitions = 0;
    let mut seconds = 0.0;
    for (index, current) in stages.iter().enumerate() {
        if index > 0 && !stages[index-1].queued { encoder.reconfigure(current.config)?; }
        assert_eq!(encoder.config(), current.config);
        if current.reset { encoder.reset(); decoder.reset(); }
        for packet_index in 0..current.packets {
            if current.queued && packet_index + 1 == current.packets {
                encoder.prepare_reconfigure(stages[index+1].config)?;
                assert_eq!(encoder.config(), current.config);
            }
            let input = pcm(current.config.input_samples(), current.config.channels(), current.config.sample_rate(), seconds);
            let length = encoder.encode(&input, &mut buffer)?;
            let packet = Packet::parse(&buffer[..length])?;
            assert_eq!(packet.toc().mode(), current.config.mode());
            assert_eq!(packet.toc().bandwidth(), current.config.bandwidth());
            assert_eq!(usize::from(packet.toc().channels()), current.config.channels());
            let samples = current.config.duration().samples();
            assert_eq!(packet.samples(48_000)?, samples);
            assert_eq!(packet.samples(current.config.sample_rate())?, current.config.input_samples());
            assert_eq!(length, (u64::from(current.config.bitrate()) * samples as u64 / 384_000) as usize);
            assert_eq!(decoder.decode(&buffer[..length], &mut output)?, samples);
            assert_eq!(decoder.final_range(), encoder.final_range());
            assert!(output[..samples*2].iter().all(|x| x.is_finite()));
            let flags = u32::from(current.reset && packet_index == 0)
                | (u32::from(encoder.has_fec()) << 1) | (u32::from(encoder.has_transition_redundancy()) << 2);
            record(&mut packets, &mut decoded, &buffer[..length], encoder.final_range(), samples, flags, 2, &output[..samples*2]);
            fec += usize::from(encoder.has_fec());
            transitions += usize::from(encoder.has_transition_redundancy());
            seconds += samples as f64 / 48_000.0;
            total += 1;
        }
    }
    writeln!(summary, "{{\"name\":\"{name}\",\"kind\":\"opus\",\"packets\":{total},\"fec_packets\":{fec},\"transition_packets\":{transitions}}}")?;
    Ok(())
}
fn standalone(path: &Path, summary: &mut File) -> Result<(), Box<dyn Error>> {
    for channels in [1usize, 2] {
        let name = format!("standalone-wb12k-{channels}ch-reset-fec-toggle");
        let mut encoder = silk::SilkEncoder::new();
        let mut decoder = silk::SilkDecoder::new();
        let mut packets = File::create(path.join(format!("{name}.packets")))?;
        let mut decoded = File::create(path.join(format!("{name}.pcm")))?;
        let frame = silk::Frame { bandwidth: silk::Bandwidth::Wide, duration: silk::FrameDuration::Ms20 };
        let mut packet = [0; 512];
        let mut output = vec![0.0; 320 * channels];
        let mut fec = 0;
        for index in 0..18 {
            let reset = index == 9;
            if reset { encoder.reset(); decoder.reset(); }
            encoder.set_fec((4..8).contains(&index) || index >= 13);
            let input = pcm(320, channels, 16_000, index as f64 * 0.02);
            let mut range = snacka::entropy::Encoder::new(&mut packet);
            let statistics = encoder.encode_layer(&mut range, frame, silk::PacketDuration::Ms20, channels as u8, &input, 224)?;
            let final_range = range.range();
            let length = range.finish()?;
            let mut range = snacka::entropy::Decoder::new(&packet[..length]);
            assert_eq!(decoder.decode(&mut range, frame, silk::PacketDuration::Ms20, channels as u8, &mut output)?, 320);
            assert_eq!(range.range(), final_range);
            assert!(output.iter().all(|x| x.is_finite()));
            let flags = u32::from(reset) | (u32::from(statistics.redundancy) << 1);
            record(&mut packets, &mut decoded, &packet[..length], final_range, 320, flags, channels, &output);
            fec += usize::from(statistics.redundancy);
        }
        writeln!(summary, "{{\"name\":\"{name}\",\"kind\":\"silk-layer\",\"packets\":18,\"fec_packets\":{fec},\"transition_packets\":0}}")?;
    }
    Ok(())
}
fn main() -> Result<(), Box<dyn Error>> {
    let target = std::env::args().nth(1).ok_or("output directory required")?;
    let path = Path::new(&target); fs::create_dir_all(path)?;
    let mut summary = File::create(path.join("streams.jsonl"))?;
    let wb = |rate, bits, channels, duration, fec| config(rate, bits, channels, duration, Mode::Silk, Bandwidth::Wideband, fec);
    for rate in [8_000, 12_000, 16_000, 24_000] {
        run(path, &format!("wb12k-20ms-input{rate}"), &[stage(wb(rate, 12_000, 1, FrameDuration::Ms20, false), 12)], &mut summary)?;
    }
    run(path, "wb12k-20ms-ordinary", &[stage(wb(48_000, 12_000, 1, FrameDuration::Ms20, false), 18)], &mut summary)?;
    run(path, "wb12k-20ms-stereo", &[stage(wb(48_000, 12_000, 2, FrameDuration::Ms20, false), 18)], &mut summary)?;
    for duration in [FrameDuration::Ms40, FrameDuration::Ms60, FrameDuration::Ms80, FrameDuration::Ms100, FrameDuration::Ms120] {
        run(path, &format!("wb12k-packed{}ms", duration.samples()/48), &[stage(wb(48_000, 12_000, 1, duration, false), 8)], &mut summary)?;
    }
    for bandwidth in [Bandwidth::Superwideband, Bandwidth::Fullband] {
        run(path, &format!("hybrid32k-{bandwidth:?}"), &[stage(config(48_000, 32_000, 1, FrameDuration::Ms20, Mode::Hybrid, bandwidth, false), 12)], &mut summary)?;
    }
    run(path, "wb12k-10ms", &[stage(wb(48_000, 12_000, 1, FrameDuration::Ms10, false), 18)], &mut summary)?;
    for bits in [16_800, 24_000, 32_000] {
        run(path, &format!("wb{bits}-20ms"), &[stage(wb(48_000, bits, 1, FrameDuration::Ms20, false), 18)], &mut summary)?;
    }
    run(path, "wb12k-20ms-fec", &[stage(wb(48_000, 12_000, 1, FrameDuration::Ms20, true), 18)], &mut summary)?;
    // Toggle FEC while the bitrate independently remains outside the policy.
    run(path, "wb24k-fec-toggle-reset", &[
        stage(wb(48_000, 24_000, 1, FrameDuration::Ms20, false), 6),
        stage(wb(48_000, 24_000, 1, FrameDuration::Ms20, true), 8),
        Stage { reset: true, ..stage(wb(48_000, 24_000, 1, FrameDuration::Ms20, false), 6) },
        stage(wb(48_000, 24_000, 1, FrameDuration::Ms20, true), 8),
    ], &mut summary)?;
    // Every configuration stays outside scope, including old/new queued geometry.
    run(path, "rates-modes-duration-channels-queued-reset", &[
        Stage { queued: true, ..stage(wb(16_000, 12_000, 1, FrameDuration::Ms20, false), 6) },
        stage(wb(24_000, 12_000, 1, FrameDuration::Ms20, false), 6),
        Stage { queued: true, ..stage(config(48_000, 32_000, 1, FrameDuration::Ms20, Mode::Hybrid, Bandwidth::Fullband, false), 6) },
        stage(config(48_000, 32_000, 1, FrameDuration::Ms20, Mode::Celt, Bandwidth::Fullband, false), 6),
        stage(wb(48_000, 12_000, 1, FrameDuration::Ms10, false), 6),
        Stage { reset: true, queued: true, ..stage(wb(48_000, 12_000, 2, FrameDuration::Ms20, false), 6) },
        stage(wb(48_000, 12_000, 1, FrameDuration::Ms60, false), 4),
        Stage { queued: true, ..stage(wb(48_000, 12_000, 1, FrameDuration::Ms20, true), 6) },
        stage(wb(8_000, 12_000, 1, FrameDuration::Ms20, false), 6),
    ], &mut summary)?;
    standalone(path, &mut summary)?;
    Ok(())
}
