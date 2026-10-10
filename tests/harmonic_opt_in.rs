// SPDX-License-Identifier: MIT
use snacka::{Config, Decoder, Encoder, FRAME_SAMPLES, FrameDuration, MAX_PACKET_BYTES};

#[test]
fn harmonic_speech_is_opt_in_and_selects_voiced_packets_without_decoder_setting() {
    assert!(!Config::default().harmonic_speech());
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let base = Config::new(12_000).unwrap().with_frame_duration(duration);
        assert!(!base.harmonic_speech());
        let enabled = base.with_harmonic_speech(true);
        assert!(enabled.harmonic_speech());
        let mut normal = Encoder::new(base);
        let mut disabled = Encoder::new(enabled.with_harmonic_speech(false));
        let mut harmonic = Encoder::new(enabled);
        let mut decoder = Decoder::with_frame_duration(duration);
        let mut input = [0.0; FRAME_SAMPLES];
        let mut output = [0.0; FRAME_SAMPLES];
        let mut normal_packet = [0; MAX_PACKET_BYTES];
        let mut disabled_packet = [0; MAX_PACKET_BYTES];
        let mut harmonic_packet = [0; MAX_PACKET_BYTES];
        let mut selected = 0;
        for frame in 0..12 {
            for (i, sample) in input[..duration.samples()].iter_mut().enumerate() {
                let time = (frame * duration.samples() + i) as f64;
                *sample = (1..=20)
                    .map(|h| {
                        let frequency = h as f64 * 200.0;
                        let amplitude = (-((frequency - 1300.0) / 600.0).powi(2)).exp() * 0.07;
                        amplitude
                            * (std::f64::consts::TAU * frequency * time / 48_000.0
                                + h as f64 * 0.21)
                                .cos()
                    })
                    .sum::<f64>() as f32;
            }
            let input = &input[..duration.samples()];
            let length = normal.encode(input, &mut normal_packet).unwrap();
            let disabled_length = disabled.encode(input, &mut disabled_packet).unwrap();
            assert_eq!(length, disabled_length);
            assert_eq!(&normal_packet[..length], &disabled_packet[..length]);
            assert_ne!(normal_packet[0] & 14, 14);
            let harmonic_length = harmonic.encode(input, &mut harmonic_packet).unwrap();
            assert_eq!(harmonic_length, length);
            selected += usize::from(harmonic_packet[0] & 14 == 14);
            decoder
                .decode(
                    &harmonic_packet[..harmonic_length],
                    &mut output[..duration.samples()],
                )
                .unwrap();
            assert!(
                output[..duration.samples()]
                    .iter()
                    .all(|sample| sample.is_finite())
            );
            if frame == 5 {
                normal.set_bitrate(12_345).unwrap();
                disabled.set_bitrate(12_345).unwrap();
                harmonic.set_bitrate(12_345).unwrap();
            }
        }
        assert!(
            selected > 0,
            "enabled 200 Hz speech must select harmonic profiles"
        );
    }
}
