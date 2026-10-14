// SPDX-License-Identifier: MIT
//! Fixed native-rate SILK alignment without an unnecessary interpolation filter.
use super::{Bandwidth, Error};

pub(crate) struct NativeDelay {
    history: [f32; 12],
    delay: usize,
}
impl NativeDelay {
    pub(crate) const fn new(bandwidth: Bandwidth) -> Self {
        Self {
            history: [0.0; 12],
            delay: match bandwidth {
                Bandwidth::Narrow => 4,
                Bandwidth::Medium => 9,
                Bandwidth::Wide => 12,
            },
        }
    }
    pub(crate) fn reset(&mut self, bandwidth: Bandwidth) {
        *self = Self::new(bandwidth);
    }
    pub(crate) fn process(&mut self, input: &[f32], output: &mut [f32]) -> Result<(), Error> {
        if input.len() != output.len() {
            return Err(Error::Output);
        }
        if input.iter().any(|value| !value.is_finite()) {
            return Err(Error::Argument);
        }
        let count = input.len();
        if count == 0 {
            return Ok(());
        }
        if count >= self.delay {
            output[..self.delay].copy_from_slice(&self.history[..self.delay]);
            output[self.delay..].copy_from_slice(&input[..count - self.delay]);
            self.history[..self.delay].copy_from_slice(&input[count - self.delay..]);
        } else {
            output.copy_from_slice(&self.history[..count]);
            self.history.copy_within(count..self.delay, 0);
            self.history[self.delay - count..self.delay].copy_from_slice(input);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_delay_is_literal_shift_and_chunk_independent() {
        let input: Vec<f32> = (0..1024)
            .map(|n| ((n * 73 % 103) as f32 - 51.0) / 64.0)
            .collect();
        for (bandwidth, delay) in [
            (Bandwidth::Narrow, 4),
            (Bandwidth::Medium, 9),
            (Bandwidth::Wide, 12),
        ] {
            let expected: Vec<f32> = (0..input.len())
                .map(|n| if n < delay { 0.0 } else { input[n - delay] })
                .collect();
            let mut whole = NativeDelay::new(bandwidth);
            let mut output = vec![0.0; input.len()];
            whole.process(&input, &mut output).unwrap();
            assert_eq!(output, expected);
            let mut split = NativeDelay::new(bandwidth);
            let mut chunked = vec![123.0; input.len()];
            let mut offset = 0;
            for count in [1, 2, 7, 31, 3, 80, 13, 887] {
                split
                    .process(
                        &input[offset..offset + count],
                        &mut chunked[offset..offset + count],
                    )
                    .unwrap();
                offset += count;
            }
            assert_eq!(offset, input.len());
            assert_eq!(chunked, expected);
            assert_eq!(whole.history, split.history);
            let before = split.history;
            let mut bad = [123.0];
            assert_eq!(split.process(&[], &mut bad), Err(Error::Output));
            assert_eq!(split.process(&[f32::NAN], &mut bad), Err(Error::Argument));
            assert_eq!(split.history, before);
            assert_eq!(bad, [123.0]);
            split.reset(bandwidth);
            let mut zeros = [123.0; 12];
            split.process(&[0.0; 12], &mut zeros).unwrap();
            assert_eq!(zeros, [0.0; 12]);
            let extreme = [f32::MAX; 24];
            let mut output = [0.0; 24];
            split.process(&extreme, &mut output).unwrap();
            assert!(output.iter().all(|v| v.is_finite()));
            assert_eq!(&output[delay..], &extreme[..24 - delay]);
        }
    }
}
