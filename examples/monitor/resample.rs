// SPDX-License-Identifier: MIT
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters,
    audioadapter_buffers::direct::InterleavedSlice,
};
use std::error::Error;

/// Streaming mono conversion with all storage allocated before opening audio.
/// Equal rates bypass the filter, preserving the original 48 kHz path exactly.
pub struct Converter {
    resampler: Option<Async<f32>>,
    input: Vec<f32>,
    output: Vec<f32>,
    filled: usize,
    available: usize,
    position: usize,
}

impl Converter {
    pub fn new(input_rate: u32, output_rate: u32) -> Result<Self, Box<dyn Error>> {
        let resampler = if input_rate == output_rate {
            None
        } else {
            Some(Async::new_sinc(
                f64::from(output_rate) / f64::from(input_rate),
                1.0,
                &SincInterpolationParameters::default(),
                // Ten milliseconds of input, independent of callback boundaries.
                (input_rate as usize / 100).max(1),
                1,
                FixedAsync::Input,
            )?)
        };
        let input_size = resampler.as_ref().map_or(1, Resampler::input_frames_max);
        let output_size = resampler.as_ref().map_or(1, Resampler::output_frames_max);
        Ok(Self {
            resampler,
            input: vec![0.0; input_size],
            output: vec![0.0; output_size],
            filled: 0,
            available: 0,
            position: 0,
        })
    }

    /// Feed one capture sample; a completed block returns codec-rate samples.
    pub fn push(&mut self, sample: f32) -> &[f32] {
        let Some(resampler) = self.resampler.as_mut() else {
            self.output[0] = sample;
            return &self.output[..1];
        };
        self.input[self.filled] = sample;
        self.filled += 1;
        if self.filled < resampler.input_frames_next() {
            return &[];
        }
        let input = InterleavedSlice::new(&self.input, 1, self.input.len())
            .expect("preallocated mono input");
        let output_len = self.output.len();
        let mut output = InterleavedSlice::new_mut(&mut self.output, 1, output_len)
            .expect("preallocated mono output");
        let (_, written) = resampler
            .process_into_buffer(&input, &mut output, None)
            .expect("buffers sized for the fixed resampling ratio");
        self.filled = 0;
        self.available = written;
        self.position = 0;
        &self.output[..written]
    }

    /// Pull one playback sample, consuming codec-rate samples only as needed.
    pub fn next(&mut self, mut source: impl FnMut() -> f32) -> f32 {
        if self.resampler.is_none() {
            return source();
        }
        while self.position == self.available {
            self.push(source());
        }
        let sample = self.output[self.position];
        self.position += 1;
        // Windowed interpolation can overshoot normalized input slightly.
        sample.clamp(-1.0, 1.0)
    }

    pub fn reset(&mut self) {
        if let Some(resampler) = self.resampler.as_mut() {
            resampler.reset();
        }
        self.filled = 0;
        self.available = 0;
        self.position = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        alloc::{GlobalAlloc, Layout, System},
        cell::Cell,
        f32::consts::TAU,
        hint::black_box,
    };

    thread_local! {
        static ALLOCATIONS: Cell<Option<usize>> = const { Cell::new(None) };
    }

    fn count() {
        let _ = ALLOCATIONS.try_with(|counter| {
            if let Some(previous) = counter.get() {
                counter.set(Some(previous + 1));
            }
        });
    }

