// SPDX-License-Identifier: MIT
#![no_main]

use libfuzzer_sys::fuzz_target;
use snacka::opus::multistream::MultistreamPacket;
use snacka::opus::ogg::{
    OggChainEvent, OggChainReader, OggDemuxEvent, OggDemuxReader, OggPacket, OggReader, OggWriter,
    OpusHead, OpusIdentification, OpusTags,
};
use snacka::opus::{
    Bandwidth, CeltConfig, CeltDecoder, CeltEncoder, CeltFrameDuration, Decoder, Packet,
};
use std::io::Cursor;

fn check_packet(packet: &OggPacket, streams: u8) {
    assert!(packet.keep.start <= packet.keep.end);
    assert!(packet.keep.end <= packet.samples_48khz);
    assert!(MultistreamPacket::parse(&packet.data, streams).is_ok());
}

fn check_extensions(packet: &Packet<'_>) {
    let mut extensions = packet.extensions();
    // Fuzz-only output bound: tiny zero-byte templates can expand to 48 frames.
    // The production iterator has no extension-count cap.
    for _ in 0..64 {
        let Some(item) = extensions.next() else {
            break;
        };
        match item {
            Ok(extension) => {
                assert!(extension.frame_index < packet.frame_count());
                assert!((3..=127).contains(&extension.id));
                assert!(extension.payload.as_ptr() >= packet.padding().as_ptr());
                assert!(extension.payload.as_ptr_range().end <= packet.padding().as_ptr_range().end);
            }
            Err(_) => {
                assert!(extensions.next().is_none());
                break;
            }
        }
    }
}

fn check_padding(padding: &[u8], frames: usize) {
    // Always valid base framing, with up to 512 independently mutated padding
    // bytes. This reaches extension parsing even when arbitrary packets reject.
    let mut packet = [0; 520];
    packet[0] = 0x83; // 2.5 ms CELT frames, with zero-byte audio payloads.
    packet[1] = 0x40 | frames as u8;
    let mut cursor = 2;
    let mut remaining = padding.len();
    while remaining > 254 {
        packet[cursor] = 255;
        cursor += 1;
        remaining -= 254;
    }
    packet[cursor] = remaining as u8;
    cursor += 1;
    packet[cursor..cursor + padding.len()].copy_from_slice(padding);
    check_extensions(&Packet::parse(&packet[..cursor + padding.len()]).unwrap());
}

fn check_chain(bytes: &[u8]) -> usize {
    let mut packets = 0;
    if let Ok(mut reader) = OggChainReader::new(Cursor::new(bytes)) {
        let mut streams = None;
        for _ in 0..32 {
            match reader.next_event() {
                Ok(Some(OggChainEvent::Stream(stream))) => {
                    streams = Some(stream.identification.channel_mapping.streams());
                }
                Ok(Some(OggChainEvent::Packet(packet))) => {
                    check_packet(&packet, streams.expect("stream precedes its packets"));
                    packets += 1;
                }
                _ => break,
            }
        }
    }
    packets
}

fn check_demux(bytes: &[u8]) -> usize {
    let mut reader = OggDemuxReader::new(Cursor::new(bytes));
    let mut layouts = [(0u32, 0u8); 32];
    let mut known = 0;
    let mut packets = 0;
    for _ in 0..32 {
        match reader.next_event() {
            Ok(Some(OggDemuxEvent::Stream { serial, stream })) => {
                assert!(!layouts[..known].iter().any(|layout| layout.0 == serial));
                layouts[known] = (serial, stream.identification.channel_mapping.streams());
                known += 1;
            }
            Ok(Some(OggDemuxEvent::Packet { serial, packet })) => {
                let streams = layouts[..known]
                    .iter()
                    .find(|layout| layout.0 == serial)
                    .expect("stream precedes its packets")
                    .1;
                check_packet(&packet, streams);
                packets += 1;
            }
            Ok(Some(OggDemuxEvent::Opaque { .. })) => {}
            _ => break,
        }
    }
    packets
}

fuzz_target!(|bytes: &[u8]| {
    // Test framing and reach the entropy decoder with arbitrary valid framing.
    if let Ok(packet) = Packet::parse(bytes) {
        check_extensions(&packet);
    }
    let frames = 1 + usize::from(bytes.first().copied().unwrap_or(0) % 48);
    check_padding(&bytes[..bytes.len().min(512)], frames);
    // Keep two actual templates and an RTE structurally intact while fuzzing
    // repeated lengths/payloads, including the final long's short-byte reserve.
    let mut repeated = [0; 512];
    repeated[..6].copy_from_slice(&[0x41, 1, 42, 7, 23, 4]);
    let tail = bytes.len().min(repeated.len() - 6);
    repeated[6..6 + tail].copy_from_slice(&bytes[..tail]);
    check_padding(&repeated[..6 + tail], frames.max(2));
    if let Ok(head) = OpusHead::parse(bytes) {
        let gain = head.gain_factor();
        assert!(gain.is_finite() && gain > 0.0);
    }
    let _ = OpusIdentification::parse(bytes);
    let _ = OpusTags::parse(bytes);
    if let Ok(mut reader) = OggReader::new(Cursor::new(bytes)) {
        let streams = reader.mapping().streams();
        // Bound work even if the input contains many valid short Ogg packets.
        for _ in 0..32 {
            let Ok(Some(packet)) = reader.next_packet() else {
                break;
            };
            check_packet(&packet, streams);
        }
    }
    check_chain(bytes);
    check_demux(bytes);
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
    let mut reader = OggReader::new(Cursor::new(file.as_slice())).unwrap();
    let parsed = reader.next_packet().unwrap().unwrap();
    assert_eq!(parsed.data, packet[..length]);
    assert_eq!(parsed.keep, skip..duration.samples());
    assert!(parsed.end_of_stream);
    assert!(reader.next_packet().unwrap().is_none());
    let mut writer =
        OggWriter::new(file, u32::from(first) + 1, head, &OpusTags::default()).unwrap();
    writer.write_packet(&packet[..length]).unwrap();
    let chain = writer.finish((duration.samples() - skip) as u64).unwrap();
    assert_eq!(check_chain(&chain), 2);
    assert_eq!(check_demux(&chain), 2);
});
