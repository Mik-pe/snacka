// SPDX-License-Identifier: MIT
//! Offline 48 kHz little-endian interleaved f32 PCM and Ogg Opus file conversion.
//!
//! cargo run --release --features opus --example opus_roundtrip -- \
//!     input.f32 output.opus decoded.f32 [bitrate] [channels] [frame_ms]
//!
//! Subcommands also support separate encode and decode operations. Frame sizes
//! are 2.5, 5, 10 or 20 ms; defaults are 32,000 bits/s, mono and 20 ms. Audio is
//! encoded by Snacka's CELT encoder. Container trimming preserves input length;
//! audio compression is lossy, so output samples are not bit-identical to input.

use snacka::opus::ogg::{OggReader, OggWriter, OpusHead, OpusTags};
use snacka::opus::{
    Bandwidth, CeltConfig, CeltEncoder, CeltFrameDuration, Decoder as AudioDecoder,
};
use std::error::Error;
use std::ffi::{OsStr, OsString};
use std::fs::{self, File};
use std::io::{self, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, Box<dyn Error>>;

const SAMPLE_RATE: u32 = 48_000;
const PRE_SKIP: u16 = 120;
const SERIAL: u32 = 0x534e_4b41;
const MAX_PACKET_BYTES: usize = snacka::opus::packet::MAX_FRAME_BYTES + 1;
const MAX_DECODED_SAMPLES: usize = snacka::opus::packet::MAX_PACKET_SAMPLES_48KHZ * 2;

const USAGE: &str = "usage:
  opus_roundtrip input.f32 output.opus decoded.f32 [bitrate] [channels] [frame_ms]
  opus_roundtrip roundtrip input.f32 output.opus decoded.f32 [bitrate] [channels] [frame_ms]
  opus_roundtrip encode input.f32 output.opus [bitrate] [channels] [frame_ms]
  opus_roundtrip decode input.opus output.f32

PCM is little-endian interleaved f32 at 48 kHz. Defaults: bitrate 32000,
channels 1, frame_ms 20. Supported frame_ms: 2.5, 5, 10, 20.";

#[derive(Clone, Copy, Debug)]
struct EncodingStats {
    samples: usize,
    packets: usize,
    packet_bytes: usize,
}

#[derive(Clone, Copy, Debug)]
struct DecodingStats {
    samples: usize,
    channels: usize,
    packets: usize,
}

fn invalid(message: impl Into<String>) -> Box<dyn Error> {
    io::Error::new(io::ErrorKind::InvalidInput, message.into()).into()
}

fn text(value: &OsStr) -> Result<&str> {
    value
        .to_str()
        .ok_or_else(|| invalid("numeric options must be UTF-8 text"))
}

fn config(options: &[OsString]) -> Result<CeltConfig> {
    if options.len() > 3 {
        return Err(invalid(USAGE));
    }
    let bitrate = match options.first() {
        Some(value) => text(value)?.parse()?,
        None => 32_000,
    };
    let channels = match options.get(1) {
        Some(value) => text(value)?.parse()?,
        None => 1,
    };
    let duration = match options
        .get(2)
        .map(|value| text(value))
        .transpose()?
        .unwrap_or("20")
    {
        "2.5" => CeltFrameDuration::Ms2_5,
        "5" => CeltFrameDuration::Ms5,
        "10" => CeltFrameDuration::Ms10,
        "20" => CeltFrameDuration::Ms20,
        _ => return Err(invalid("frame_ms must be 2.5, 5, 10 or 20")),
    };
    Ok(CeltConfig::new(
        bitrate,
        duration,
        channels,
        Bandwidth::Fullband,
    )?)
}

fn pcm(bytes: &[u8], channels: usize) -> Result<Vec<f32>> {
    if !(1..=2).contains(&channels) {
        return Err(invalid("channels must be 1 or 2"));
    }
    let (samples, trailing) = bytes.as_chunks::<4>();
    if !trailing.is_empty() || !samples.len().is_multiple_of(channels) {
        return Err(invalid(
            "input must contain complete interleaved f32 channel frames",
        ));
    }
    let mut result = Vec::with_capacity(samples.len());
    for &sample in samples {
        let value = f32::from_le_bytes(sample);
        if !value.is_finite() || !(-1.0..=1.0).contains(&value) {
            return Err(invalid(
                "input PCM samples must be finite and within [-1, 1]",
            ));
        }
        result.push(value);
    }
    Ok(result)
}

fn encode<W: Write>(input: &[f32], sink: W, config: CeltConfig) -> Result<(W, EncodingStats)> {
    let channels = config.channels();
    if !input.len().is_multiple_of(channels)
        || input
            .iter()
            .any(|sample| !sample.is_finite() || !(-1.0..=1.0).contains(sample))
    {
        return Err(invalid("invalid interleaved PCM input"));
    }
    let samples = input.len() / channels;
    let frame_samples = config.duration().samples();
    let packets = samples
        .checked_add(usize::from(PRE_SKIP))
        .ok_or_else(|| invalid("input duration is too large"))?
        .div_ceil(frame_samples);
    let head = OpusHead {
        channels: channels as u8,
        pre_skip: PRE_SKIP,
        input_sample_rate: SAMPLE_RATE,
        output_gain_q8: 0,
    };
    let tags = OpusTags {
        vendor: format!("snacka {}", env!("CARGO_PKG_VERSION")),
        comments: vec![format!("ENCODER=Snacka CELT {} bits/s", config.bitrate())],
    };
    let mut writer = OggWriter::new(sink, SERIAL, head, &tags)?;
    let mut encoder = CeltEncoder::new(config);
    let mut packet = [0u8; MAX_PACKET_BYTES];
    let mut frame = [0.0f32; 960 * 2];
    let mut packet_bytes = 0;
    for index in 0..packets {
        let frame = &mut frame[..frame_samples * channels];
        frame.fill(0.0);
        let start = index * frame.len();
        if start < input.len() {
            let available = (input.len() - start).min(frame.len());
            frame[..available].copy_from_slice(&input[start..start + available]);
        }
        let length = encoder.encode(frame, &mut packet)?;
        writer.write_packet(&packet[..length])?;
        packet_bytes += length;
    }
    let mut sink = writer.finish(samples as u64)?;
    sink.flush()?;
    Ok((
        sink,
        EncodingStats {
            samples,
            packets,
            packet_bytes,
        },
    ))
}

#[cfg(test)]
fn decode<R: Read, W: Write>(source: R, sink: W) -> Result<(W, DecodingStats)> {
    decode_reader(OggReader::new(source)?, sink)
}

fn decode_reader<R: Read, W: Write>(
    mut reader: OggReader<R>,
    mut sink: W,
) -> Result<(W, DecodingStats)> {
    let channels = usize::from(reader.head().channels);
    let gain = reader.head().gain_factor();
    let mut decoder = AudioDecoder::new(channels)?;
    let mut output = [0.0f32; MAX_DECODED_SAMPLES];
    let mut samples = 0;
    let mut packets = 0;
    while let Some(packet) = reader.next_packet()? {
        let count = decoder.decode(&packet.data, &mut output)?;
        if count != packet.samples_48khz
            || packet.keep.start > packet.keep.end
            || packet.keep.end > count
        {
            return Err(invalid(
                "decoded packet duration differs from container framing",
            ));
        }
        for &sample in &output[packet.keep.start * channels..packet.keep.end * channels] {
            let value = sample * gain;
            if !value.is_finite() {
                return Err(invalid("decoder produced non-finite PCM"));
            }
            sink.write_all(&value.to_le_bytes())?;
        }
        samples += packet.keep.len();
        packets += 1;
    }
    sink.flush()?;
    Ok((
        sink,
        DecodingStats {
            samples,
            channels,
            packets,
        },
    ))
}

fn resolved_path(path: &Path) -> io::Result<PathBuf> {
    if path.exists() {
        return path.canonicalize();
    }
    let parent = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    let file = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "output needs a file name"))?;
    Ok(parent.canonicalize()?.join(file))
}

