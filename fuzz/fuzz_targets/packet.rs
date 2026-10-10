// SPDX-License-Identifier: MIT
#![no_main]
use libfuzzer_sys::fuzz_target;
use snacka::opus_packet::Packet;

fuzz_target!(|bytes: &[u8]| {
    if let Ok(packet) = Packet::parse(bytes) {
        assert!((1..=48).contains(&packet.frame_count()));
        assert!((1..=2).contains(&packet.channels()));
        assert!(packet.configuration() < 32);
        let frames = packet.frames();
        assert_eq!(frames.len(), packet.frame_count());
        let payload: usize = frames
            .map(|frame| {
                assert!(frame.len() <= 1_275);
                frame.len()
            })
            .sum();
        assert!(payload < bytes.len());
        for rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
            assert!(packet.samples(rate).unwrap() <= rate as usize * 120 / 1_000);
        }
    }
});
