// SPDX-License-Identifier: MIT
use snacka::FrameDuration;
use snacka::entropy::{Decoder, Encoder, Model};
use snacka::transform::{Complex, FftPlan, MdctPlan, MdctWorkspace};
use snacka::{
    Config, Decoder as AudioDecoder, Encoder as AudioEncoder, FRAME_SAMPLES, MAX_PACKET_BYTES,
    Packet,
};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;
use std::time::Duration;

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
// SAFETY: Every allocation operation delegates its exact arguments to System.
// Counting uses allocation-free thread local storage and does not affect pointers.
unsafe impl GlobalAlloc for Tracked {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: The allocator contract is forwarded to System unchanged.
        unsafe { System.alloc(layout) }
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        count();
        // SAFETY: The allocator contract is forwarded to System unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: System allocated the pointer with this layout.
        unsafe { System.dealloc(pointer, layout) }
    }
    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, size: usize) -> *mut u8 {
        count();
        // SAFETY: The allocator contract is forwarded to System unchanged.
        unsafe { System.realloc(pointer, layout, size) }
    }
}
#[global_allocator]
static ALLOCATOR: Tracked = Tracked;

#[test]
fn adaptation_construction_feedback_updates_and_timeouts_never_allocate() {
    use snacka::adaptive::{BitrateController, Feedback};
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    let mut rate = BitrateController::new(24_000, 12_000, 32_000, 39_984, Duration::ZERO).unwrap();
    for second in 1..=100 {
        rate.update(
            Duration::from_secs(second),
            Feedback {
                frames: 50,
                lost: if second < 10 { 5 } else { 0 },
                rtt: Some(Duration::from_millis(30)),
            },
        );
        rate.set_maximum(24_000).unwrap();
        black_box(rate.bitrate());
    }
    rate.tick(Duration::from_secs(105));
    let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(allocations, 0);
}

#[test]
fn steady_state_processing_never_allocates() {
    let model = Model::new(&[0, 27, 200, 256]).unwrap();
    let fft = FftPlan::new(480).unwrap();
    let mdct = MdctPlan::new(960, 120).unwrap();
    let mut workspace = MdctWorkspace::new(&mdct);
    let mut transform_output = [Complex::ZERO; 480];
    let mut transform_recovered = [Complex::ZERO; 480];
    let transform_input = [Complex::ZERO; 480];
    let mdct_input = [0.0; 1_920];
    let mut coefficients = [0.0; 960];
    let mut mdct_output = [0.0; 1_920];
    let mut entropy = [0; 128];
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let mut audio_encoder = AudioEncoder::new(
            Config::default()
                .with_frame_duration(duration)
                .with_harmonic_speech(true),
        );
        let mut audio_decoder = AudioDecoder::with_frame_duration(duration);
        let mut encoded = [0; MAX_PACKET_BYTES];
        let input: [f32; FRAME_SAMPLES] = std::array::from_fn(|i| (i as f32 * 0.15).sin());
        let dense: [f32; FRAME_SAMPLES] = std::array::from_fn(|i| {
            (1..=12)
                .map(|tone| {
                    ((i as f32 + 0.5)
                        * ((tone as f32 + 0.5) * std::f32::consts::TAU * 50.0 / 48_000.0))
                        .cos()
                })
                .sum::<f32>()
                / 20.0
        });
        let voiced: [f32; FRAME_SAMPLES] = std::array::from_fn(|i| {
            (1..=20)
                .map(|h| {
                    let f0 = 200.0f64;
                    let amplitude = (-((h as f64 * f0 - 1300.0) / 600.0).powi(2)).exp() * 0.07;
                    amplitude
                        * (std::f64::consts::TAU * h as f64 * f0 * i as f64 / 48000.0
                            + h as f64 * 0.21)
                            .cos()
                })
                .sum::<f64>() as f32
        });
        let silence = [0.0; FRAME_SAMPLES];
        let mut models = 0u8;
        let mut harmonic_frames = 0usize;
        let mut gain_widths = 0u16;
        let mut scaled = [0.0; FRAME_SAMPLES];
        let mut decoded_storage = [0.0; FRAME_SAMPLES];
        let decoded = &mut decoded_storage[..duration.samples()];
        ALLOCATIONS.with(|counter| counter.set(Some(0)));
        for bitrate in [12_345, 12_800, 14_400, 16_000, 24_000] {
            audio_encoder.set_bitrate(bitrate).unwrap();
            for frame in 0..1_000 {
                let input = &match frame / 250 {
                    0 => {
                        if (100..200).contains(&frame) {
                            &voiced
                        } else {
                            &input
                        }
                    }
                    1 | 2 => &dense,
                    _ => &silence,
                }[..duration.samples()];
                let level = 0.2 + (frame % 40) as f32 * 0.02;
                for (value, &source) in scaled.iter_mut().zip(input) {
                    *value = level * source;
                }
                let input = &scaled[..duration.samples()];
                let length = audio_encoder
                    .encode(black_box(input), &mut encoded)
                    .unwrap();
                let parsed = Packet::parse(black_box(&encoded[..length])).unwrap();
                black_box(parsed.payload());
                harmonic_frames += usize::from(encoded[0] & 14 == 14);
                models |= if parsed.gain_bands() == 4 { 2 } else { 1 };
                if parsed.gain_bands() == 4 {
                    let bits = (parsed.payload().len() * 240 / duration.samples()).clamp(5, 8);
                    gain_widths |= 1 << bits;
                }
                audio_decoder.decode(&encoded[..length], decoded).unwrap();
                black_box(&*decoded);
                audio_decoder.conceal(decoded).unwrap();
                audio_encoder.encode(input, &mut encoded).unwrap();
                black_box(&*decoded);
                let mut encoder = Encoder::new(&mut entropy);
                encoder.symbol(1, model).unwrap();
                encoder.uniform(935, 1_000).unwrap();
                encoder.raw(0xa5, 8).unwrap();
                let length = encoder.finish().unwrap();
                let mut decoder = Decoder::new(&entropy[..length]);
                black_box(decoder.symbol(model));
                black_box(decoder.uniform(1_000).unwrap());
                black_box(decoder.raw(8).unwrap());
                fft.forward(black_box(&transform_input), &mut transform_output)
                    .unwrap();
                fft.inverse(black_box(&transform_output), &mut transform_recovered)
                    .unwrap();
                black_box(&transform_recovered);
                mdct.forward(black_box(&mdct_input), &mut coefficients, &mut workspace)
                    .unwrap();
                mdct.inverse(black_box(&coefficients), &mut mdct_output, &mut workspace)
                    .unwrap();
                black_box(&mdct_output);
            }
        }
        assert_eq!(
            models, 3,
            "both spectral models must reach allocation tracking"
        );
        audio_encoder.set_bitrate(12_345).unwrap();
        audio_encoder.finish(&mut encoded).unwrap();
        audio_encoder.reset();
        audio_decoder.reset();
        let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
        assert_eq!(allocations, 0);
        assert!(
            harmonic_frames > 0,
            "harmonic decoding must reach allocation tracking"
        );
        assert_eq!(
            gain_widths,
            if duration == FrameDuration::Ms10 {
                0b1_1110_0000
            } else {
                0b1_1100_0000
            }
        );
    }
}
