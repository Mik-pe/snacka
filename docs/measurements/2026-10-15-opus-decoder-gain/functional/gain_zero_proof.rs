use snacka::opus::{Decoder, Recovery, repacketize::Repacketizer};
use std::{env, fs};

fn hash(mut digest: u64, value: u32) -> u64 {
    for byte in value.to_le_bytes() {
        digest ^= u64::from(byte);
        digest = digest.wrapping_mul(0x100000001b3);
    }
    digest
}

fn record(decoder: &Decoder, output: &[f32], samples: usize, digest: &mut [u64; 2]) {
    let count = samples * decoder.channels();
    for sample in &output[..count] {
        assert!(sample.is_finite());
        digest[0] = hash(digest[0], sample.to_bits());
    }
    assert!(output[count..].iter().all(|&v| v.to_bits() == 0x7fc01234));
    digest[1] = hash(digest[1], decoder.final_range());
    digest[1] = hash(digest[1], samples as u32);
}

fn main() {
    for path in env::args().skip(1) {
        let data = fs::read(&path).unwrap();
        let mut cursor = 0;
        let mut packets = Vec::new();
        while cursor < data.len() {
            let length = u32::from_le_bytes(data[cursor..cursor + 4].try_into().unwrap()) as usize;
            cursor += 4;
            packets.push(&data[cursor..cursor + length]);
            cursor += length;
        }
        let mut repacketizer = Repacketizer::new();
        for packet in &packets[..6] {
            repacketizer.push(packet).unwrap();
        }
        let mut long = vec![0; 8192];
        let length = repacketizer.write(0..6, &mut long).unwrap();
        long.truncate(length);
        for rate in [8000, 12000, 16000, 24000, 48000] {
            for channels in [1, 2] {
                let mut decoder = Decoder::with_sample_rate(channels, rate).unwrap();
                let mut output = vec![f32::from_bits(0x7fc01234); rate as usize * 120 / 1000 * channels + 9];
                let mut digest = [0xcbf29ce484222325; 2];
                let samples = rate as usize / 50;
                let mut actual_fec = 0;
                let mut operations = 0;
                for (index, packet) in packets.iter().enumerate() {
                    if index == 2 { continue; }
                    if index == 3 {
                        output.fill(f32::from_bits(0x7fc01234));
                        actual_fec += usize::from(decoder.recover(packet, samples * 2, &mut output).unwrap() == Recovery::Fec);
                        record(&decoder, &output, samples * 2, &mut digest);
                        operations += 1;
                    }
                    if index == 5 || index == 8 {
                        output.fill(f32::from_bits(0x7fc01234));
                        let loss = if index == 5 { rate as usize / 400 } else { samples };
                        assert_eq!(decoder.conceal(loss, &mut output).unwrap(), loss);
                        record(&decoder, &output, loss, &mut digest);
                        operations += 1;
                    }
                    output.fill(f32::from_bits(0x7fc01234));
                    assert_eq!(decoder.decode(packet, &mut output).unwrap(), samples);
                    record(&decoder, &output, samples, &mut digest);
                    operations += 1;
                }
                decoder.reset();
                output.fill(f32::from_bits(0x7fc01234));
                assert_eq!(decoder.decode(&long, &mut output).unwrap(), samples * 6);
                record(&decoder, &output, samples * 6, &mut digest);
                operations += 1;
                println!("{{\"file\":{:?},\"rate\":{rate},\"channels\":{channels},\"operations\":{operations},\"actual_fec\":{actual_fec},\"pcm_hash\":\"{:016x}\",\"range_sample_hash\":\"{:016x}\"}}", path, digest[0], digest[1]);
            }
        }
    }
}
