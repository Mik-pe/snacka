// SPDX-License-Identifier: MIT
//! Decode the official Opus `.bit` container using only Snacka's Rust decoder.
//! Record headers are big-endian packet length and expected final range.
//! Output is little-endian signed PCM16 at 48 kHz, rounded to nearest even and
//! saturated. Packet errors stop decoding; range mismatches retain complete PCM
//! so the separate official `opus_compare` utility can still measure quality.

use std::{
    env,
    fs::{self, File},
    io::{self, BufReader, BufWriter, Read, Write},
    path::{Path, PathBuf},
    process::ExitCode,
};

use snacka::opus::{Decoder, Toc};

const MAX_PACKET_BYTES: usize = 1_048_576;
const MAX_SAMPLES: usize = 5_760;

fn record(reader: &mut impl Read, packet: &mut [u8]) -> io::Result<Option<(usize, u32)>> {
    let mut header = [0; 8];
    if reader.read(&mut header[..1])? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut header[1..])?;
    let length = u32::from_be_bytes(header[..4].try_into().unwrap()) as usize;
    let range = u32::from_be_bytes(header[4..].try_into().unwrap());
    if length > packet.len() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "packet exceeds 1 MiB container limit",
        ));
    }
    reader.read_exact(&mut packet[..length])?;
    Ok(Some((length, range)))
}

fn json_string(value: &str) -> String {
    let mut escaped = String::from("\"");
    for character in value.chars() {
        match character {
            '\"' => escaped.push_str("\\\""),
            '\\' => escaped.push_str("\\\\"),
            '\n' => escaped.push_str("\\n"),
            '\r' => escaped.push_str("\\r"),
            '\t' => escaped.push_str("\\t"),
            control if control <= '\u{1f}' => {
                escaped.push_str(&format!("\\u{:04x}", control as u32))
            }
            other => escaped.push(other),
        }
    }
    escaped.push('"');
    escaped
}

fn failure(
    index: usize,
    offset: u64,
    packet: &[u8],
    expected: u32,
    actual: Option<u32>,
    error: &str,
) -> String {
    let toc = packet.first().copied().map(Toc::from_byte);
    let config = toc
        .map(|toc| toc.config().to_string())
        .unwrap_or_else(|| "null".into());
    let channels = toc
        .map(|toc| toc.channels().to_string())
        .unwrap_or_else(|| "null".into());
    let mode = toc
        .map(|toc| json_string(&format!("{:?}", toc.mode())))
        .unwrap_or_else(|| "null".into());
    let actual = actual
        .map(|range| range.to_string())
        .unwrap_or_else(|| "null".into());
    format!(
        "{{\"packet_index\":{index},\"offset\":{offset},\"bytes\":{},\"config\":{config},\"coded_channels\":{channels},\"mode\":{mode},\"expected_range\":{expected},\"actual_range\":{actual},\"error\":{}}}",
        packet.len(),
        json_string(error)
    )
}

