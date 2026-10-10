// SPDX-License-Identifier: MIT
use snacka::entropy::{Decoder, Encoder, Model};
use snacka::g711::Law;
use snacka::opus_packet::Packet;
use snacka::transform::{Complex, FftPlan, MdctPlan, MdctWorkspace};
use std::hint::black_box;
use std::time::Instant;

fn measure(name: &str, mut operation: impl FnMut()) {
    for _ in 0..1_000 {
        operation();
    }
    let mut batch = 1;
    loop {
        let start = Instant::now();
        for _ in 0..batch {
            operation();
        }
        if start.elapsed().as_micros() >= 200 || batch >= 1 << 20 {
            break;
        }
        batch *= 2;
    }
    let mut samples = Vec::with_capacity(1_000);
    for _ in 0..1_000 {
        let start = Instant::now();
        for _ in 0..batch {
            operation();
        }
        samples.push(start.elapsed().as_nanos() as u64);
    }
    samples.sort_unstable();
    println!(
        "{name},batch={batch},median_ns_per_op={:.2},p95_batch_ns_per_op={:.2},p99_batch_ns_per_op={:.2}",
        samples[500] as f64 / batch as f64,
        samples[950] as f64 / batch as f64,
        samples[990] as f64 / batch as f64,
    );
}

#[cfg(feature = "reference-tests")]
unsafe extern "C" {
    fn opus_packet_parse(
        data: *const u8,
        length: i32,
        toc: *mut u8,
        frames: *mut *const u8,
        sizes: *mut i16,
        payload_offset: *mut i32,
    ) -> i32;
}
#[cfg(feature = "reference-tests")]
#[link(name = "opus")]
unsafe extern "C" {}

#[cfg(feature = "reference-tests")]
fn reference_packet(bytes: &[u8]) -> usize {
    use std::mem::MaybeUninit;
    let mut toc = MaybeUninit::<u8>::uninit();
    let mut frames = MaybeUninit::<[*const u8; 48]>::uninit();
    let mut sizes = MaybeUninit::<[i16; 48]>::uninit();
    let mut offset = MaybeUninit::<i32>::uninit();
    // SAFETY: Length describes the input and the outputs hold the maximum
    // frame count. The valid fixture guarantees the reference writes the TOC.
    let count = unsafe {
        opus_packet_parse(
            bytes.as_ptr(),
            bytes.len() as i32,
            toc.as_mut_ptr(),
            frames.as_mut_ptr().cast(),
            sizes.as_mut_ptr().cast(),
            offset.as_mut_ptr(),
        )
    };
    assert!((1..=48).contains(&count));
    // SAFETY: A positive result initializes the TOC and the first count sizes.
    let toc = unsafe { toc.assume_init() };
    // SAFETY: The checked frame count is exactly the initialized prefix.
    let sizes = unsafe { std::slice::from_raw_parts(sizes.as_ptr().cast::<i16>(), count as usize) };
    sizes.iter().map(|&length| length as usize).sum::<usize>()
        + usize::from(toc >> 3)
        + usize::from(1 + ((toc >> 2) & 1))
}

fn main() {
    println!(
        "1000 warmups; 1000 timed batches calibrated to >=200us; percentiles describe batch-average throughput, not individual tail latency"
    );
    let pcm: Vec<_> = (0..160)
        .map(|index| ((index * 937) as i16).wrapping_sub(16_000))
        .collect();
    let mut bytes = [0; 160];
    let mut decoded = [0; 160];
    for law in [Law::A, Law::Mu] {
        measure(&format!("g711_{law:?}_encode_160"), || {
            law.encode_into(black_box(&pcm), &mut bytes).unwrap();
            black_box(&bytes);
        });
        measure(&format!("g711_{law:?}_decode_160"), || {
            law.decode_into(black_box(&bytes), &mut decoded).unwrap();
            black_box(&decoded);
        });
    }
    let mut packet = [0; 81];
    packet[0] = 0xf8;
    measure("packet_parse_1_frame_80_bytes", || {
        let parsed = Packet::parse(black_box(&packet)).unwrap();
        black_box(
            parsed.frames().map(|frame| frame.len()).sum::<usize>()
                + usize::from(parsed.configuration())
                + usize::from(parsed.channels()),
        );
    });
    #[cfg(feature = "reference-tests")]
    measure("libopus_packet_parse_1_frame_80_bytes", || {
        black_box(reference_packet(black_box(&packet)));
    });
    let model = Model::new(&[0, 27, 200, 256]).unwrap();
    let mut entropy_packet = [0; 128];
    measure("entropy_encode_32_symbols", || {
        let mut encoder = Encoder::new(&mut entropy_packet);
        for symbol in 0..32 {
            encoder.symbol(black_box(symbol % 3), model).unwrap();
        }
        black_box(encoder.finish().unwrap());
    });
    let mut encoder = Encoder::new(&mut entropy_packet);
    for symbol in 0..32 {
        encoder.symbol(symbol % 3, model).unwrap();
    }
    let length = encoder.finish().unwrap();
    measure("entropy_decode_32_symbols", || {
        let mut decoder = Decoder::new(black_box(&entropy_packet[..length]));
        for _ in 0..32 {
            black_box(decoder.symbol(model));
        }
    });
    for length in [60, 120, 240, 480] {
        let plan = FftPlan::new(length).unwrap();
        let input: Vec<_> = (0..length)
            .map(|index| Complex {
                real: (index as f32 * 0.17).sin(),
                imaginary: 0.0,
            })
            .collect();
        let mut output = vec![Complex::ZERO; length];
        measure(&format!("fft_forward_{length}"), || {
            plan.forward(black_box(&input), &mut output).unwrap();
            black_box(&output);
        });
    }
    for length in [120, 240, 480, 960] {
        let plan = MdctPlan::new(length, 120).unwrap();
        let mut workspace = MdctWorkspace::new(&plan);
        let input: Vec<_> = (0..2 * length)
            .map(|index| (index as f32 * 0.17).sin())
            .collect();
        let mut coefficients = vec![0.0; length];
        let mut output = vec![0.0; 2 * length];
        measure(&format!("mdct_forward_{length}"), || {
            plan.forward(black_box(&input), &mut coefficients, &mut workspace)
                .unwrap();
            black_box(&coefficients);
        });
        measure(&format!("mdct_inverse_{length}"), || {
            plan.inverse(black_box(&coefficients), &mut output, &mut workspace)
                .unwrap();
            black_box(&output);
        });
    }
}
