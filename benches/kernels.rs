// SPDX-License-Identifier: MIT
use snacka::entropy::{Decoder, Encoder, Model};
use snacka::transform::{Complex, FftPlan, MdctPlan, MdctWorkspace};
use snacka::{
    Config, Decoder as AudioDecoder, Encoder as AudioEncoder, FrameDuration, MAX_PACKET_BYTES,
    Packet,
};
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
#[link(name = "opus")]
unsafe extern "C" {
    fn opus_encoder_create(
        rate: i32,
        channels: i32,
        application: i32,
        error: *mut i32,
    ) -> *mut std::ffi::c_void;
    fn opus_encoder_ctl(encoder: *mut std::ffi::c_void, request: i32, ...) -> i32;
    fn opus_encoder_destroy(encoder: *mut std::ffi::c_void);
    fn opus_decoder_create(rate: i32, channels: i32, error: *mut i32) -> *mut std::ffi::c_void;
    fn opus_decoder_destroy(decoder: *mut std::ffi::c_void);
    fn opus_encode_float(
        encoder: *mut std::ffi::c_void,
        pcm: *const f32,
        samples: i32,
        bytes: *mut u8,
        capacity: i32,
    ) -> i32;
    fn opus_decode_float(
        decoder: *mut std::ffi::c_void,
        bytes: *const u8,
        length: i32,
        pcm: *mut f32,
        samples: i32,
        fec: i32,
    ) -> i32;
    fn opus_get_version_string() -> *const std::ffi::c_char;
}

#[cfg(feature = "reference-tests")]
fn reference_audio(input: &[f32], bitrate: i32, milliseconds: u32) {
    let mut error = 0;
    // SAFETY: Valid mono 48 kHz parameters and writable error storage; returned
    // objects are checked before use and destroyed after the benchmark.
    let encoder = unsafe { opus_encoder_create(48_000, 1, 2051, &mut error) };
    assert_eq!(error, 0);
    assert!(!encoder.is_null());
    // SAFETY: Valid decoder parameters and writable error storage.
    let decoder = unsafe { opus_decoder_create(48_000, 1, &mut error) };
    assert_eq!(error, 0);
    assert!(!decoder.is_null());
    // SAFETY: Control request argument types are integer bitrate, integer VBR
    // switch, integer complexity and writable lookahead pointer respectively.
    unsafe {
        assert_eq!(opus_encoder_ctl(encoder, 4002, bitrate), 0);
        assert_eq!(opus_encoder_ctl(encoder, 4006, 0i32), 0);
        assert_eq!(opus_encoder_ctl(encoder, 4010, 10i32), 0);
        let mut lookahead = 0i32;
        assert_eq!(
            opus_encoder_ctl(encoder, 4027, &mut lookahead as *mut i32),
            0
        );
        println!(
            "libopus_restricted_lowdelay,bitrate={bitrate},frame_ms={milliseconds},lookahead_samples={lookahead},complexity=10,vbr=0"
        );
    }
    let mut packet = [0; MAX_PACKET_BYTES];
    let mut output = vec![0.0; input.len()];
    let mut length = 0;
    measure(
        &format!("libopus_encode_{milliseconds}ms_{bitrate}"),
        || {
            // SAFETY: Input has one complete configured frame, output capacity
            // matches the array, and the encoder remains alive and exclusively used.
            length = unsafe {
                opus_encode_float(
                    encoder,
                    black_box(input).as_ptr(),
                    input.len() as i32,
                    packet.as_mut_ptr(),
                    packet.len() as i32,
                )
            };
            assert!(length > 0);
            black_box(&packet[..length as usize]);
        },
    );
    measure(
        &format!("libopus_decode_{milliseconds}ms_{bitrate}"),
        || {
            // SAFETY: The encoded prefix is initialized and output holds a complete
            // frame. The decoder remains alive and exclusively used.
            let samples = unsafe {
                opus_decode_float(
                    decoder,
                    black_box(&packet).as_ptr(),
                    length,
                    output.as_mut_ptr(),
                    output.len() as i32,
                    0,
                )
            };
            assert_eq!(samples as usize, input.len());
            black_box(&output);
        },
    );
    // SAFETY: Both opaque objects are live, owned here, and no longer used.
    unsafe {
        opus_encoder_destroy(encoder);
        opus_decoder_destroy(decoder);
    }
}

fn main() {
    println!(
        "1000 warmups; 1000 timed batches calibrated to >=200us; percentiles describe batch-average throughput, not individual tail latency"
    );
    #[cfg(feature = "reference-tests")]
    {
        // SAFETY: The reference API returns a static NUL-terminated version string.
        let version = unsafe { std::ffi::CStr::from_ptr(opus_get_version_string()) };
        println!("reference_version={}", version.to_str().unwrap());
    }
    for duration in [FrameDuration::Ms10, FrameDuration::Ms20] {
        let samples = duration.samples();
        let input: Vec<_> = (0..samples)
            .map(|i| 0.5 * (i as f32 * 0.0576).sin() + 0.1 * (i as f32 * 0.127).sin())
            .collect();
        for bitrate in [12_000, 24_000, 32_000] {
            let config = Config::new(bitrate).unwrap().with_frame_duration(duration);
            let mut encoder = AudioEncoder::new(config);
            let mut packet = [0; MAX_PACKET_BYTES];
            measure(
                &format!("snacka_encode_{}ms_{bitrate}", duration.milliseconds()),
                || {
                    let length = encoder.encode(black_box(&input), &mut packet).unwrap();
                    black_box(&packet[..length]);
                },
            );
            let mut source = AudioEncoder::new(config);
            let packets: Vec<Vec<u8>> = (0..256)
                .map(|_| {
                    let length = source.encode(&input, &mut packet).unwrap();
                    packet[..length].to_vec()
                })
                .collect();
            let mut decoder = AudioDecoder::with_frame_duration(duration);
            let mut output = vec![0.0; samples];
            let mut index = 0;
            measure(
                &format!("snacka_decode_{}ms_{bitrate}", duration.milliseconds()),
                || {
                    decoder
                        .decode(black_box(&packets[index]), &mut output)
                        .unwrap();
                    index = (index + 1) % 256;
                    black_box(&output);
                },
            );
            measure(
                &format!("snacka_parse_{}ms_{bitrate}", duration.milliseconds()),
                || {
                    black_box(Packet::parse(black_box(&packets[0])).unwrap());
                },
            );
            #[cfg(feature = "reference-tests")]
            reference_audio(&input, bitrate as i32, duration.milliseconds());
        }
    }
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