    struct Tracked;
    // SAFETY: The exact allocation arguments and pointers are forwarded to System.
    // Counting uses allocation-free thread-local storage.
    unsafe impl GlobalAlloc for Tracked {
        unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
            count();
            // SAFETY: The allocator contract is forwarded unchanged.
            unsafe { System.alloc(layout) }
        }
        unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
            count();
            // SAFETY: The allocator contract is forwarded unchanged.
            unsafe { System.alloc_zeroed(layout) }
        }
        unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
            count();
            // SAFETY: System owns this pointer and layout; the contract is forwarded.
            unsafe { System.realloc(pointer, layout, size) }
        }
        unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
            // SAFETY: System owns this pointer and layout; the contract is forwarded.
            unsafe { System.dealloc(pointer, layout) }
        }
    }

    #[global_allocator]
    static ALLOCATOR: Tracked = Tracked;

    #[test]
    fn callback_conversion_and_repriming_do_not_allocate() {
        for rate in [8_000, 16_000, 24_000, 44_100, 48_000, 96_000] {
            let mut capture = Converter::new(rate, 48_000).unwrap();
            let mut playback = Converter::new(48_000, rate).unwrap();
            ALLOCATIONS.with(|counter| counter.set(Some(0)));
            // Include the first processing block and repeated playback resets.
            for i in 0..rate {
                black_box(capture.push(0.25));
                black_box(playback.next(|| 0.25));
                if i % 257 == 0 {
                    playback.reset();
                }
            }
            let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
            assert_eq!(allocations, 0, "device rate {rate}");
        }
    }

    fn tone_amplitude(signal: &[f32], rate: u32, frequency: f32) -> f32 {
        let (sin, cos) = signal
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(sin, cos), (i, &s)| {
                let phase = TAU * frequency * i as f32 / rate as f32;
                (sin + s * phase.sin(), cos + s * phase.cos())
            });
        2.0 * sin.hypot(cos) / signal.len() as f32
    }

    #[test]
    fn equal_rates_preserve_samples_without_buffering() {
        let mut converter = Converter::new(48_000, 48_000).unwrap();
        for sample in [-1.0, -0.1, 0.0, 0.25, 1.0] {
            assert_eq!(converter.push(sample), &[sample]);
            assert_eq!(converter.next(|| sample), sample);
        }
    }

    #[test]
    fn capture_preserves_duration_and_pitch_at_common_device_rates() {
        for rate in [8_000, 16_000, 24_000, 44_100, 96_000] {
            let mut converter = Converter::new(rate, 48_000).unwrap();
            let mut output = Vec::new();
            // Feed one sample at a time so no hardware callback size is assumed.
            for i in 0..rate {
                let sample = 0.2 * (TAU * 1_000.0 * i as f32 / rate as f32).sin();
                output.extend_from_slice(converter.push(sample));
            }
            assert!(
                output.len().abs_diff(48_000) < 1_600,
                "{rate}: {}",
                output.len()
            );
            let amplitude = tone_amplitude(&output[4_800..], 48_000, 1_000.0);
            assert!((amplitude - 0.2).abs() < 0.01, "{rate}: {amplitude}");
        }
    }

    #[test]
    fn playback_preserves_pitch_and_consumes_codec_rate_audio() {
        for rate in [8_000, 16_000, 24_000, 44_100, 96_000] {
            let mut converter = Converter::new(48_000, rate).unwrap();
            let mut consumed = 0_usize;
            let output: Vec<_> = (0..rate)
                .map(|_| {
                    converter.next(|| {
                        let sample = 0.2 * (TAU * 1_000.0 * consumed as f32 / 48_000.0).sin();
                        consumed += 1;
                        sample
                    })
                })
                .collect();
            assert!(consumed.abs_diff(48_000) <= 960, "{rate}: {consumed}");
            let amplitude = tone_amplitude(&output[rate as usize / 10..], rate, 1_000.0);
            assert!((amplitude - 0.2).abs() < 0.01, "{rate}: {amplitude}");
        }
    }

    #[test]
    fn downsampling_rejects_tones_above_device_nyquist() {
        let mut converter = Converter::new(48_000, 16_000).unwrap();
        let mut output = Vec::new();
        for i in 0..48_000 {
            output.extend_from_slice(converter.push((TAU * 12_000.0 * i as f32 / 48_000.0).sin()));
        }
        let signal = &output[1_600..];
        let rms = (signal.iter().map(|s| s * s).sum::<f32>() / signal.len() as f32).sqrt();
        assert!(rms < 0.001, "aliased RMS: {rms}");
    }

    #[test]
    fn reprime_clears_pending_audio_and_filter_history() {
        let mut converter = Converter::new(48_000, 16_000).unwrap();
        for _ in 0..1_000 {
            converter.next(|| 0.5);
        }
        converter.reset();
        for _ in 0..1_000 {
            assert_eq!(converter.next(|| 0.0), 0.0);
        }
    }
}
