// SPDX-License-Identifier: MIT
use snacka::entropy::{Decoder, Encoder, Model};
use snacka::g711::Law;
use snacka::opus_packet::Packet;
use snacka::transform::{Complex, FftPlan, MdctPlan, MdctWorkspace};
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::hint::black_box;

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
    let mut encoded = [0; 160];
    let mut decoded = [0; 160];
    let packet = [0xf8, 0x12, 0x34];
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    for _ in 0..1_000 {
        for law in [Law::A, Law::Mu] {
            law.encode_into(black_box(&decoded), &mut encoded).unwrap();
            law.decode_into(black_box(&encoded), &mut decoded).unwrap();
        }
        let parsed = Packet::parse(black_box(&packet)).unwrap();
        black_box(parsed.frames().map(|frame| frame.len()).sum::<usize>());
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
    let allocations = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(allocations, 0);
}
