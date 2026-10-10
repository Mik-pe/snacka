// SPDX-License-Identifier: MIT
use snacka::entropy::{Decoder, Encoder, Error, Model};

#[test]
fn independent_reference_frames_match_every_symbol_and_coder_state() {
    let fixture = include_bytes!("fixtures/entropy-libopus-1.6.1.bin");
    assert_eq!(&fixture[..8], b"SNKENT01");
    let cases = u32::from_le_bytes(fixture[8..12].try_into().unwrap());
    let mut cursor = 12;
    let model = Model::new(&[0, 27, 200, 256]).unwrap();
    for case in 0..cases {
        let length = u16::from_le_bytes(fixture[cursor..cursor + 2].try_into().unwrap()) as usize;
        let events = u16::from_le_bytes(fixture[cursor + 2..cursor + 4].try_into().unwrap());
        cursor += 4;
        let mut decoder = Decoder::new(&fixture[cursor..cursor + length]);
        cursor += length;
        let mut native_packet = [0; 4_096];
        let mut encoder = Encoder::new(&mut native_packet);
        let events_start = cursor;
        for _ in 0..events {
            let kind = fixture[cursor];
            let fields: [u32; 5] = std::array::from_fn(|index| {
                let offset = cursor + 1 + index * 4;
                u32::from_le_bytes(fixture[offset..offset + 4].try_into().unwrap())
            });
            let [parameter, value, range, tell, eighths] = fields;
            let actual = match kind {
                0 => {
                    encoder.symbol(value as usize, model).unwrap();
                    decoder.symbol(model) as u32
                }
                1 => {
                    encoder.bit(value != 0, parameter).unwrap();
                    u32::from(decoder.bit(parameter).unwrap())
                }
                2 => {
                    encoder.raw(value, parameter).unwrap();
                    decoder.raw(parameter).unwrap()
                }
                3 => {
                    encoder.uniform(value, parameter).unwrap();
                    decoder.uniform(parameter).unwrap()
                }
                _ => panic!("unknown fixture event"),
            };
            assert_eq!(actual, value, "reference frame {case}");
            assert_eq!(
                (decoder.range(), decoder.tell(), decoder.tell_eighths()),
                (range, u64::from(tell), u64::from(eighths))
            );
            assert_eq!(
                (encoder.range(), encoder.tell(), encoder.tell_eighths()),
                (range, u64::from(tell), u64::from(eighths))
            );
            cursor += 21;
        }
        let length = encoder.finish().unwrap();
        let mut decoder = Decoder::new(&native_packet[..length]);
        for offset in (events_start..cursor).step_by(21) {
            let kind = fixture[offset];
            let parameter = u32::from_le_bytes(fixture[offset + 1..offset + 5].try_into().unwrap());
            let value = u32::from_le_bytes(fixture[offset + 5..offset + 9].try_into().unwrap());
            let actual = match kind {
                0 => decoder.symbol(model) as u32,
                1 => u32::from(decoder.bit(parameter).unwrap()),
                2 => decoder.raw(parameter).unwrap(),
                _ => decoder.uniform(parameter).unwrap(),
            };
            assert_eq!(
                actual, value,
                "native termination for reference frame {case}"
            );
        }
    }
    assert_eq!(cursor, fixture.len());
}

#[test]
fn mixed_symbols_raw_bits_and_uniform_integers_round_trip() {
    let model = Model::new(&[0, 27, 200, 256]).unwrap();
    for seed in 1..100u32 {
        let mut packet = [0; 1_024];
        let mut encoder = Encoder::new(&mut packet);
        let mut random = seed;
        let mut events = Vec::new();
        for index in 0..100 {
            random ^= random << 13;
            random ^= random >> 17;
            random ^= random << 5;
            let (kind, parameter, value) = match index % 4 {
                0 => (0, 3, random % 3),
                1 => (1, 1 + random % 16, (random >> 16) & 1),
                2 => {
                    let bits = random % 25;
                    (2, bits, (random >> 7) & ((1 << bits) - 1))
                }
                _ => {
                    let total = 2 + (random & 65_535);
                    (3, total, (random >> 4) % total)
                }
            };
            match kind {
                0 => encoder.symbol(value as usize, model).unwrap(),
                1 => encoder.bit(value != 0, parameter).unwrap(),
                2 => encoder.raw(value, parameter).unwrap(),
                _ => encoder.uniform(value, parameter).unwrap(),
            }
            events.push((
                kind,
                parameter,
                value,
                encoder.range(),
                encoder.tell(),
                encoder.tell_eighths(),
            ));
        }
        let length = encoder.finish().unwrap();
        let mut decoder = Decoder::new(&packet[..length]);
        for (kind, parameter, value, range, tell, eighths) in events {
            let actual = match kind {
                0 => decoder.symbol(model) as u32,
                1 => u32::from(decoder.bit(parameter).unwrap()),
                2 => decoder.raw(parameter).unwrap(),
                _ => decoder.uniform(parameter).unwrap(),
            };
            assert_eq!(actual, value, "seed {seed}");
            assert_eq!(decoder.range(), range);
            assert_eq!(decoder.tell(), tell);
            assert_eq!(decoder.tell_eighths(), eighths);
        }
    }
}