fn distinct(paths: &[&Path]) -> Result<()> {
    let resolved = paths
        .iter()
        .map(|path| resolved_path(path))
        .collect::<io::Result<Vec<_>>>()?;
    for (index, path) in resolved.iter().enumerate() {
        if resolved[..index].contains(path) {
            return Err(invalid(
                "input, Opus output and PCM output must use distinct file paths",
            ));
        }
    }
    Ok(())
}

fn encode_file(input: &Path, output: &Path, config: CeltConfig) -> Result<EncodingStats> {
    let data = pcm(&fs::read(input)?, config.channels())?;
    let (_, stats) = encode(&data, BufWriter::new(File::create(output)?), config)?;
    Ok(stats)
}

fn decode_file(input: &Path, output: &Path) -> Result<DecodingStats> {
    let source = BufReader::new(File::open(input)?);
    // Validate mandatory headers before creating the requested output file.
    let reader = OggReader::new(source)?;
    let (_, stats) = decode_reader(reader, BufWriter::new(File::create(output)?))?;
    Ok(stats)
}

fn run(arguments: &[OsString]) -> Result<()> {
    if arguments.is_empty() || matches!(arguments[0].to_str(), Some("--help" | "-h")) {
        println!("{USAGE}");
        return Ok(());
    }
    let mode = arguments[0].to_str();
    let (command, values) = match mode {
        Some(command @ ("encode" | "decode" | "roundtrip")) => (command, &arguments[1..]),
        _ => ("roundtrip", arguments),
    };
    match command {
        "encode" if (2..=5).contains(&values.len()) => {
            let config = config(&values[2..])?;
            let input = Path::new(&values[0]);
            let output = Path::new(&values[1]);
            distinct(&[input, output])?;
            let stats = encode_file(input, output, config)?;
            println!(
                "{} samples/channel encoded into {} Opus packets ({} packet bytes)",
                stats.samples, stats.packets, stats.packet_bytes
            );
        }
        "decode" if values.len() == 2 => {
            let input = Path::new(&values[0]);
            let output = Path::new(&values[1]);
            distinct(&[input, output])?;
            let stats = decode_file(input, output)?;
            println!(
                "{} samples/channel, {} channels decoded from {} Opus packets at 48 kHz",
                stats.samples, stats.channels, stats.packets
            );
        }
        "roundtrip" if (3..=6).contains(&values.len()) => {
            let config = config(&values[3..])?;
            let input = Path::new(&values[0]);
            let opus = Path::new(&values[1]);
            let output = Path::new(&values[2]);
            distinct(&[input, opus, output])?;
            let encoded = encode_file(input, opus, config)?;
            let decoded = decode_file(opus, output)?;
            if decoded.samples != encoded.samples || decoded.channels != config.channels() {
                return Err(invalid(
                    "roundtrip failed to preserve the original PCM sample count",
                ));
            }
            println!(
                "{} samples/channel preserved; {} channels, {} packets, {} Opus packet bytes",
                decoded.samples, decoded.channels, encoded.packets, encoded.packet_bytes
            );
        }
        _ => return Err(invalid(USAGE)),
    }
    Ok(())
}

