// SPDX-License-Identifier: MIT
#![cfg(feature = "opus")]

use snacka::opus::{
    Decoder, Encoder, EncoderConfig, Packet,
    extensions::{Extension, Extensions, FramingError},
    packet::MAX_FRAMES,
    repacketize::Repacketizer,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

thread_local! {
    static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
}
struct Tracked;
fn allocation() {
    let _ = ALLOCATIONS.try_with(|counter| {
        if let Some(previous) = counter.get() {
            counter.set(Some(previous + 1));
        }
    });
}
// SAFETY: The unchanged allocation arguments are forwarded to System; the
// thread-local counter does not allocate or alter allocation ownership.
unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        allocation();
        // SAFETY: Forwarding the caller's contract unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        allocation();
        // SAFETY: Forwarding the caller's contract unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: System owns this allocation with the supplied layout.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        allocation();
        // SAFETY: Forwarding the caller's contract unchanged.
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Tracked = Tracked;

fn parsed(padding: &[u8], frames: usize) -> Vec<Extension<'_>> {
    Extensions::new(padding, frames)
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

fn extension(frame_index: usize, id: u8, payload: &[u8]) -> Extension<'_> {
    Extension {
        frame_index,
        id,
        payload,
    }
}

#[test]
fn ietf_appendix_vectors_preserve_order_within_each_frame() {
    // Mathematical framing examples from draft-ietf-mlcodec-opus-extension-06,
    // Appendix A. These opaque IDs do not imply support for their audio content.
    let expected = [
        extension(0, 28, b"a"),
        extension(1, 28, b"b"),
        extension(2, 28, b"c"),
        extension(0, 29, b"d"),
        extension(1, 29, b""),
        extension(2, 29, b""),
        extension(2, 120, b"E0ex2"),
        extension(1, 120, b"E0ex"),
        extension(1, 30, b""),
        extension(2, 30, b""),
        extension(2, 31, b"f"),
        extension(1, 31, b"e"),
        // Appendix Table 2 says frame 1 for this last instance, but A.13's
        // actual bytes and prose place it after the separator into frame 2.
        extension(2, 120, b"E0example"),
    ];
    let cases: &[(usize, &[u8])] = &[
        (1, &[0x39, 0x61]),
        (2, &[0x39, 0x61, 0x02, 0x39, 0x62]),
        (3, &[0x39, 0x61, 0x04, 0x62, 0x63]),
        (4, &[0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64]),
        (5, &[0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a]),
        (
            6,
            &[0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a, 0x04],
        ),
        (
            6,
            &[
                0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a, 0x04, 0x04, 0x00,
            ],
        ),
        (
            7,
            &[
                0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a, 0x04, 0xf0, 0x45, 0x30, 0x65,
                0x78, 0x32,
            ],
        ),
        (
            8,
            &[
                0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a, 0xf1, 0x04, 0x45, 0x30, 0x65,
                0x78, 0x04, 0x45, 0x30, 0x65, 0x78, 0x32,
            ],
        ),
        (
            8,
            &[
                0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a, 0xf1, 0x04, 0x45, 0x30, 0x65,
                0x78, 0x01, 0x04, 0x45, 0x30, 0x65, 0x78, 0x32,
            ],
        ),
        (
            8,
            &[
                0x39, 0x61, 0x05, 0x62, 0x63, 0x05, 0x3b, 0x64, 0x02, 0x3a, 0x05, 0xf1, 0x04, 0x45,
                0x30, 0x65, 0x78, 0x01, 0x04, 0x45, 0x30, 0x65, 0x78, 0x32,
            ],
        ),
        (
            9,
            &[
                0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a, 0xf1, 0x04, 0x45, 0x30, 0x65,
                0x78, 0x05, 0x05, 0x45, 0x30, 0x65, 0x78, 0x32, 0x3c,
            ],
        ),
        (
            10,
            &[
                0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a, 0xf1, 0x04, 0x45, 0x30, 0x65,
                0x78, 0x3c, 0x04, 0x45, 0x30, 0x65, 0x78, 0x32,
            ],
        ),
        (
            10,
            &[
                0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a, 0xf1, 0x04, 0x45, 0x30, 0x65,
                0x78, 0x03, 0x00, 0x3c, 0x04, 0x45, 0x30, 0x65, 0x78, 0x32,
            ],
        ),
        (
            11,
            &[
                0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a, 0xf1, 0x04, 0x45, 0x30, 0x65,
                0x78, 0x3c, 0x05, 0x05, 0x45, 0x30, 0x65, 0x78, 0x32, 0x02, 0x3f, 0x66,
            ],
        ),
        (
            12,
            &[
                0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a, 0xf1, 0x04, 0x45, 0x30, 0x65,
                0x78, 0x3c, 0x3f, 0x65, 0x04, 0x45, 0x30, 0x65, 0x78, 0x32, 0x66,
            ],
        ),
        (
            13,
            &[
                0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a, 0xf1, 0x04, 0x45, 0x30, 0x65,
                0x78, 0x3c, 0x3f, 0x65, 0x05, 0x05, 0x45, 0x30, 0x65, 0x78, 0x32, 0x66, 0x02, 0xf0,
                0x45, 0x30, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65,
            ],
        ),
        (
            13,
            &[
                0x39, 0x61, 0x05, 0x62, 0x63, 0x3b, 0x64, 0x02, 0x3a, 0xf1, 0x04, 0x45, 0x30, 0x65,
                0x78, 0x3c, 0x3f, 0x65, 0x05, 0x05, 0x45, 0x30, 0x65, 0x78, 0x32, 0x66, 0x04, 0xf0,
                0x45, 0x30, 0x65, 0x78, 0x61, 0x6d, 0x70, 0x6c, 0x65,
            ],
        ),
    ];
    for &(count, padding) in cases {
        let actual = parsed(padding, 3);
        for frame in 0..3 {
            assert_eq!(
                actual
                    .iter()
                    .filter(|item| item.frame_index == frame)
                    .collect::<Vec<_>>(),
                expected[..count]
                    .iter()
                    .filter(|item| item.frame_index == frame)
                    .collect::<Vec<_>>(),
                "padding {padding:x?}, frame {frame}"
            );
        }
    }
}

