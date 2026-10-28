// SPDX-License-Identifier: MIT
// Consecutive lag lanes share one contiguous four-sample window.
#[inline(always)]
#[allow(clippy::needless_range_loop)]
fn correlations_consecutive4(samples:&[f32],window:usize,first:usize,step:usize,current_energy:f32)->[f32;4] {
 let start=samples.len()-window;
 let previous=&samples[start-first-3..samples.len()-first];
 let mut product=[0.0f32;4];let mut previous_energy=[0.0f32;4];
 for (&current,values) in samples[start..].iter().zip(previous.array_windows::<4>()).step_by(step) {
  for lane in 0..4 {
   let past=values[3-lane];
   product[lane]+=current*past;previous_energy[lane]+=past*past;
  }
 }
 std::array::from_fn(|lane|product[lane]/(current_energy*previous_energy[lane]).sqrt().max(1e-20))
}
#[cfg(test)]
mod consecutive_tests {
 use super::*;
 #[test]
 fn every_lane_preserves_scalar_sum_bits() {
  let mut seed=20261028u32;
  for case in 0..200 {
   let samples:Vec<f32>=(0..1920).map(|i|{seed^=seed<<13;seed^=seed>>17;seed^=seed<<5;match case%4 {0=>0.,1=>(i as f32*0.031).sin(),_=>(seed as f32/u32::MAX as f32-0.5)*0.5}}).collect();
   for (window,step,lo,hi) in [(160,1,14,156),(960,3,80,957)] {
    let view=if window==160 {&samples[..320]} else {&samples[..]};
    let total=energy(view,window,step);
    for first in (lo..=hi).step_by(if window==160 {4} else {2}) {
     let four=correlations_consecutive4(view,window,first,step,total);
     for (lane,&score) in four.iter().enumerate() {assert_eq!(score.to_bits(),correlation(view,window,first+lane,step,total).to_bits());}
    }
   }
  }
 }
}