fn decode(
    input: &Path,
    output: &Path,
    channels: usize,
) -> Result<bool, Box<dyn std::error::Error>> {
    if output.exists() && input.canonicalize()? == output.canonicalize()? {
        return Err("input and output must be different files".into());
    }
    let mut reader = BufReader::new(File::open(input)?);
    let mut writer = BufWriter::new(File::create(output)?);
    let mut decoder = Decoder::new(channels)?;
    let mut packet = vec![0; MAX_PACKET_BYTES];
    let mut pcm = vec![0.0; MAX_SAMPLES * channels];
    let mut encoded = vec![0; MAX_SAMPLES * channels * 2];
    let mut packets = 0usize;
    let mut samples = 0u64;
    let mut offset = 0u64;
    let mut mismatches = 0usize;
    let mut overshoots = 0u64;
    let mut peak = 0.0f32;
    let mut first = None;
    let mut complete = true;
    loop {
        let next = match record(&mut reader, &mut packet) {
            Ok(Some(record)) => record,
            Ok(None) => break,
            Err(error) => {
                first.get_or_insert_with(|| {
                    failure(
                        packets + 1,
                        offset,
                        &[],
                        0,
                        None,
                        &format!("container: {error}"),
                    )
                });
                complete = false;
                break;
            }
        };
        let (length, expected) = next;
        packets += 1;
        let bytes = &packet[..length];
        let count = match decoder.decode(bytes, &mut pcm) {
            Ok(count) if count <= MAX_SAMPLES => count,
            result => {
                let error = match result {
                    Err(error) => error.to_string(),
                    Ok(_) => "decoder exceeded output capacity".into(),
                };
                if first.is_none() {
                    first = Some(failure(packets, offset, bytes, expected, None, &error));
                    fs::write(output.with_extension("first-failure.opus"), bytes)?;
                }
                complete = false;
                break;
            }
        };
        let actual = decoder.final_range();
        if actual != expected {
            mismatches += 1;
            if first.is_none() {
                first = Some(failure(
                    packets,
                    offset,
                    bytes,
                    expected,
                    Some(actual),
                    "final range mismatch",
                ));
                fs::write(output.with_extension("first-failure.opus"), bytes)?;
            }
        }
        let frame = &pcm[..count * channels];
        if frame.iter().any(|sample| !sample.is_finite()) {
            first.get_or_insert_with(|| {
                failure(
                    packets,
                    offset,
                    bytes,
                    expected,
                    Some(actual),
                    "nonfinite PCM",
                )
            });
            complete = false;
            break;
        }
        let (pairs, _) = encoded.as_chunks_mut::<2>();
        for (sample, pair) in frame.iter().zip(pairs) {
            peak = peak.max(sample.abs());
            overshoots += u64::from(sample.abs() > 1.0);
            let value = (sample * 32_768.0)
                .round_ties_even()
                .clamp(-32_768.0, 32_767.0) as i16;
            pair.copy_from_slice(&value.to_le_bytes());
        }
        writer.write_all(&encoded[..frame.len() * 2])?;
        samples += count as u64;
        offset += 8 + length as u64;
    }
    writer.flush()?;
    let first = first.unwrap_or_else(|| "null".into());
    println!(
        "{{\"input\":{},\"output\":{},\"sample_rate\":48000,\"channels\":{channels},\"packets\":{packets},\"samples_per_channel\":{samples},\"complete\":{complete},\"range_mismatches\":{mismatches},\"peak\":{peak},\"clipped_samples\":{overshoots},\"conversion\":\"round_ties_even_clamp_i16\",\"first_failure\":{first}}}",
        json_string(&input.to_string_lossy()),
        json_string(&output.to_string_lossy())
    );
    Ok(complete && mismatches == 0)
}

fn main() -> ExitCode {
    let arguments: Vec<_> = env::args_os().collect();
    if arguments.len() != 4 {
        eprintln!("usage: opus_vectors INPUT.bit OUTPUT.dec CHANNELS(1|2)");
        return ExitCode::FAILURE;
    }
    let channels = arguments[3]
        .to_str()
        .and_then(|value| value.parse::<usize>().ok());
    let Some(channels @ (1 | 2)) = channels else {
        eprintln!("channels must be 1 or 2");
        return ExitCode::FAILURE;
    };
    match decode(
        &PathBuf::from(&arguments[1]),
        &PathBuf::from(&arguments[2]),
        channels,
    ) {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(error) => {
            eprintln!("opus_vectors: {error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_rejects_partial_headers_payloads_and_oversized_lengths() {
        let mut packet = [0; 4];
        assert_eq!(record(&mut &[][..], &mut packet).unwrap(), None);
        for length in 1..8 {
            assert_eq!(
                record(&mut &vec![0; length][..], &mut packet)
                    .unwrap_err()
                    .kind(),
                io::ErrorKind::UnexpectedEof
            );
        }
        let mut bytes = Vec::from(3u32.to_be_bytes());
        bytes.extend(0x1234_5678u32.to_be_bytes());
        bytes.extend([1, 2, 3]);
        assert_eq!(
            record(&mut &bytes[..10], &mut packet).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
        assert_eq!(
            record(&mut &bytes[..], &mut packet).unwrap(),
            Some((3, 0x1234_5678))
        );
        assert_eq!(&packet[..3], &[1, 2, 3]);
        bytes[..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert_eq!(
            record(&mut &bytes[..], &mut packet).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}