#[test]
fn independent_repeat_cases_cover_group_resets_and_final_short_reservation() {
    assert_eq!(
        parsed(&[0x41, 1, b'A', 7, b'B', 4, b'C', b'D'], 2),
        [
            extension(0, 32, b"A"),
            extension(0, 3, b"B"),
            extension(1, 32, b"C"),
            extension(1, 3, b"D")
        ]
    );
    assert_eq!(
        parsed(&[0x41, 1, b'A', 7, b'B', 4, b'D'], 2),
        [
            extension(0, 32, b"A"),
            extension(0, 3, b"B"),
            extension(1, 32, b""),
            extension(1, 3, b"D")
        ]
    );
    assert_eq!(
        parsed(&[7, b'A', 3, 0, 9, b'B', 4, b'C', b'D', b'E', b'F'], 3),
        [
            extension(0, 3, b"A"),
            extension(0, 4, b"B"),
            extension(1, 3, b"C"),
            extension(1, 4, b"D"),
            extension(2, 3, b"E"),
            extension(2, 4, b"F")
        ]
    );
    assert_eq!(
        parsed(&[7, b'A', 4, b'B', b'C', 9, b'D'], 3),
        [
            extension(0, 3, b"A"),
            extension(1, 3, b"B"),
            extension(2, 3, b"C"),
            extension(1, 4, b"D")
        ]
    );
    assert_eq!(
        parsed(
            &[7, b'A', 5, b'B', b'C', 9, b'D', 5, b'E', b'F', 11, b'G'],
            3
        ),
        [
            extension(0, 3, b"A"),
            extension(1, 3, b"B"),
            extension(2, 3, b"C"),
            extension(0, 4, b"D"),
            extension(1, 4, b"E"),
            extension(2, 4, b"F"),
            extension(0, 5, b"G")
        ]
    );
    // Two long templates: the first still needs an explicit length in the final frame.
    assert_eq!(
        parsed(&[0x41, 1, b'A', 0x43, 1, b'B', 4, 1, b'C', b'D'], 2),
        [
            extension(0, 32, b"A"),
            extension(0, 33, b"B"),
            extension(1, 32, b"C"),
            extension(1, 33, b"D")
        ]
    );
    assert_eq!(parsed(&[6, 4], 48).len(), 48);
    assert_eq!(parsed(&[5, 5, 4], 3), []);
    assert_eq!(
        parsed(&[7, b'A', 4, 255, 255, 255], 1),
        [extension(0, 3, b"A")]
    );
    assert_eq!(
        parsed(&[7, b'A', 5, 9, b'B'], 1),
        [extension(0, 3, b"A"), extension(0, 4, b"B")]
    );
}

