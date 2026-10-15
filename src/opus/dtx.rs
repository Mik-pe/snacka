// SPDX-License-Identifier: MIT
//! Original conservative silence admission and discontinuous transmit clock.

const HANGOVER_SAMPLES: usize = 9_600;
const REFRESH_SAMPLES: usize = 19_200;
const QUIET_MEAN_SQUARE: f64 = 1e-8;
const QUIET_PEAK: f32 = 4e-4;

/// Whether this interval needs a coded frame or can carry an empty frame.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Decision {
    /// Send a complete coded frame, including periodic inactive refreshes.
    Encode,
    /// The caller may omit transmission or emit duration-preserving framing.
    Suppress,
}

/// No-allocation DTX clock at the encoder's 48 kHz input boundary.
///
/// Admission intentionally requires quiet energy and quiet peaks in every
/// channel. Speech on either stereo channel immediately restores coding.
/// An active interval rearms a 200 ms hangover. During continuing quiet input,
/// a coded refresh is requested at least every 400 ms, rounded up to the
/// caller's packet duration. No codec state is changed by this selector.
pub(crate) struct State {
    inactive_samples: usize,
    since_refresh: usize,
    previous: Decision,
}

impl State {
    pub(crate) const fn new() -> Self {
        Self {
            inactive_samples: 0,
            since_refresh: 0,
            previous: Decision::Encode,
        }
    }

    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }

    /// Inspect already validated, normalized interleaved PCM. `samples` is
    /// the duration per channel, so stereo does not change the timing clock.
    pub(crate) fn decide(&mut self, pcm: &[f32], samples: usize) -> Decision {
        debug_assert!(!pcm.is_empty() && samples > 0);
        let mut square = 0.0f64;
        let quiet = pcm.iter().all(|&sample| {
            square += f64::from(sample) * f64::from(sample);
            sample.abs() <= QUIET_PEAK
        }) && square <= QUIET_MEAN_SQUARE * pcm.len() as f64;
        let decision = if !quiet {
            self.inactive_samples = 0;
            self.since_refresh = 0;
            Decision::Encode
        } else if self.inactive_samples < HANGOVER_SAMPLES {
            self.inactive_samples = self
                .inactive_samples
                .saturating_add(samples)
                .min(HANGOVER_SAMPLES);
            self.since_refresh = 0;
            Decision::Encode
        } else {
            self.since_refresh = self.since_refresh.saturating_add(samples);
            if self.since_refresh >= REFRESH_SAMPLES {
                self.since_refresh %= REFRESH_SAMPLES;
                Decision::Encode
            } else {
                Decision::Suppress
            }
        };
        self.previous = decision;
        decision
    }

    pub(crate) const fn suppressed(&self) -> bool {
        matches!(self.previous, Decision::Suppress)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hangover_and_refresh_use_audio_time_at_every_packet_duration() {
        for samples in [120, 240, 480, 960, 1920, 2880, 3840, 4800, 5760] {
            for channels in [1, 2] {
                let mut state = State::new();
                let quiet = vec![0.0; samples * channels];
                let mut elapsed = 0;
                while elapsed < HANGOVER_SAMPLES {
                    assert_eq!(state.decide(&quiet, samples), Decision::Encode);
                    assert!(!state.suppressed());
                    elapsed += samples;
                }
                let mut since_refresh = 0;
                for _ in 0..700 {
                    since_refresh += samples;
                    let wanted = if since_refresh >= REFRESH_SAMPLES {
                        since_refresh %= REFRESH_SAMPLES;
                        Decision::Encode
                    } else {
                        Decision::Suppress
                    };
                    assert_eq!(state.decide(&quiet, samples), wanted);
                    assert_eq!(state.suppressed(), wanted == Decision::Suppress);
                }
                state.reset();
                assert_eq!(state.decide(&quiet, samples), Decision::Encode);
            }
        }
    }

    #[test]
    fn quiet_background_and_stereo_onsets_preserve_speech_and_restart_hangover() {
        let mut state = State::new();
        let mut quiet = [0.0; 1920];
        for (index, sample) in quiet.iter_mut().enumerate() {
            *sample = if index % 2 == 0 { 8e-5 } else { -8e-5 };
        }
        for _ in 0..10 {
            assert_eq!(state.decide(&quiet, 960), Decision::Encode);
        }
        assert_eq!(state.decide(&quiet, 960), Decision::Suppress);
        // An onset in one channel must not be diluted by the quiet channel.
        quiet[1919] = 0.001;
        assert_eq!(state.decide(&quiet, 960), Decision::Encode);
        quiet[1919] = -8e-5;
        for _ in 0..10 {
            assert_eq!(state.decide(&quiet, 960), Decision::Encode);
        }
        assert_eq!(state.decide(&quiet, 960), Decision::Suppress);
        // Quiet voices above the conservative floor remain fully coded.
        quiet.fill(0.0002);
        for _ in 0..1000 {
            assert_eq!(state.decide(&quiet, 960), Decision::Encode);
        }
    }
}