fn main() -> Result<()> {
    run(&std::env::args_os().skip(1).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn conversion_preserves_short_and_partial_frame_lengths() {
        for channels in 1..=2 {
            for duration in [
                CeltFrameDuration::Ms2_5,
                CeltFrameDuration::Ms5,
                CeltFrameDuration::Ms10,
                CeltFrameDuration::Ms20,
            ] {
                let config =
                    CeltConfig::new(32_000, duration, channels, Bandwidth::Fullband).unwrap();
                for count in [0, 1, 119, 120, 121, 959, 960, 961] {
                    let input = vec![0.0; count * channels];
                    let (container, encoded) = encode(&input, Vec::new(), config).unwrap();
                    let (output, decoded) = decode(Cursor::new(container), Vec::new()).unwrap();
                    assert_eq!(encoded.samples, count);
                    assert_eq!(
                        encoded.packets,
                        (count + usize::from(PRE_SKIP)).div_ceil(duration.samples())
                    );
                    assert_eq!(decoded.samples, count);
                    assert_eq!(decoded.channels, channels);
                    assert_eq!(output.len(), count * channels * 4);
                    assert!(pcm(&output, channels).is_ok());
                }
            }
        }
    }

    #[test]
    fn raw_pcm_rejects_truncation_channel_misalignment_and_invalid_values() {
        assert!(pcm(&[0, 0, 0], 1).is_err());
        assert!(pcm(&0.0f32.to_le_bytes(), 2).is_err());
        assert!(pcm(&[], 0).is_err());
        assert!(pcm(&[], 3).is_err());
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -1.01, 1.01] {
            assert!(pcm(&value.to_le_bytes(), 1).is_err());
        }
        for value in [-1.0f32, 0.0, 1.0] {
            assert_eq!(pcm(&value.to_le_bytes(), 1).unwrap(), [value]);
        }
        assert_eq!(pcm(&[], 2).unwrap(), []);
    }

    #[test]
    fn options_defaults_and_domains_are_explicit() {
        assert_eq!(config(&[]).unwrap(), CeltConfig::default());
        for invalid in [
            vec!["bad"],
            vec!["32000", "0"],
            vec!["32000", "3"],
            vec!["32000", "1", "7.5"],
            vec!["32000", "1", "20", "extra"],
        ] {
            let options = invalid.into_iter().map(OsString::from).collect::<Vec<_>>();
            assert!(config(&options).is_err());
        }
        assert!(run(&[OsString::from("decode")]).is_err());
        assert!(run(&[OsString::from("encode")]).is_err());
        assert!(decode(Cursor::new(Vec::<u8>::new()), Vec::new()).is_err());
    }
}