#[test]
fn every_id_and_explicit_length_boundaries_are_opaque_and_borrowed() {
    for id in 3..=127 {
        for length in if id < 32 {
            vec![0, 1]
        } else {
            vec![0, 1, 254, 255, 256, 509, 510, 511, 65_536]
        } {
            let mut padding = vec![(id << 1) | u8::from(length != 0 || id >= 32)];
            if id >= 32 {
                padding.extend(std::iter::repeat_n(255, length / 255));
                padding.push((length % 255) as u8);
            }
            let start = padding.len();
            padding.extend((0..length).map(|index| (index % 251) as u8));
            let actual = parsed(&padding, 1);
            assert_eq!(actual, [extension(0, id, &padding[start..])]);
            assert_eq!(actual[0].payload.as_ptr(), padding[start..].as_ptr());
        }
        if id >= 32 {
            let padding = [id << 1, 0, 1, 2, 255];
            assert_eq!(parsed(&padding, 1), [extension(0, id, &padding[1..])]);
        }
    }
    assert_eq!(parsed(&[1, 1, 6, 1, 0, 255, 7], 1), [extension(0, 3, b"")]);
    assert_eq!(parsed(&[3, 2, 7, b'X'], 3), [extension(2, 3, b"X")]);
}

#[test]
fn malformed_extensions_report_one_error_then_fuse_preserving_prefix() {
    for frames in [0, 49, 255, usize::MAX] {
        assert_eq!(
            Extensions::new(&[], frames).unwrap_err(),
            FramingError::InvalidFrameCount
        );
    }
    let cases: &[(&[u8], usize, FramingError)] = &[
        (&[7], 1, FramingError::TruncatedPayload),
        (&[0x41], 1, FramingError::TruncatedLength),
        (&[0x41, 255], 1, FramingError::TruncatedLength),
        (&[0x41, 255, 255, 0], 1, FramingError::TruncatedPayload),
        (&[0x41, 2, 1], 1, FramingError::TruncatedPayload),
        (&[3], 1, FramingError::TruncatedPayload),
        (&[2], 1, FramingError::InvalidFrameIndex),
        (&[3, 255, 6], 48, FramingError::InvalidFrameIndex),
        (&[7, b'A', 4, b'B'], 3, FramingError::TruncatedPayload),
        (
            &[0x41, 1, b'A', 7, b'B', 4],
            2,
            FramingError::TruncatedPayload,
        ),
    ];
    for &(padding, frames, expected) in cases {
        let mut iterator = Extensions::new(padding, frames).unwrap();
        while let Some(item) = iterator.next() {
            if let Err(error) = item {
                assert_eq!(error, expected, "{padding:x?}");
                assert_eq!(iterator.next(), None);
                assert_eq!(iterator.clone().next(), None);
                break;
            }
        }
        assert_eq!(
            Extensions::new(padding, frames).unwrap().validate(),
            Err(expected)
        );
    }
    // A valid empty repeated instance precedes the malformed implicit long one.
    let padding = [6, 0x41, 1, b'A', 7, b'B', 4];
    let mut iterator = Extensions::new(&padding, 2).unwrap();
    assert_eq!(iterator.next(), Some(Ok(extension(0, 3, b""))));
    assert_eq!(iterator.next(), Some(Ok(extension(0, 32, b"A"))));
    assert_eq!(iterator.next(), Some(Ok(extension(0, 3, b"B"))));
    assert_eq!(iterator.next(), Some(Ok(extension(1, 3, b""))));
    assert_eq!(iterator.next(), Some(Err(FramingError::TruncatedPayload)));
    assert_eq!(iterator.next(), None);
}

fn packet_with_padding(padding: &[u8], frames: usize, delimited: bool) -> Vec<u8> {
    let mut packet = vec![0x83, 0x40 | frames as u8];
    let mut remaining = padding.len();
    while remaining > 254 {
        packet.push(255);
        remaining -= 254;
    }
    packet.push(remaining as u8);
    if delimited {
        packet.push(1);
    }
    packet.extend(std::iter::repeat_n(0xa5, frames));
    packet.extend_from_slice(padding);
    packet
}

#[test]
fn packet_padding_excludes_all_length_headers_and_following_streams() {
    for length in [0, 1, 254, 255, 508, 509, 510, 65_536] {
        let mut padding = vec![0; length];
        if length >= 2 {
            padding[..2].copy_from_slice(&[7, b'Z']);
        }
        for frames in [1, 2, 3, MAX_FRAMES] {
            let ordinary = packet_with_padding(&padding, frames, false);
            let packet = Packet::parse(&ordinary).unwrap();
            assert_eq!(packet.padding(), padding);
            assert_eq!(packet.padding_len(), length);
            assert!(packet.frames().all(|frame| frame == [0xa5]));
            assert_eq!(
                packet.extensions().collect::<Vec<_>>(),
                Extensions::new(&padding, frames)
                    .unwrap()
                    .collect::<Vec<_>>()
            );
            let mut delimited = packet_with_padding(&padding, frames, true);
            let consumed = delimited.len();
            delimited.extend_from_slice(&[128, 1, 2, 3, 4, 5]);
            let (packet, actual) = Packet::parse_self_delimited(&delimited).unwrap();
            assert_eq!(actual, consumed);
            assert_eq!(packet.padding(), padding);
            assert_eq!(packet.bytes(), &delimited[..consumed]);
        }
    }
}

