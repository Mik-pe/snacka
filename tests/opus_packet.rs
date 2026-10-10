// SPDX-License-Identifier: MIT
use snacka::opus_packet::{Packet, PacketError};

#[test]
fn four_framing_codes_locate_payloads() {
    for (bytes, expected) in [
        (&[0xf8, 11, 12][..], vec![&[11, 12][..]]),
        (&[0xf9, 11, 12][..], vec![&[11][..], &[12][..]]),
        (&[0xfa, 1, 11, 12, 13][..], vec![&[11][..], &[12, 13][..]]),
        (
            &[0xfb, 3, 11, 12, 13][..],
            vec![&[11][..], &[12][..], &[13][..]],
        ),
    ] {
        let packet = Packet::parse(bytes).unwrap();
        assert_eq!(packet.frames().collect::<Vec<_>>(), expected);
        assert_eq!(packet.samples(48_000).unwrap(), expected.len() * 960);
    }
}

#[test]
fn variable_frames_and_padding_are_separate() {
    let bytes = [0xfb, 0xc3, 2, 0, 1, 11, 12, 13, 99, 98];
    let packet = Packet::parse(&bytes).unwrap();
    assert_eq!(
        packet.frames().collect::<Vec<_>>(),
        [&[][..], &[11][..], &[12, 13][..]]
    );
    let mut continued = vec![0xfb, 0x41, 255, 1, 42];
    continued.resize(260, 99);
    assert_eq!(
        Packet::parse(&continued).unwrap().frames().next(),
        Some(&[42][..])
    );
}

#[test]
fn zero_length_frames_are_valid_in_every_layout() {
    for bytes in [
        &[0xf8][..],
        &[0xf9][..],
        &[0xfa, 0][..],
        &[0xfb, 3][..],
        &[0xfb, 0x83, 0, 0][..],
    ] {
        let packet = Packet::parse(bytes).unwrap();
        assert!(packet.frames().all(|frame| frame.is_empty()));
    }
}

#[test]
fn duration_limits_follow_the_configuration() {
    assert_eq!(
        Packet::parse(&[0x83, 48]).unwrap().samples(48_000),
        Ok(5_760)
    );
    assert_eq!(
        Packet::parse(&[0x83, 49]).unwrap_err(),
        PacketError::Duration
    );
    assert_eq!(Packet::parse(&[0x1b, 2]).unwrap().samples(8_000), Ok(960));
    assert_eq!(
        Packet::parse(&[0x1b, 3]).unwrap_err(),
        PacketError::Duration
    );
    assert_eq!(
        Packet::parse(&[0x83, 0]).unwrap_err(),
        PacketError::Duration
    );
    assert_eq!(
        Packet::parse(&[0xf8]).unwrap().samples(44_100),
        Err(PacketError::SampleRate)
    );
}

#[test]
fn truncated_headers_and_impossible_lengths_are_rejected() {
    for bytes in [
        &[][..],
        &[0xfa][..],
        &[0xfa, 252][..],
        &[0xfb][..],
        &[0xfb, 0x41][..],
        &[0xfb, 0x82, 2][..],
        &[0xfa, 2, 1][..],
        &[0xf9, 1][..],
    ] {
        assert!(Packet::parse(bytes).is_err(), "accepted {bytes:?}");
    }
    let mut oversized = vec![0xf8];
    oversized.resize(1_277, 0);
    assert_eq!(Packet::parse(&oversized).unwrap_err(), PacketError::Length);
    assert!(Packet::parse(&[0xfb, 0x41, 255]).is_err());
}

#[test]
fn maximum_two_byte_length_is_accepted() {
    let mut bytes = vec![0xfa, 255, 255];
    bytes.resize(1_278, 42);
    let packet = Packet::parse(&bytes).unwrap();
    assert_eq!(
        packet.frames().map(|frame| frame.len()).collect::<Vec<_>>(),
        [1_275, 0]
    );
}