#[test]
fn invalid_arguments_do_not_consume_encoder_state() {
    let mut packet = [0; 32];
    let mut encoder = Encoder::new(&mut packet);
    let initial = (encoder.range(), encoder.tell_eighths());
    assert_eq!(encoder.bit(true, 0), Err(Error::Argument));
    assert_eq!(encoder.bit(true, 17), Err(Error::Argument));
    assert_eq!(encoder.raw(2, 1), Err(Error::Argument));
    assert_eq!(encoder.uniform(5, 5), Err(Error::Argument));
    assert_eq!(
        encoder.symbol(usize::MAX, Model::new(&[0, 1]).unwrap()),
        Err(Error::Argument)
    );
    assert_eq!((encoder.range(), encoder.tell_eighths()), initial);
}

#[test]
fn raw_reads_are_bounded_and_refusal_preserves_state() {
    let mut decoder = Decoder::new(&[0x3c, 0xa5]);
    assert_eq!(decoder.raw(4), Ok(5));
    let before = decoder.tell();
    assert_eq!(decoder.raw(13), Err(Error::Truncated));
    assert_eq!(decoder.raw(33), Err(Error::Argument));
    assert_eq!(decoder.tell(), before);
    assert_eq!(decoder.raw(4), Ok(10));
    assert_eq!(decoder.raw(8), Ok(0x3c));
    assert_eq!(decoder.raw(1), Err(Error::Truncated));
}

#[test]
fn buffer_exhaustion_is_terminal() {
    let mut packet = [0; 1];
    let mut encoder = Encoder::new(&mut packet);
    assert_eq!(encoder.raw(0xffff, 16), Err(Error::Output));
    assert_eq!(encoder.bit(false, 1), Err(Error::Output));
    assert_eq!(encoder.uniform(0, 1), Err(Error::Output));
    assert_eq!(encoder.finish(), Err(Error::Output));
}

#[test]
fn models_reject_zero_width_and_unsorted_intervals() {
    for boundaries in [&[][..], &[0][..], &[1, 2][..], &[0, 0][..], &[0, 3, 2][..]] {
        assert!(Model::new(boundaries).is_err());
    }
}

#[test]
fn boundary_uniform_values_and_32_raw_bits_round_trip() {
    for total in [1, 2, 255, 256, 257, 65_535, u32::MAX] {
        for value in [0, total - 1] {
            let mut packet = [0; 32];
            let mut encoder = Encoder::new(&mut packet);
            encoder.uniform(value, total).unwrap();
            encoder.raw(u32::MAX, 32).unwrap();
            let length = encoder.finish().unwrap();
            let mut decoder = Decoder::new(&packet[..length]);
            assert_eq!(decoder.uniform(total), Ok(value));
            assert_eq!(decoder.raw(32), Ok(u32::MAX));
        }
    }
}

#[test]
fn fixed_budget_frames_keep_raw_suffix_at_the_end_and_zero_the_gap() {
    for capacity in 6..=64 {
        let mut packet = [0xa5; 64];
        let mut encoder = Encoder::new(&mut packet[..capacity]);
        encoder.uniform(417, 480).unwrap();
        encoder.raw(0xabbc, 16).unwrap();
        encoder.uniform(6, 13).unwrap();
        encoder.raw(17, 5).unwrap();
        assert_eq!(encoder.finish_padded(), Ok(capacity));
        let mut decoder = Decoder::new(&packet[..capacity]);
        assert_eq!(decoder.uniform(480), Ok(417));
        assert_eq!(decoder.raw(16), Ok(0xabbc));
        assert_eq!(decoder.uniform(13), Ok(6));
        assert_eq!(decoder.raw(5), Ok(17));
        assert_eq!(&packet[capacity..], &[0xa5; 64][capacity..]);
    }
}
