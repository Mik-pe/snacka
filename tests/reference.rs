// SPDX-License-Identifier: MIT
#![cfg(feature = "reference-tests")]

use snacka::opus_packet::Packet;

#[link(name = "opus")]
unsafe extern "C" {
    fn opus_packet_parse(
        data: *const u8,
        length: i32,
        toc: *mut u8,
        frames: *mut *const u8,
        sizes: *mut i16,
        offset: *mut i32,
    ) -> i32;
    fn opus_packet_get_nb_samples(data: *const u8, length: i32, rate: i32) -> i32;
}

fn compare(bytes: &[u8]) {
    let mut toc = 0;
    let mut frames = [std::ptr::null(); 48];
    let mut sizes = [0; 48];
    let mut offset = 0;
    // SAFETY: Input length is representable and all outputs have the capacities
    // specified by the public libopus API. An empty input is never dereferenced.
    let count = unsafe {
        opus_packet_parse(
            bytes.as_ptr(),
            bytes.len().try_into().unwrap(),
            &mut toc,
            frames.as_mut_ptr(),
            sizes.as_mut_ptr(),
            &mut offset,
        )
    };
    let native = Packet::parse(bytes);
    assert_eq!(native.is_ok(), count > 0, "framing disagreement: {bytes:?}");
    if let Ok(packet) = native {
        assert_eq!(packet.configuration(), toc >> 3);
        assert_eq!(packet.channels(), 1 + ((toc >> 2) & 1));
        assert_eq!(packet.frame_count(), count as usize);
        for (index, frame) in packet.frames().enumerate() {
            assert_eq!(frame.len(), sizes[index] as usize);
            assert_eq!(frame.as_ptr(), frames[index]);
        }
        for rate in [8_000, 12_000, 16_000, 24_000, 48_000] {
            // SAFETY: The same input is a validated packet and fits the C length.
            let samples =
                unsafe { opus_packet_get_nb_samples(bytes.as_ptr(), bytes.len() as i32, rate) };
            assert_eq!(packet.samples(rate as u32).unwrap(), samples as usize);
        }
    }
}

#[test]
fn every_short_header_and_random_payload_matches_libopus() {
    compare(&[]);
    for toc in 0..=255 {
        compare(&[toc]);
        for second in 0..=255 {
            compare(&[toc, second]);
        }
    }
    let mut random = 0x659a_23c7u32;
    let mut bytes = [0; 4_097];
    for case in 0..100_000 {
        let length = case % bytes.len();
        for byte in &mut bytes[..length] {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            *byte = random as u8;
        }
        compare(&bytes[..length]);
    }
}
