// SPDX-License-Identifier: MIT
#![no_main]

use libfuzzer_sys::fuzz_target;
use snacka::opus::ogg::{OggReader, OggWriter, OpusHead, OpusTags};
use snacka::opus::{
    Bandwidth, CeltConfig, CeltDecoder, CeltEncoder, CeltFrameDuration, Decoder, Packet,
};
use std::io::Cursor;

fuzz_target!(|bytes: &[u8]| {
    // Test framing and reach the entropy decoder with arbitrary valid framing.
    let _ = Packet::parse(bytes);
    if let Ok(head) = OpusHead::parse(bytes) {
        let gain = head.gain_factor();
        assert!(gain.is_finite() && gain > 0.0);
    }
    let _ = OpusTags::parse(bytes);
    if let Ok(mut reader) = OggReader::new(Cursor::new(bytes)) {
        // Bound work even if the input contains many valid short Ogg packets.
        for _ in 0..32 {
            let Ok(Some(packet)) = reader.next_packet() else {
                break;
            };
            assert!(packet.keep.start <= packet.keep.end);
            assert!(packet.keep.end <= packet.samples_48khz);
            assert!(Packet::parse(&packet.data).is_ok());
        }
    }
    let mut output = [0.0; 5760 * 2];
    for channels in [1, 2] {
        let mut universal = Decoder::new(channels).unwrap();
        if let Ok(samples) = universal.decode(bytes, &mut output) {
            assert!(
                output[..samples * channels]
                    .iter()
                    .all(|value| value.is_finite())
            );
        }
        let mut decoder = CeltDecoder::new(channels).unwrap();
        if let Ok(samples) = decoder.decode(bytes, &mut output) {
            assert!(
                output[..samples * channels]
                    .iter()
                    .all(|value| value.is_finite())
            );
        }
    }

    // Preserve valid TOC/framing while mutating actual spectral payloads.
    let first = bytes.first().copied().unwrap_or(0);
    let duration = match first & 3 {
        0 => CeltFrameDuration::Ms2_5,
        1 => CeltFrameDuration::Ms5,
        2 => CeltFrameDuration::Ms10,
        _ => CeltFrameDuration::Ms20,
    };
    let channels = 1 + usize::from(first >> 2 & 1);
    let rate = 24_000 + u32::from(first >> 3) * 2000;
    let config = CeltConfig::new(rate, duration, channels, Bandwidth::Fullband).unwrap();
    let mut encoder = CeltEncoder::new(config);
    let mut decoder = CeltDecoder::new(channels).unwrap();
    let mut input = [0.0; 960 * 2];
    for (index, value) in input[..duration.samples() * channels]
        .iter_mut()
        .enumerate()
    {
        *value = (f32::from(
            bytes
                .get(index % bytes.len().max(1))
                .copied()
                .unwrap_or(128),
        ) - 128.0)
            / 256.0;
    }
    let mut packet = [0; 1276];
    for _ in 0..2 {
        let length = encoder
            .encode(&input[..duration.samples() * channels], &mut packet)
            .unwrap();
        let samples = decoder.decode(&packet[..length], &mut output).unwrap();
        assert!(
            output[..samples * channels]
                .iter()
                .all(|value| value.is_finite())
        );
    }
    let length = encoder
        .encode(&input[..duration.samples() * channels], &mut packet)
        .unwrap();
    for (destination, &source) in packet[1..length].iter_mut().zip(bytes) {
        *destination ^= source;
    }
    if let Ok(samples) = decoder.decode(&packet[..length], &mut output) {
        assert!(
            output[..samples * channels]
                .iter()
                .all(|value| value.is_finite())
        );
    }
    // Grammar-preserving Ogg coverage reaches valid pages, timestamps and trims
    // on every input, while arbitrary bytes above exercise parser rejection.
    let skip = usize::from(first).min(duration.samples());
    let head = OpusHead {
        channels: channels as u8,
        pre_skip: skip as u16,
        input_sample_rate: 48_000,
        output_gain_q8: i16::from(first) - 128,
    };
    let mut writer =
        OggWriter::new(Vec::new(), u32::from(first), head, &OpusTags::default()).unwrap();
    writer.write_packet(&packet[..length]).unwrap();
    let file = writer.finish((duration.samples() - skip) as u64).unwrap();
    let mut reader = OggReader::new(Cursor::new(file)).unwrap();
    let parsed = reader.next_packet().unwrap().unwrap();
    assert_eq!(parsed.data, packet[..length]);
    assert_eq!(parsed.keep, skip..duration.samples());
    assert!(parsed.end_of_stream);
    assert!(reader.next_packet().unwrap().is_none());
});
