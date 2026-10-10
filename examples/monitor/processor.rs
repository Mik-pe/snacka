// SPDX-License-Identifier: MIT
use snacka::{ALGORITHMIC_DELAY, Config, Decoder, Encoder, FrameDuration, MAX_PACKET_BYTES};

/// Keep encoding in raw mode too, so A/B switches preserve codec history.
pub struct Processor {
    encoder: Encoder,
    decoder: Decoder,
    raw_delay: [f32; ALGORITHMIC_DELAY],
    delay_position: usize,
    pub config: Config,
    pub raw: bool,
    pub bytes: usize,
    pub samples: usize,
}

impl Processor {
    pub fn new(config: Config, raw: bool) -> Self {
        Self {
            encoder: Encoder::new(config),
            decoder: Decoder::with_frame_duration(config.frame_duration()),
            raw_delay: [0.0; ALGORITHMIC_DELAY],
            delay_position: 0,
            config,
            raw,
            bytes: 0,
            samples: 0,
        }
    }

    pub fn set_bitrate(&mut self, bitrate: u32) -> Result<(), snacka::Error> {
        let config = Config::new(bitrate)?.with_frame_duration(self.config.frame_duration());
        self.encoder.set_bitrate(bitrate)?;
        self.config = config;
        Ok(())
    }

    pub fn toggle_duration(&mut self) {
        let duration = match self.config.frame_duration() {
            FrameDuration::Ms10 => FrameDuration::Ms20,
            FrameDuration::Ms20 => FrameDuration::Ms10,
        };
        // Frame duration is a stream profile; both sides must restart together.
        let (bytes, samples) = (self.bytes, self.samples);
        *self = Self::new(self.config.with_frame_duration(duration), self.raw);
        self.bytes = bytes;
        self.samples = samples;
    }

    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), snacka::Error> {
        let mut packet = [0; MAX_PACKET_BYTES];
        let length = self.encoder.encode(input, &mut packet)?;
        self.decoder.decode(&packet[..length], output)?;
        self.bytes += length;
        self.samples += input.len();
        // Match the transform delay in the reference, including at startup.
        for (&sample, decoded) in input.iter().zip(output) {
            let delayed = std::mem::replace(&mut self.raw_delay[self.delay_position], sample);
            self.delay_position = (self.delay_position + 1) % ALGORITHMIC_DELAY;
            if self.raw {
                *decoded = delayed;
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use snacka::FRAME_SAMPLES;

    #[test]
    fn live_frames_match_codec_and_raw_switch_keeps_history() {
        for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
            let config = Config::new(24_000).unwrap().with_frame_duration(duration);
            let samples = config.frame_samples();
            let mut processor = Processor::new(config, false);
            let mut encoder = Encoder::new(config);
            let mut decoder = Decoder::with_frame_duration(duration);
            let mut output = [0.0; FRAME_SAMPLES];
            let mut expected = [0.0; FRAME_SAMPLES];
            let mut packet = [0; MAX_PACKET_BYTES];
            let signal: Vec<f32> = (0..samples * 6)
                .map(|i| (i as f32 * 0.06).sin() * 0.3)
                .collect();
            for (index, input) in signal.chunks(samples).enumerate() {
                processor.raw = index == 2 || index == 3;
                if index == 3 {
                    processor.set_bitrate(12_000).unwrap();
                    encoder.set_bitrate(12_000).unwrap();
                }
                processor.process(input, &mut output[..samples]).unwrap();
                let length = encoder.encode(input, &mut packet).unwrap();
                decoder
                    .decode(&packet[..length], &mut expected[..samples])
                    .unwrap();
                if processor.raw {
                    let start = index * samples - ALGORITHMIC_DELAY;
                    assert_eq!(&output[..samples], &signal[start..start + samples]);
                } else {
                    assert_eq!(&output[..samples], &expected[..samples]);
                }
            }
        }
    }

    #[test]
    fn frame_profile_switch_restarts_both_sides() {
        let mut processor = Processor::new(Config::default(), true);
        processor.toggle_duration();
        assert_eq!(processor.config.frame_duration(), FrameDuration::Ms10);
        let input = [0.25; 480];
        let mut output = [0.0; 480];
        processor.process(&input, &mut output).unwrap();
        assert_eq!(&output[..ALGORITHMIC_DELAY], &[0.0; ALGORITHMIC_DELAY]);
        assert_eq!(&output[ALGORITHMIC_DELAY..], &[0.25; 360]);
        processor.toggle_duration();
        assert_eq!(processor.config.frame_duration(), FrameDuration::Ms20);
        processor
            .process(&[0.0; FRAME_SAMPLES], &mut [0.0; FRAME_SAMPLES])
            .unwrap();
    }
}