#[test]
fn invalid_extensions_do_not_change_valid_base_audio_or_repacketization() {
    let mut encoder = Encoder::new(EncoderConfig::default()).unwrap();
    let mut plain_decoder = Decoder::new(1).unwrap();
    let mut padded_decoder = Decoder::new(1).unwrap();
    let mut encoded = [0; 1276];
    let mut padded = [0; 1500];
    let mut plain_output = [0.0; 960];
    let mut padded_output = [0.0; 960];
    let padding_cases: &[&[u8]] = &[
        &[7],
        &[0x41, 255],
        &[2, 255],
        &[0xff, 255, 0],
        &[7, 42, 0x41, 99],
        &[0xf9, 2, 17, 23],
    ];
    let mut malformed = 0;
    for (index, &padding) in padding_cases.iter().enumerate() {
        let input = std::array::from_fn::<_, 960, _>(|sample| {
            (0.03 * (sample + 960 * index) as f32).sin() * 0.2
        });
        let length = encoder.encode(&input, &mut encoded).unwrap();
        let mut repacketizer = Repacketizer::new();
        repacketizer.push(&encoded[..length]).unwrap();
        let size = repacketizer
            .write_padded(0..1, &mut padded, length + padding.len() + 8)
            .unwrap();
        let pad_len = Packet::parse(&padded[..size]).unwrap().padding_len();
        padded[size - pad_len..size].fill(0);
        padded[size - pad_len..size - pad_len + padding.len()].copy_from_slice(padding);
        let parsed = Packet::parse(&padded[..size]).unwrap();
        malformed += usize::from(parsed.extensions().validate().is_err());
        assert_eq!(
            plain_decoder
                .decode(&encoded[..length], &mut plain_output)
                .unwrap(),
            960
        );
        assert_eq!(
            padded_decoder
                .decode(&padded[..size], &mut padded_output)
                .unwrap(),
            960
        );
        assert_eq!(plain_output, padded_output);
        assert_eq!(plain_decoder.final_range(), padded_decoder.final_range());
        let mut stripped = Repacketizer::new();
        stripped.push_packet(&parsed).unwrap();
        let mut output = [0; 1500];
        let size = stripped.write(0..1, &mut output).unwrap();
        assert_eq!(&output[..size], &encoded[..length]);
        assert!(Packet::parse(&output[..size]).unwrap().padding().is_empty());
    }
    assert!(malformed >= 3);
}

#[test]
fn adversarial_repeats_are_bounded_and_all_parser_paths_allocate_nothing() {
    let mut large = vec![6; 65_536];
    large.push(4);
    let packet_bytes = packet_with_padding(&large, MAX_FRAMES, false);
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    let packet = Packet::parse(&packet_bytes).unwrap();
    let mut iterator = packet.extensions();
    let mut count = 0;
    for item in iterator.by_ref() {
        let extension = item.unwrap();
        assert!(extension.frame_index < MAX_FRAMES);
        assert!(extension.payload.is_empty());
        count += 1;
    }
    assert_eq!(count, MAX_FRAMES * 65_536);
    assert_eq!(iterator.next(), None);
    assert_eq!(packet.extensions().validate(), Ok(()));
    assert_eq!(
        Extensions::new(&[6, 4], 0).unwrap_err(),
        FramingError::InvalidFrameCount
    );
    assert_eq!(
        Extensions::new(&[0x41, 255], 1).unwrap().validate(),
        Err(FramingError::TruncatedLength)
    );
    let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(allocations, 0);

    let mut random = 0x1248_ace1u32;
    for length in 0..256 {
        let mut padding = vec![0; length];
        for _ in 0..64 {
            for byte in &mut padding {
                random ^= random << 13;
                random ^= random >> 17;
                random ^= random << 5;
                *byte = random as u8;
            }
            for frames in [1, 2, 3, MAX_FRAMES] {
                let mut iterator = Extensions::new(&padding, frames).unwrap();
                let mut outputs = 0;
                let mut errors = 0;
                for item in iterator.by_ref().take(frames * length + 1) {
                    outputs += 1;
                    match item {
                        Ok(extension) => {
                            assert!(extension.frame_index < frames);
                            assert!((3..=127).contains(&extension.id));
                            assert!(extension.payload.as_ptr() >= padding.as_ptr());
                            assert!(
                                extension.payload.as_ptr_range().end <= padding.as_ptr_range().end
                            );
                        }
                        Err(_) => errors += 1,
                    }
                }
                assert!(outputs <= frames * length + 1);
                assert!(errors <= 1);
                assert_eq!(iterator.next(), None);
            }
        }
    }
}
