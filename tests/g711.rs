// SPDX-License-Identifier: MIT
use snacka::g711::{decode_a_law, decode_mu_law, encode_a_law, encode_mu_law};

#[test]
fn block_conversion_checks_capacity_before_writing() {
    use snacka::g711::{BufferError, Law};
    for law in [Law::A, Law::Mu] {
        let input = [i16::MIN, -1, 0, i16::MAX];
        let mut bytes = [42; 5];
        assert_eq!(
            law.encode_into(&input, &mut bytes[..3]),
            Err(BufferError {
                required: 4,
                available: 3
            })
        );
        assert_eq!(bytes, [42; 5]);
        assert_eq!(law.encode_into(&input, &mut bytes), Ok(4));
        assert_eq!(bytes[4], 42);
        let mut samples = [123; 5];
        assert!(law.decode_into(&bytes[..4], &mut samples[..3]).is_err());
        assert_eq!(samples, [123; 5]);
        assert_eq!(law.decode_into(&bytes[..4], &mut samples), Ok(4));
        assert_eq!(samples[4], 123);
        for (index, &sample) in input.iter().enumerate() {
            let (encoded, decoded) = match law {
                Law::A => (encode_a_law(sample), decode_a_law(bytes[index])),
                Law::Mu => (encode_mu_law(sample), decode_mu_law(bytes[index])),
            };
            assert_eq!(bytes[index], encoded);
            assert_eq!(samples[index], decoded);
        }
    }
}

#[test]
fn every_decoded_sample_matches_independent_fixtures() {
    let a_law = include_bytes!("fixtures/a-law-decoded.pcm");
    let mu_law = include_bytes!("fixtures/mu-law-decoded.pcm");
    for code in 0..=255u8 {
        let offset = usize::from(code) * 2;
        let expected_a = i16::from_le_bytes([a_law[offset], a_law[offset + 1]]);
        let expected_mu = i16::from_le_bytes([mu_law[offset], mu_law[offset + 1]]);
        assert_eq!(decode_a_law(code), expected_a);
        assert_eq!(decode_mu_law(code), expected_mu);
    }
}

#[test]
fn companding_matches_standard_boundary_values() {
    assert_eq!(encode_mu_law(0), 0xff);
    assert_eq!(encode_mu_law(-1), 0x7f);
    assert_eq!(decode_mu_law(0xff), 0);
    assert_eq!(decode_mu_law(0x7f), 0);
    assert_eq!(decode_mu_law(0x80), 32_124);
    assert_eq!(decode_mu_law(0), -32_124);
    assert_eq!(encode_a_law(0), 0xd5);
    assert_eq!(encode_a_law(-1), 0x55);
    assert_eq!(decode_a_law(0xd5), 8);
    assert_eq!(decode_a_law(0x55), -8);
    assert_eq!(decode_a_law(0xaa), 32_256);
    assert_eq!(decode_a_law(0x2a), -32_256);
}

#[test]
fn every_code_round_trips_to_its_canonical_byte() {
    for code in 0..=255 {
        let code = code as u8;
        assert_eq!(encode_a_law(decode_a_law(code)), code);
        let canonical_mu = if code == 0x7f { 0xff } else { code };
        assert_eq!(encode_mu_law(decode_mu_law(code)), canonical_mu);
    }
}

#[test]
fn all_pcm_values_remain_monotone_and_bounded() {
    let mut previous_a = i16::MIN;
    let mut previous_mu = i16::MIN;
    for sample in i16::MIN..=i16::MAX {
        let a = decode_a_law(encode_a_law(sample));
        let mu = decode_mu_law(encode_mu_law(sample));
        assert!(a >= previous_a);
        assert!(mu >= previous_mu);
        assert!((i32::from(a) - i32::from(sample)).abs() <= 512);
        assert!((i32::from(mu) - i32::from(sample)).abs() <= 644);
        previous_a = a;
        previous_mu = mu;
    }
}
