// SPDX-License-Identifier: MIT
use snacka::SAMPLE_RATE;
use sonora::{
    AudioProcessing, Config, StreamConfig,
    config::{AdaptiveDigital, EchoCanceller, GainController2, HighPassFilter, NoiseSuppression},
};

pub const BLOCK: usize = 480;

pub struct Processing {
    apm: AudioProcessing,
    scratch: [f32; BLOCK],
}

impl Processing {
    pub fn new(aec: bool, ns: bool, agc: bool) -> Self {
        let config = Config {
            echo_canceller: aec.then(EchoCanceller::default),
            high_pass_filter: Some(HighPassFilter::default()),
            noise_suppression: ns.then(NoiseSuppression::default),
            gain_controller2: agc.then(|| GainController2 {
                adaptive_digital: Some(AdaptiveDigital {
                    initial_gain_db: 0.0,
                    max_gain_db: 20.0,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        };
        Self {
            apm: AudioProcessing::builder()
                .config(config)
                .capture_config(StreamConfig::new(SAMPLE_RATE, 1))
                .render_config(StreamConfig::new(SAMPLE_RATE, 1))
                .build(),
            scratch: [0.0; BLOCK],
        }
    }

    pub fn render(&mut self, block: &[f32; BLOCK]) -> Result<(), sonora::Error> {
        self.apm
            .process_render_f32(&[block], &mut [&mut self.scratch])
    }

    pub fn capture(
        &mut self,
        block: &[f32; BLOCK],
        output: &mut [f32],
    ) -> Result<(), sonora::Error> {
        self.apm.process_capture_f32(&[block], &mut [output])?;
        for sample in output {
            *sample = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn echo_reference_reduces_delayed_broadband_echo_after_adaptation() {
        // A reproducible far-end signal with a 30 ms acoustic path. Test AEC
        // alone so NS/AGC cannot hide a disconnected playback reference.
        let mut processing = Processing::new(true, false, false);
        let mut random = 19u32;
        let mut history = [[0.0; BLOCK]; 4];
        let mut output = [0.0; BLOCK];
        let mut input_energy = 0.0f64;
        let mut output_energy = 0.0f64;
        for frame in 0..800 {
            let mut render = [0.0; BLOCK];
            for sample in &mut render {
                random = random.wrapping_mul(1664525).wrapping_add(1013904223);
                *sample = ((random >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.4;
            }
            let capture = history[(frame + 1) % 4].map(|s| s * 0.6);
            history[frame % 4] = render;
            processing.render(&render).unwrap();
            processing.capture(&capture, &mut output).unwrap();
            if frame >= 600 {
                input_energy += capture.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
                output_energy += output.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>();
            }
            assert!(output.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        }
        assert!(
            output_energy < input_energy * 0.25,
            "AEC reduction was only {} dB",
            10.0 * (input_energy / output_energy).log10()
        );
    }

    #[test]
    fn full_pipeline_handles_silence_without_nonfinite_output() {
        let mut processing = Processing::new(true, true, true);
        let mut output = [0.0; BLOCK];
        for _ in 0..100 {
            processing.render(&[0.0; BLOCK]).unwrap();
            processing.capture(&[0.0; BLOCK], &mut output).unwrap();
            assert!(output.iter().all(|s| s.is_finite() && s.abs() <= 1.0));
        }
    }
}
