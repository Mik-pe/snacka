// SPDX-License-Identifier: MIT AND BSD-3-Clause
//! CELT normalized band shapes, RFC 6716 sections 4.3.4–4.3.6 and 5.3.8.
//!
//! The recursive coder, transforms and folding are owned Rust implementations.
//! Integer angle polynomial coefficients and the sequency permutation describe
//! normative wire decisions in Appendix A; they are format constants.
//! The Hybrid folding correction in RFC 8251 section 9 is included.

use super::{allocation, pvq, range};
use allocation::{AllocationPlan, BAND_EDGES, BANDS};
use range::Error;

const MAX_COEFFICIENTS: usize = 800;
const MAX_BAND: usize = 176;

#[derive(Clone, Copy)]
pub(crate) struct ShapeSettings {
    pub(crate) lm: u8,
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) transient: bool,
    pub(crate) spread: u8,
    /// Available arithmetic + raw-bit capacity, less any reserved trailer.
    pub(crate) total_eighth_bits: i32,
}

pub(crate) struct ShapeFrame<'a> {
    pub(crate) left: &'a mut [f32],
    pub(crate) right: Option<&'a mut [f32]>,
    /// Band amplitudes used to choose the intensity-stereo downmix.
    pub(crate) energies: &'a [[f32; BANDS]; 2],
    pub(crate) tf: &'a [i8; BANDS],
    pub(crate) allocation: &'a AllocationPlan,
}

/// Persistent caller-owned scratch; no heap activity in shape coding.
pub(crate) struct ShapeWorkspace {
    normal: [[f32; MAX_COEFFICIENTS]; 2],
    pulses: [i16; MAX_BAND],
    pub(crate) collapse_masks: [[u8; BANDS]; 2],
    pub(crate) seed: u32,
}

impl ShapeWorkspace {
    pub(crate) const fn new() -> Self {
        Self {
            normal: [[0.0; MAX_COEFFICIENTS]; 2],
            pulses: [0; MAX_BAND],
            collapse_masks: [[0; BANDS]; 2],
            seed: 0,
        }
    }
}

fn pulse_error(error: pvq::Error) -> Error {
    match error {
        pvq::Error::Entropy(error) => error,
        _ => Error::Argument,
    }
}

#[derive(Clone, Copy)]
enum AngleModel {
    Uniform,
    Triangular,
    Stereo,
}

impl AngleModel {
    fn total(self, levels: u32) -> u32 {
        match self {
            Self::Uniform => levels + 1,
            Self::Triangular => (levels / 2 + 1).pow(2),
            Self::Stereo => 4 * (levels / 2) + 3,
        }
    }

    fn interval(self, value: u32, levels: u32) -> (u32, u32) {
        match self {
            Self::Uniform => (value, value + 1),
            Self::Stereo if value <= levels / 2 => (3 * value, 3 * value + 3),
            Self::Stereo => {
                let low = 3 * (levels / 2 + 1) + value - levels / 2 - 1;
                (low, low + 1)
            }
            Self::Triangular if value <= levels / 2 => {
                let low = value * (value + 1) / 2;
                (low, low + value + 1)
            }
            Self::Triangular => {
                let width = levels + 1 - value;
                let low = self.total(levels) - width * (width + 1) / 2;
                (low, low + width)
            }
        }
    }

    fn locate(self, frequency: u32, levels: u32) -> u32 {
        // At most eight steps for the 256-level maximum. Binary search avoids
        // float square-root rounding in triangular entropy boundaries.
        let mut low = 0;
        let mut high = levels;
        while low < high {
            let middle = (low + high) / 2;
            if frequency < self.interval(middle, levels).1 {
                high = middle;
            } else {
                low = middle + 1;
            }
        }
        low
    }
}

trait ShapeCoder {
    fn encoding(&self) -> bool;
    fn tell(&self) -> i32;
    fn raw(&mut self, value: u32, bits: u32) -> Result<u32, Error>;
    fn bit(&mut self, value: bool, logp: u32) -> Result<bool, Error>;
    fn angle(&mut self, value: u32, levels: u32, model: AngleModel) -> Result<u32, Error>;
    fn vector(
        &mut self,
        values: &mut [f32],
        count: usize,
        scratch: &mut [i16],
    ) -> Result<(), Error>;
}

impl ShapeCoder for range::Encoder<'_> {
    fn encoding(&self) -> bool {
        true
    }
    fn tell(&self) -> i32 {
        self.tell_frac() as i32
    }
    fn raw(&mut self, value: u32, bits: u32) -> Result<u32, Error> {
        range::Encoder::raw(self, value, bits)?;
        Ok(value)
    }
    fn bit(&mut self, value: bool, logp: u32) -> Result<bool, Error> {
        range::Encoder::bit(self, value, logp)?;
        Ok(value)
    }
    fn angle(&mut self, value: u32, levels: u32, model: AngleModel) -> Result<u32, Error> {
        if matches!(model, AngleModel::Uniform) {
            self.uniform(value, levels + 1)?;
        } else {
            let (low, high) = model.interval(value, levels);
            self.interval(low, high, model.total(levels))?;
        }
        Ok(value)
    }
    fn vector(
        &mut self,
        values: &mut [f32],
        count: usize,
        scratch: &mut [i16],
    ) -> Result<(), Error> {
        let book = pvq::Codebook::new(values.len(), count).map_err(pulse_error)?;
        pvq::quantize(values, count, scratch).map_err(pulse_error)?;
        self.uniform(book.index_of(scratch).map_err(pulse_error)?, book.total())?;
        pvq::synthesize(scratch, 1.0, values).map_err(pulse_error)
    }
}

impl ShapeCoder for range::Decoder<'_> {
    fn encoding(&self) -> bool {
        false
    }
    fn tell(&self) -> i32 {
        self.tell_frac() as i32
    }
    fn raw(&mut self, _value: u32, bits: u32) -> Result<u32, Error> {
        range::Decoder::raw(self, bits)
    }
    fn bit(&mut self, _value: bool, logp: u32) -> Result<bool, Error> {
        range::Decoder::bit(self, logp)
    }
    fn angle(&mut self, _value: u32, levels: u32, model: AngleModel) -> Result<u32, Error> {
        if matches!(model, AngleModel::Uniform) {
            return self.uniform(levels + 1);
        }
        let total = model.total(levels);
        let value = model.locate(self.frequency(total)?, levels);
        let (low, high) = model.interval(value, levels);
        self.update(low, high, total)?;
        Ok(value)
    }
    fn vector(
        &mut self,
        values: &mut [f32],
        count: usize,
        scratch: &mut [i16],
    ) -> Result<(), Error> {
        let book = pvq::Codebook::new(values.len(), count).map_err(pulse_error)?;
        book.vector_at(self.uniform(book.total())?, scratch)
            .map_err(pulse_error)?;
        pvq::synthesize(scratch, 1.0, values).map_err(pulse_error)
    }
}

struct Context<'a, C> {
    coder: &'a mut C,
    pulses: &'a mut [i16; MAX_BAND],
    seed: &'a mut u32,
    remaining: i32,
}

#[derive(Clone, Copy)]
struct Partition {
    band: usize,
    lm: i8,
    blocks: usize,
    spread: u8,
    gain: f32,
    fill: u16,
}

fn random(seed: &mut u32) -> u32 {
    *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    *seed
}

fn block_mask(blocks: usize) -> u16 {
    ((1u32 << blocks) - 1) as u16
}

// A rounded signed Q15 product. The Appendix A coefficients below are wire
// constants: replacing these with libm changes recursive bit allocations.
fn q15(a: i32, b: i32) -> i32 {
    (a * b + 16_384) >> 15
}

fn angle_cos(angle: i32) -> i32 {
    let square = (angle * angle + 4096) >> 13;
    let polynomial = q15(square, -626) + 8277;
    let polynomial = q15(square, polynomial) - 7651;
    32768 - square + q15(square, polynomial)
}

fn angle_log_ratio(side: i32, mid: i32) -> i32 {
    let side_log = 32 - side.leading_zeros() as i32;
    let mid_log = 32 - mid.leading_zeros() as i32;
    let normalized_side = side << (15 - side_log);
    let normalized_mid = mid << (15 - mid_log);
    let approximation = |value| q15(value, 7932 + q15(value, -2597));
    (side_log - mid_log) * 2048 + approximation(normalized_side) - approximation(normalized_mid)
}

fn angle_levels(n: usize, bits: i32, pulse_cap: i32, stereo: bool) -> u32 {
    // Table entries are rounded 2^(j/8) in Q14; only eight wire constants.
    const EXP: [i32; 8] = [16384, 17866, 19483, 21247, 23170, 25267, 27554, 30048];
    let dof = 2 * n as i32 - 1 - i32::from(stereo && n == 2);
    let offset = pulse_cap / 2 - if stereo && n == 2 { 16 } else { 4 };
    let precision = ((bits + dof * offset) / dof)
        .min(bits - pulse_cap - 32)
        .min(64);
    if precision < 4 {
        return 1;
    }
    let levels = EXP[(precision & 7) as usize] >> (14 - (precision >> 3));
    ((levels + 1) & !1) as u32
}

fn requested_angle(left: &[f32], right: &[f32], stereo: bool) -> i32 {
    let (mut first, mut second) = (1e-15f64, 1e-15f64);
    for (&a, &b) in left.iter().zip(right) {
        let a = f64::from(a);
        let b = f64::from(b);
        let (a, b) = if stereo {
            ((a + b) * 0.5, (a - b) * 0.5)
        } else {
            (a, b)
        };
        first += a * a;
        second += b * b;
    }
    (second.sqrt().atan2(first.sqrt()) * (32768.0 / std::f64::consts::PI)).round() as i32
}

#[derive(Clone, Copy)]
struct SplitGain {
    angle: i32,
    mid: f32,
    side: f32,
    delta: i32,
    spent: i32,
}

struct SplitRequest {
    n: usize,
    bits: i32,
    wanted: i32,
    stereo: bool,
    intensity: bool,
    temporal: bool,
}

fn split_gain<C: ShapeCoder>(
    context: &mut Context<'_, C>,
    spec: Partition,
    request: SplitRequest,
) -> Result<SplitGain, Error> {
    let SplitRequest {
        n,
        bits,
        wanted,
        stereo,
        intensity,
        temporal,
    } = request;
    let cap = allocation::band_log2_eighths(spec.band)? + i32::from(spec.lm) * 8;
    let levels = if intensity {
        1
    } else {
        angle_levels(n, bits, cap, stereo)
    };
    let before = context.coder.tell();
    let angle = if levels == 1 {
        0
    } else {
        let model = if stereo && n > 2 {
            AngleModel::Stereo
        } else if stereo || temporal {
            AngleModel::Uniform
        } else {
            AngleModel::Triangular
        };
        let index = ((wanted as u32 * levels + 8192) >> 14).min(levels);
        (context.coder.angle(index, levels, model)? * 16384 / levels) as i32
    };
    let (mid, side, delta) = match angle {
        0 => (32767, 0, -16384),
        16384 => (0, 32767, 16384),
        _ => {
            let mid = angle_cos(angle);
            let side = angle_cos(16384 - angle);
            (
                mid,
                side,
                q15((n as i32 - 1) * 128, angle_log_ratio(side, mid)),
            )
        }
    };
    Ok(SplitGain {
        angle,
        mid: mid as f32 / 32768.0,
        side: side as f32 / 32768.0,
        delta,
        spent: context.coder.tell() - before,
    })
}

/// The normative pair sweep rotates overlapping pairs across the whole block.
fn rotation(values: &mut [f32], stride: usize, c: f32, s: f32) {
    let pair = |values: &mut [f32], i: usize| {
        let a = values[i];
        let b = values[i + stride];
        values[i] = c * a - s * b;
        values[i + stride] = s * a + c * b;
    };
    for i in 0..values.len() - stride {
        pair(values, i);
    }
    if let Some(last) = values.len().checked_sub(2 * stride + 1) {
        for i in (0..=last).rev() {
            pair(values, i);
        }
    }
}

fn spread(values: &mut [f32], pulses: usize, blocks: usize, strength: u8, encode: bool) {
    if strength == 0 || 2 * pulses >= values.len() {
        return;
    }
    let factor = [0, 15, 10, 5][strength as usize];
    let ratio = values.len() as f64 / (values.len() + factor * pulses) as f64;
    let angle = std::f64::consts::FRAC_PI_4 * ratio * ratio;
    let (s, c) = angle.sin_cos();
    let (s, c) = (s as f32, c as f32);
    let width = values.len() / blocks;
    let stride = if width >= 8 {
        (width as f64).sqrt().round() as usize
    } else {
        0
    };
    for block in values.chunks_exact_mut(width) {
        if encode {
            rotation(block, 1, c, -s);
            if stride != 0 {
                rotation(block, stride, s, -c);
            }
        } else {
            if stride != 0 {
                rotation(block, stride, s, c);
            }
            rotation(block, 1, c, s);
        }
    }
}

fn collapse(pulses: &[i16], blocks: usize) -> u16 {
    let width = pulses.len() / blocks;
    pulses
        .chunks_exact(width)
        .enumerate()
        .fold(0, |mask, (i, values)| {
            mask | (u16::from(values.iter().any(|&value| value != 0)) << i)
        })
}

fn empty<C: ShapeCoder>(
    context: &mut Context<'_, C>,
    values: &mut [f32],
    spec: Partition,
    fold: Option<&[f32]>,
) -> Result<u16, Error> {
    let fill = spec.fill & block_mask(spec.blocks);
    if fill == 0 {
        values.fill(0.0);
        return Ok(0);
    }
    let mask = if let Some(fold) = fold {
        for (value, &source) in values.iter_mut().zip(fold) {
            let dither = if random(context.seed) & 0x8000 != 0 {
                1.0
            } else {
                -1.0
            } / 256.0;
            *value = source + dither;
        }
        fill
    } else {
        for value in values.iter_mut() {
            *value = (random(context.seed) as i32 >> 20) as f32;
        }
        block_mask(spec.blocks)
    };
    pvq::normalize(values, spec.gain).map_err(pulse_error)?;
    Ok(mask)
}

fn partition<C: ShapeCoder>(
    context: &mut Context<'_, C>,
    mut spec: Partition,
    values: &mut [f32],
    bits: i32,
    fold: Option<&[f32]>,
) -> Result<u16, Error> {
    if values.len() == 1 {
        let sign = if context.remaining >= 8 {
            context.remaining -= 8;
            context.coder.raw(u32::from(values[0] < 0.0), 1)? != 0
        } else {
            false
        };
        values[0] = if sign { -1.0 } else { 1.0 };
        return Ok(1);
    }
    let maximum = allocation::max_pulse_index(spec.band, spec.lm)?;
    let maximum_bits = allocation::pulse_bits(spec.band, spec.lm, maximum)?;
    if spec.lm >= 0 && values.len() > 2 && bits > maximum_bits + 11 {
        let original_blocks = spec.blocks;
        let half = values.len() / 2;
        let (left, right) = values.split_at_mut(half);
        spec.lm -= 1;
        if original_blocks == 1 {
            spec.fill = (spec.fill & 1) * 3;
        }
        spec.blocks = original_blocks.div_ceil(2);
        let wanted = if context.coder.encoding() {
            requested_angle(left, right, false)
        } else {
            0
        };
        let gain = split_gain(
            context,
            spec,
            SplitRequest {
                n: half,
                bits,
                wanted,
                stereo: false,
                intensity: false,
                temporal: original_blocks > 1,
            },
        )?;
        context.remaining -= gain.spent;
        let bits = bits - gain.spent;
        let mut delta = gain.delta;
        if original_blocks > 1 && gain.angle != 0 && gain.angle != 16384 {
            if gain.angle > 8192 {
                delta -= delta >> (4 - spec.lm);
            } else {
                delta = (delta + ((half as i32 * 8) >> (5 - spec.lm))).min(0);
            }
        }
        let mut left_bits = ((bits - delta) / 2).clamp(0, bits.max(0));
        let mut right_bits = bits - left_bits;
        let valid = block_mask(spec.blocks);
        let mut left_spec = Partition {
            gain: spec.gain * gain.mid,
            fill: spec.fill & valid,
            ..spec
        };
        let mut right_spec = Partition {
            gain: spec.gain * gain.side,
            fill: spec.fill >> spec.blocks,
            ..spec
        };
        if gain.angle == 0 {
            right_spec.fill = 0;
        }
        if gain.angle == 16384 {
            left_spec.fill = 0;
        }
        let (left_fold, right_fold) = fold.map_or((None, None), |values| {
            let (a, b) = values.split_at(half);
            (Some(a), Some(b))
        });
        let before = context.remaining;
        let (left_mask, right_mask) = if left_bits >= right_bits {
            let left_mask = partition(context, left_spec, left, left_bits, left_fold)?;
            let unused = left_bits - (before - context.remaining);
            if unused > 24 && gain.angle != 0 {
                right_bits += unused - 24;
            }
            (
                left_mask,
                partition(context, right_spec, right, right_bits, right_fold)?,
            )
        } else {
            let right_mask = partition(context, right_spec, right, right_bits, right_fold)?;
            let unused = right_bits - (before - context.remaining);
            if unused > 24 && gain.angle != 16384 {
                left_bits += unused - 24;
            }
            (
                partition(context, left_spec, left, left_bits, left_fold)?,
                right_mask,
            )
        };
        return Ok(left_mask | (right_mask << (original_blocks / 2)));
    }
    let mut index = allocation::pulse_index(spec.band, spec.lm, bits)?;
    let mut used = allocation::pulse_bits(spec.band, spec.lm, index)?;
    while used > context.remaining && index > 0 {
        index -= 1;
        used = allocation::pulse_bits(spec.band, spec.lm, index)?;
    }
    context.remaining -= used;
    if index == 0 {
        return empty(context, values, spec, fold);
    }
    let count = allocation::pulses_from_index(index)? as usize;
    if context.coder.encoding() {
        pvq::normalize(values, 1.0).map_err(pulse_error)?;
        spread(values, count, spec.blocks, spec.spread, true);
    }
    let pulses = &mut context.pulses[..values.len()];
    context.coder.vector(values, count, pulses)?;
    let mask = collapse(pulses, spec.blocks);
    spread(values, count, spec.blocks, spec.spread, false);
    for value in values {
        *value *= spec.gain;
    }
    Ok(mask)
}

fn haar(values: &mut [f32], width: usize, stride: usize) {
    for lane in 0..stride {
        for pair in 0..width / 2 {
            let first = 2 * pair * stride + lane;
            let second = first + stride;
            let a = values[first] * std::f32::consts::FRAC_1_SQRT_2;
            let b = values[second] * std::f32::consts::FRAC_1_SQRT_2;
            values[first] = a + b;
            values[second] = a - b;
        }
    }
}

fn paired_mask(mask: u16) -> u16 {
    let mut paired = 0;
    for pair in 0..8 {
        paired |= u16::from(mask & (3 << (2 * pair)) != 0) << pair;
    }
    paired
}

fn unpaired_mask(mask: u16) -> u16 {
    let mut unpaired = 0;
    for pair in 0..8 {
        if mask & (1 << pair) != 0 {
            unpaired |= 3 << (2 * pair);
        }
    }
    unpaired
}

fn reorder(values: &mut [f32], width: usize, blocks: usize, sequency: bool, to_time: bool) {
    // Walsh-Hadamard sequency ordering from RFC 6716 Appendix A bands.c.
    const SEQUENCY: [usize; 30] = [
        1, 0, 3, 0, 2, 1, 7, 0, 4, 3, 6, 1, 5, 2, 15, 0, 8, 7, 12, 3, 11, 4, 14, 1, 9, 6, 13, 2,
        10, 5,
    ];
    let mut result = [0.0; MAX_BAND];
    for block in 0..blocks {
        let ordered = if sequency {
            SEQUENCY[blocks - 2 + block]
        } else {
            block
        };
        for bin in 0..width {
            let frequency = bin * blocks + block;
            let time = ordered * width + bin;
            let (from, to) = if to_time {
                (frequency, time)
            } else {
                (time, frequency)
            };
            result[to] = values[from];
        }
    }
    values.copy_from_slice(&result[..values.len()]);
}

fn mono_band<C: ShapeCoder>(
    context: &mut Context<'_, C>,
    mut spec: Partition,
    values: &mut [f32],
    bits: i32,
    fold: Option<&[f32]>,
    tf: i8,
) -> Result<u16, Error> {
    if values.len() == 1 {
        return partition(context, spec, values, bits, fold);
    }
    let original_blocks = spec.blocks;
    let long = original_blocks == 1;
    let mut fold_storage = [0.0; MAX_BAND];
    let mut fold = fold.map(|input| {
        fold_storage[..input.len()].copy_from_slice(input);
        &mut fold_storage[..input.len()]
    });
    let recombine = tf.max(0) as usize;
    for level in 0..recombine {
        if context.coder.encoding() {
            haar(values, values.len() >> level, 1 << level);
        }
        if let Some(fold) = fold.as_deref_mut() {
            haar(fold, fold.len() >> level, 1 << level);
        }
        spec.fill = paired_mask(spec.fill);
    }
    spec.blocks >>= recombine;
    if spec.blocks == 0 {
        return Err(Error::Argument);
    }
    let mut width = values.len() / spec.blocks;
    let mut divide = 0;
    while divide < usize::from(tf.unsigned_abs()) && tf < 0 && width.is_multiple_of(2) {
        if context.coder.encoding() {
            haar(values, width, spec.blocks);
        }
        if let Some(fold) = fold.as_deref_mut() {
            haar(fold, width, spec.blocks);
        }
        spec.fill |= spec.fill << spec.blocks;
        spec.blocks *= 2;
        width /= 2;
        divide += 1;
    }
    let resolution_blocks = spec.blocks;
    let reorder_blocks = resolution_blocks << recombine;
    if resolution_blocks > 1 {
        if context.coder.encoding() {
            reorder(values, width >> recombine, reorder_blocks, long, true);
        }
        if let Some(fold) = fold.as_deref_mut() {
            reorder(fold, width >> recombine, reorder_blocks, long, true);
        }
    }
    let mut mask = partition(context, spec, values, bits, fold.as_deref())?;
    if resolution_blocks > 1 {
        reorder(values, width >> recombine, reorder_blocks, long, false);
    }
    for _ in 0..divide {
        spec.blocks /= 2;
        width *= 2;
        mask |= mask >> spec.blocks;
        haar(values, width, spec.blocks);
    }
    for level in 0..recombine {
        mask = unpaired_mask(mask);
        haar(values, values.len() >> level, 1 << level);
    }
    Ok(mask & block_mask(original_blocks))
}

fn stereo_merge(left: &mut [f32], right: &mut [f32], mid: f32) {
    let mut product = 0.0;
    let mut side_energy = 0.0;
    for (&a, &b) in left.iter().zip(right.iter()) {
        product += a * b;
        side_energy += b * b;
    }
    let first_energy = mid * mid + side_energy - 2.0 * mid * product;
    let second_energy = mid * mid + side_energy + 2.0 * mid * product;
    if first_energy < 6e-4 || second_energy < 6e-4 {
        right.copy_from_slice(left);
        return;
    }
    let first_scale = first_energy.sqrt().recip();
    let second_scale = second_energy.sqrt().recip();
    for (a, b) in left.iter_mut().zip(right) {
        let common = *a * mid;
        let difference = *b;
        *a = (common - difference) * first_scale;
        *b = (common + difference) * second_scale;
    }
}

fn save_fold(values: &[f32], output: &mut [f32]) {
    let scale = (values.len() as f32).sqrt();
    for (output, &value) in output.iter_mut().zip(values) {
        *output = value * scale;
    }
}

struct StereoBand<'a> {
    left: &'a mut [f32],
    right: &'a mut [f32],
    bits: i32,
    fold: Option<&'a [f32]>,
    tf: i8,
    intensity: bool,
    amplitudes: [f32; 2],
    fold_output: &'a mut [f32],
}

fn stereo_band<C: ShapeCoder>(
    context: &mut Context<'_, C>,
    spec: Partition,
    stereo: StereoBand<'_>,
) -> Result<u16, Error> {
    let StereoBand {
        left,
        right,
        bits,
        fold,
        tf,
        intensity,
        amplitudes,
        fold_output,
    } = stereo;
    if left.len() == 1 {
        let first = partition(context, spec, left, bits, fold)?;
        save_fold(left, fold_output);
        return Ok(first | partition(context, spec, right, bits, fold)?);
    }
    let wanted = if context.coder.encoding() {
        requested_angle(left, right, true)
    } else {
        0
    };
    let cap = allocation::band_log2_eighths(spec.band)? + i32::from(spec.lm) * 8;
    let levels = if intensity {
        1
    } else {
        angle_levels(left.len(), bits, cap, true)
    };
    let before = context.coder.tell();
    let mut inverse = false;
    if levels == 1 {
        if context.coder.encoding() && wanted > 8192 {
            inverse = true;
            for value in right.iter_mut() {
                *value = -*value;
            }
        }
        if bits > 16 && context.remaining > 16 {
            inverse = context.coder.bit(inverse, 2)?;
        }
    }
    let mut gain = split_gain(
        context,
        spec,
        SplitRequest {
            n: left.len(),
            bits,
            wanted,
            stereo: true,
            intensity,
            temporal: spec.blocks > 1,
        },
    )?;
    gain.spent = context.coder.tell() - before;
    if context.coder.encoding() {
        if gain.angle == 0 {
            let norm =
                (amplitudes[0] * amplitudes[0] + amplitudes[1] * amplitudes[1]).sqrt() + 1e-15;
            let a = amplitudes[0] / norm;
            let b = amplitudes[1] / norm;
            for (left, &right) in left.iter_mut().zip(right.iter()) {
                *left = a * *left + b * right;
            }
        } else {
            for (a, b) in left.iter_mut().zip(right.iter_mut()) {
                let first = *a * std::f32::consts::FRAC_1_SQRT_2;
                let second = *b * std::f32::consts::FRAC_1_SQRT_2;
                *a = first + second;
                *b = second - first;
            }
        }
    }
    context.remaining -= gain.spent;
    let bits = bits - gain.spent;
    if left.len() == 2 {
        let side_bits = if gain.angle != 0 && gain.angle != 16384 {
            8
        } else {
            0
        };
        let sign = if side_bits != 0 {
            context.remaining -= 8;
            let requested = if gain.angle > 8192 {
                right[0] * left[1] - right[1] * left[0] < 0.0
            } else {
                left[0] * right[1] - left[1] * right[0] < 0.0
            };
            if context.coder.raw(u32::from(requested), 1)? == 0 {
                1.0
            } else {
                -1.0
            }
        } else {
            1.0
        };
        let mask = if gain.angle > 8192 {
            let mask = mono_band(context, spec, right, bits - side_bits, fold, tf)?;
            save_fold(right, fold_output);
            left[0] = -sign * right[1];
            left[1] = sign * right[0];
            mask
        } else {
            let mask = mono_band(context, spec, left, bits - side_bits, fold, tf)?;
            save_fold(left, fold_output);
            right[0] = -sign * left[1];
            right[1] = sign * left[0];
            mask
        };
        for (a, b) in left.iter_mut().zip(right.iter_mut()) {
            let common = *a * gain.mid;
            let side = *b * gain.side;
            *a = common - side;
            *b = common + side;
        }
        if inverse {
            for value in right {
                *value = -*value;
            }
        }
        return Ok(mask);
    }
    let mut mid_bits = ((bits - gain.delta) / 2).clamp(0, bits.max(0));
    let mut side_bits = bits - mid_bits;
    let mid_spec = Partition {
        gain: 1.0,
        fill: if gain.angle == 16384 { 0 } else { spec.fill },
        ..spec
    };
    let side_spec = Partition {
        gain: gain.side,
        fill: 0,
        ..spec
    };
    let before = context.remaining;
    let mask = if mid_bits >= side_bits {
        let first = mono_band(context, mid_spec, left, mid_bits, fold, tf)?;
        let unused = mid_bits - (before - context.remaining);
        if unused > 24 && gain.angle != 0 {
            side_bits += unused - 24;
        }
        first | mono_band(context, side_spec, right, side_bits, None, tf)?
    } else {
        let second = mono_band(context, side_spec, right, side_bits, None, tf)?;
        let unused = side_bits - (before - context.remaining);
        if unused > 24 && gain.angle != 16384 {
            mid_bits += unused - 24;
        }
        second | mono_band(context, mid_spec, left, mid_bits, fold, tf)?
    };
    save_fold(left, fold_output);
    stereo_merge(left, right, gain.mid);
    if inverse {
        for value in right {
            *value = -*value;
        }
    }
    Ok(mask)
}

fn shapes<C: ShapeCoder>(
    coder: &mut C,
    settings: ShapeSettings,
    frame: ShapeFrame<'_>,
    workspace: &mut ShapeWorkspace,
) -> Result<(), Error> {
    if settings.lm > 3 {
        return Err(Error::Argument);
    }
    let channels = 1 + usize::from(frame.right.is_some());
    let multiplier = 1usize << settings.lm;
    let coefficients = BAND_EDGES[settings.end.min(BANDS)] * multiplier;
    if settings.start >= settings.end
        || settings.end > BANDS
        || settings.spread > 3
        || settings.total_eighth_bits < 0
        || frame.left.len() < coefficients
        || frame
            .right
            .as_ref()
            .is_some_and(|right| right.len() < coefficients)
        || frame.allocation.coded_bands > settings.end
        || frame.allocation.coded_bands < settings.start
        || frame.tf.iter().any(|&tf| {
            !(-3..=3).contains(&tf)
                || tf > settings.lm as i8
                || (!settings.transient && tf > 0)
                || (settings.transient && tf < -1)
        })
        || frame
            .energies
            .iter()
            .flatten()
            .any(|value| !value.is_finite() || *value < 0.0)
        || (coder.encoding()
            && (frame.left[..coefficients]
                .iter()
                .any(|value| !value.is_finite())
                || frame.right.as_ref().is_some_and(|right| {
                    right[..coefficients].iter().any(|value| !value.is_finite())
                })))
    {
        return Err(Error::Argument);
    }
    let blocks = if settings.transient { multiplier } else { 1 };
    let mut right = frame.right;
    let mut balance = frame.allocation.balance_eighth_bits;
    let mut lowband_offset = 0;
    let mut update_lowband = true;
    let mut dual_stereo = frame.allocation.dual_stereo;
    workspace.collapse_masks = [[0; BANDS]; 2];
    workspace.normal = [[0.0; MAX_COEFFICIENTS]; 2];
    let mut context = Context {
        coder,
        pulses: &mut workspace.pulses,
        seed: &mut workspace.seed,
        remaining: 0,
    };
    for band in settings.start..settings.end {
        let begin = multiplier * BAND_EDGES[band];
        let end = multiplier * BAND_EDGES[band + 1];
        let n = end - begin;
        let tell = context.coder.tell();
        if band != settings.start {
            balance -= tell;
        }
        context.remaining = settings.total_eighth_bits - tell - 1;
        let bits = if band < frame.allocation.coded_bands {
            let adjustment = balance / (frame.allocation.coded_bands - band).min(3) as i32;
            (frame.allocation.shape_eighth_bits[band] + adjustment)
                .min(context.remaining + 1)
                .clamp(0, 16383)
        } else {
            0
        };
        if (begin >= multiplier * BAND_EDGES[settings.start] + n || band == settings.start + 1)
            && (update_lowband || lowband_offset == 0)
        {
            lowband_offset = band;
        }
        if band == settings.start + 1 {
            // RFC 8251 section 9: Hybrid begins with an 8-bin band followed
            // by a 12-bin band. Extend the previous band's folding template
            // with its trailing four bins rather than generating new noise.
            // Equal-width CELT starting bands require no extension.
            let first_begin = multiplier * BAND_EDGES[settings.start];
            let first_width = begin - first_begin;
            if n > first_width {
                let source_begin = begin - (n - first_width);
                for normal in workspace.normal.iter_mut().take(channels) {
                    normal.copy_within(source_begin..begin, begin);
                }
            }
        }
        if dual_stereo && band == frame.allocation.intensity {
            dual_stereo = false;
            for i in multiplier * BAND_EDGES[settings.start]..begin {
                workspace.normal[0][i] = 0.5 * (workspace.normal[0][i] + workspace.normal[1][i]);
            }
        }
        let mut fill = [block_mask(blocks); 2];
        let mut folds = [[0.0; MAX_BAND]; 2];
        let source = if lowband_offset != 0
            && (settings.spread != 3 || blocks > 1 || frame.tf[band] < 0)
        {
            let source = (multiplier * BAND_EDGES[lowband_offset])
                .saturating_sub(n)
                .max(multiplier * BAND_EDGES[settings.start]);
            let first = BAND_EDGES[..lowband_offset]
                .partition_point(|&edge| multiplier * edge <= source)
                .saturating_sub(1);
            let last = BAND_EDGES[..=band].partition_point(|&edge| multiplier * edge < source + n);
            fill = [0; 2];
            for previous in first..last {
                for (channel, fill) in fill.iter_mut().enumerate().take(channels) {
                    *fill |= u16::from(workspace.collapse_masks[channel][previous]);
                }
            }
            for (channel, fold) in folds.iter_mut().enumerate().take(channels) {
                fold[..n].copy_from_slice(&workspace.normal[channel][source..source + n]);
            }
            true
        } else {
            false
        };
        let spec = Partition {
            band,
            lm: settings.lm as i8,
            blocks,
            spread: settings.spread,
            gain: 1.0,
            fill: fill[0],
        };
        let left = &mut frame.left[begin..end];
        let left_fold = source.then_some(&folds[0][..n]);
        let (left_mask, right_mask) = if let Some(right) = right.as_deref_mut() {
            let right = &mut right[begin..end];
            if dual_stereo {
                let first = mono_band(
                    &mut context,
                    spec,
                    left,
                    bits / 2,
                    left_fold,
                    frame.tf[band],
                )?;
                let second = mono_band(
                    &mut context,
                    Partition {
                        fill: fill[1],
                        ..spec
                    },
                    right,
                    bits / 2,
                    source.then_some(&folds[1][..n]),
                    frame.tf[band],
                )?;
                (first, second)
            } else {
                let mask = stereo_band(
                    &mut context,
                    Partition {
                        fill: fill[0] | fill[1],
                        ..spec
                    },
                    StereoBand {
                        left,
                        right,
                        bits,
                        fold: left_fold,
                        tf: frame.tf[band],
                        intensity: band >= frame.allocation.intensity,
                        amplitudes: [frame.energies[0][band], frame.energies[1][band]],
                        fold_output: &mut workspace.normal[0][begin..end],
                    },
                )?;
                (mask, mask)
            }
        } else {
            (
                mono_band(&mut context, spec, left, bits, left_fold, frame.tf[band])?,
                0,
            )
        };
        workspace.collapse_masks[0][band] = left_mask as u8;
        workspace.collapse_masks[1][band] = right_mask as u8;
        if channels == 1 || dual_stereo {
            save_fold(left, &mut workspace.normal[0][begin..end]);
        }
        if dual_stereo && let Some(right) = right.as_ref() {
            save_fold(&right[begin..end], &mut workspace.normal[1][begin..end]);
        }
        balance += frame.allocation.shape_eighth_bits[band] + tell;
        update_lowband = bits > n as i32 * 8;
    }
    Ok(())
}

pub(crate) fn encode_shapes(
    coder: &mut range::Encoder<'_>,
    settings: ShapeSettings,
    frame: ShapeFrame<'_>,
    workspace: &mut ShapeWorkspace,
) -> Result<(), Error> {
    shapes(coder, settings, frame, workspace)
}

pub(crate) fn decode_shapes(
    coder: &mut range::Decoder<'_>,
    settings: ShapeSettings,
    frame: ShapeFrame<'_>,
    workspace: &mut ShapeWorkspace,
) -> Result<(), Error> {
    shapes(coder, settings, frame, workspace)
}

pub(crate) struct CollapseFrame<'a> {
    pub(crate) left: &'a mut [f32],
    pub(crate) right: Option<&'a mut [f32]>,
    pub(crate) current: &'a [[f32; BANDS]; 2],
    pub(crate) previous: &'a [[f32; BANDS]; 2],
    pub(crate) older: &'a [[f32; BANDS]; 2],
    pub(crate) allocation: &'a AllocationPlan,
}

/// Restore collapsed short blocks at the level permitted by recent energies.
/// The noise seed is intentionally a copy; only shape folding advances state.
pub(crate) fn anti_collapse(
    settings: ShapeSettings,
    frame: CollapseFrame<'_>,
    workspace: &ShapeWorkspace,
) -> Result<(), Error> {
    if settings.lm > 3 || settings.start >= settings.end || settings.end > BANDS {
        return Err(Error::Argument);
    }
    let blocks = 1usize << settings.lm;
    let required = BAND_EDGES[settings.end] * blocks;
    if frame.left.len() < required
        || frame
            .right
            .as_ref()
            .is_some_and(|right| right.len() < required)
        || frame
            .current
            .iter()
            .chain(frame.previous)
            .chain(frame.older)
            .flatten()
            .any(|value| !value.is_finite())
    {
        return Err(Error::Argument);
    }
    let channels = 1 + usize::from(frame.right.is_some());
    let mut right = frame.right;
    let mut seed = workspace.seed;
    for band in settings.start..settings.end {
        let width = BAND_EDGES[band + 1] - BAND_EDGES[band];
        let depth = (1 + frame.allocation.shape_eighth_bits[band]) / (width * blocks) as i32;
        let threshold = 0.5 * (-0.125 * depth as f32).exp2();
        for channel in 0..channels {
            let mut previous = frame.previous[channel][band];
            let mut older = frame.older[channel][band];
            if channels == 1 {
                previous = previous.max(frame.previous[1][band]);
                older = older.max(frame.older[1][band]);
            }
            let difference = (frame.current[channel][band] - previous.min(older)).max(0.0);
            let history_gain = 2.0
                * (-difference).exp2()
                * if settings.lm == 3 {
                    std::f32::consts::SQRT_2
                } else {
                    1.0
                };
            let amplitude = threshold.min(history_gain) / ((width * blocks) as f32).sqrt();
            let begin = BAND_EDGES[band] * blocks;
            let end = BAND_EDGES[band + 1] * blocks;
            let values = if channel == 0 {
                &mut frame.left[begin..end]
            } else {
                &mut right.as_deref_mut().ok_or(Error::Argument)?[begin..end]
            };
            let mut changed = false;
            for block in 0..blocks {
                if workspace.collapse_masks[channel][band] & (1 << block) != 0 {
                    continue;
                }
                for bin in 0..width {
                    values[bin * blocks + block] = if random(&mut seed) & 0x8000 != 0 {
                        amplitude
                    } else {
                        -amplitude
                    };
                }
                changed = true;
            }
            if changed {
                // The normative energy floor matters for deep allocations:
                // injected noise can be below 1e-7 in amplitude.
                let energy = values
                    .iter()
                    .fold(1e-15f32, |sum, value| sum + value * value);
                let scale = energy.sqrt().recip();
                for value in values {
                    *value *= scale;
                }
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plan(bits: i32, stereo: bool, intensity: usize, dual: bool) -> AllocationPlan {
        let _ = stereo;
        AllocationPlan {
            shape_eighth_bits: [bits; BANDS],
            fine_energy_bits: [0; BANDS],
            fine_priority: [0; BANDS],
            coded_bands: BANDS,
            intensity,
            dual_stereo: dual,
            balance_eighth_bits: 0,
        }
    }

    fn spectra(values: &mut [f32], lm: u8, offset: f32) {
        for band in 0..BANDS {
            let begin = BAND_EDGES[band] << lm;
            let end = BAND_EDGES[band + 1] << lm;
            for (bin, value) in values[begin..end].iter_mut().enumerate() {
                *value = ((bin as f32 + offset) * (0.13 + band as f32 * 0.027)).sin();
            }
            pvq::normalize(&mut values[begin..end], 1.0).unwrap();
        }
    }

    #[test]
    fn shape_roundtrips_cover_recursion_tf_stereo_and_folding() {
        for lm in 0..=3 {
            for transient in [false, true] {
                for stereo in [false, true] {
                    for spread in 0..=3 {
                        for (bits, intensity, dual) in
                            [(0, 0, false), (72, 15, true), (360, BANDS, false)]
                        {
                            let allocation = plan(bits, stereo, intensity, dual);
                            let mut left = [0.0; 960];
                            let mut right = [0.0; 960];
                            spectra(&mut left, lm, 0.8);
                            spectra(&mut right, lm, 2.7);
                            let mut restored = [[0.0; 960]; 2];
                            let mut encoder_workspace = ShapeWorkspace::new();
                            let mut decoder_workspace = ShapeWorkspace::new();
                            let mut tf = [0; BANDS];
                            if lm > 0 {
                                for (band, value) in tf.iter_mut().enumerate() {
                                    *value = if transient && band % 3 == 0 {
                                        lm as i8
                                    } else if band % 2 == 0 {
                                        -1
                                    } else {
                                        0
                                    };
                                }
                            }
                            let settings = ShapeSettings {
                                lm,
                                start: 0,
                                end: BANDS,
                                transient,
                                spread,
                                total_eighth_bits: 4000 * 64,
                            };
                            let energies = [[1.0; BANDS]; 2];
                            let mut packet = [0; 4000];
                            let mut encoder = range::Encoder::new(&mut packet);
                            encode_shapes(
                                &mut encoder,
                                settings,
                                ShapeFrame {
                                    left: &mut left,
                                    right: stereo.then_some(&mut right),
                                    energies: &energies,
                                    tf: &tf,
                                    allocation: &allocation,
                                },
                                &mut encoder_workspace,
                            )
                            .unwrap();
                            let range = encoder.range();
                            let tell = encoder.tell_frac();
                            encoder.finish_padded().unwrap();
                            let mut decoder = range::Decoder::new(&packet);
                            let [restored_left, restored_right] = &mut restored;
                            decode_shapes(
                                &mut decoder,
                                settings,
                                ShapeFrame {
                                    left: restored_left,
                                    right: stereo.then_some(restored_right),
                                    energies: &energies,
                                    tf: &tf,
                                    allocation: &allocation,
                                },
                                &mut decoder_workspace,
                            )
                            .unwrap();
                            assert_eq!(
                                (decoder.range(), decoder.tell_frac()),
                                (range, tell),
                                "lm={lm} transient={transient} stereo={stereo} spread={spread} bits={bits}"
                            );
                            assert_eq!(
                                encoder_workspace.collapse_masks,
                                decoder_workspace.collapse_masks
                            );
                            assert_eq!(encoder_workspace.seed, decoder_workspace.seed);
                            assert_eq!(left, restored[0]);
                            if stereo {
                                assert_eq!(right, restored[1]);
                            }
                            for band in 0..BANDS {
                                let begin = BAND_EDGES[band] << lm;
                                let end = BAND_EDGES[band + 1] << lm;
                                let norm: f32 = restored[0][begin..end]
                                    .iter()
                                    .map(|value| value * value)
                                    .sum();
                                assert!((norm - 1.0).abs() < 0.005, "band={band} norm={norm}");
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn anti_collapse_restores_each_short_block_and_preserves_seed() {
        let allocation = plan(0, false, BANDS, false);
        let settings = ShapeSettings {
            lm: 3,
            start: 0,
            end: BANDS,
            transient: true,
            spread: 2,
            total_eighth_bits: 6400,
        };
        let energy = [[0.0; BANDS]; 2];
        let mut values = [0.0; 960];
        let workspace = ShapeWorkspace::new();
        anti_collapse(
            settings,
            CollapseFrame {
                left: &mut values,
                right: None,
                current: &energy,
                previous: &energy,
                older: &energy,
                allocation: &allocation,
            },
            &workspace,
        )
        .unwrap();
        assert_eq!(workspace.seed, 0);
        for band in 0..BANDS {
            let begin = BAND_EDGES[band] * 8;
            let end = BAND_EDGES[band + 1] * 8;
            assert!(values[begin..end].iter().all(|value| *value != 0.0));
            let norm: f32 = values[begin..end].iter().map(|value| value * value).sum();
            assert!((norm - 1.0).abs() < 1e-5);
        }
    }

    #[test]
    fn angle_models_cover_their_alphabets() {
        for levels in (2..=256).step_by(2) {
            for model in [
                AngleModel::Triangular,
                AngleModel::Stereo,
                AngleModel::Uniform,
            ] {
                let mut previous = 0;
                for value in 0..=levels {
                    let (low, high) = model.interval(value, levels);
                    assert_eq!(low, previous);
                    assert_eq!(model.locate(low, levels), value);
                    assert_eq!(model.locate(high - 1, levels), value);
                    previous = high;
                }
                assert_eq!(previous, model.total(levels));
            }
        }
    }

    #[test]
    fn normative_shape_decoder_oracle() {
        let mut bytes = include_bytes!("testdata/bands-rfc6716.bin").as_slice();
        let word = |bytes: &mut &[u8]| {
            let (value, rest) = bytes.split_at(4);
            *bytes = rest;
            u32::from_le_bytes(value.try_into().unwrap())
        };
        let mut cases = 0;
        let mut maximum_error = 0.0f32;
        while !bytes.is_empty() {
            let lm = word(&mut bytes) as u8;
            let transient = word(&mut bytes) != 0;
            let channels = word(&mut bytes) as usize;
            let spread = word(&mut bytes) as u8;
            let choice = word(&mut bytes) as usize;
            let expected_range = word(&mut bytes);
            let expected_tell = word(&mut bytes);
            let expected_seed = word(&mut bytes);
            let expected_error = word(&mut bytes);
            let (expected_masks, rest) = bytes.split_at(42);
            bytes = rest;
            let mut allocation = plan(
                [0, 72, 360, 0][choice],
                channels == 2,
                [0, 15, BANDS, 6][choice],
                channels == 2 && choice == 1,
            );
            if choice == 3 {
                allocation.coded_bands = 13;
                allocation.shape_eighth_bits = [
                    31, 26, 26, 24, 21, 19, 17, 14, 25, 23, 16, 18, 25, 0, 0, 0, 0, 0, 0, 0, 0,
                ];
            }
            let mut packet = [0u8; 4000];
            for (index, value) in packet.iter_mut().enumerate() {
                *value = (index * 73
                    + usize::from(lm) * 17
                    + usize::from(transient) * 31
                    + channels * 49
                    + usize::from(spread) * 11
                    + choice * 29) as u8;
            }
            let mut tf = [0; BANDS];
            if lm > 0 && choice != 3 {
                for (band, value) in tf.iter_mut().enumerate() {
                    *value = if transient && band % 3 == 0 {
                        lm as i8
                    } else if band % 2 == 0 {
                        -1
                    } else {
                        0
                    };
                }
            }
            let settings = ShapeSettings {
                lm,
                start: 0,
                end: if choice == 3 { 13 } else { BANDS },
                transient,
                spread,
                total_eighth_bits: 4000 * 64,
            };
            let energies = [[1.0; BANDS]; 2];
            let mut workspace = ShapeWorkspace::new();
            let mut values = [[0.0; 960]; 2];
            let [left, right] = &mut values;
            let mut decoder = range::Decoder::new(&packet);
            let result = decode_shapes(
                &mut decoder,
                settings,
                ShapeFrame {
                    left,
                    right: (channels == 2).then_some(right),
                    energies: &energies,
                    tf: &tf,
                    allocation: &allocation,
                },
                &mut workspace,
            );
            if expected_error != 0 {
                assert!(result.is_err(), "case={cases} normative corrupt accepted");
                bytes = &bytes[channels * (100 << lm) * 4..];
                cases += 1;
                continue;
            }
            result.unwrap_or_else(|error| panic!("case={cases} lm={lm} transient={transient} channels={channels} spread={spread} choice={choice} error={error}"));
            assert_eq!(
                (decoder.range(), decoder.tell_frac() as u32, workspace.seed),
                (expected_range, expected_tell, expected_seed),
                "case={cases} lm={lm} transient={transient} channels={channels} spread={spread} choice={choice}"
            );
            for band in 0..BANDS {
                for channel in 0..channels {
                    assert_eq!(
                        workspace.collapse_masks[channel][band],
                        expected_masks[band * channels + channel],
                        "case={cases} mask band={band} channel={channel}"
                    );
                }
            }
            for (channel, values) in values.iter().enumerate().take(channels) {
                for (index, &value) in values.iter().enumerate().take(100 << lm) {
                    let expected = f32::from_bits(word(&mut bytes));
                    let error = (expected - value).abs();
                    maximum_error = maximum_error.max(error);
                    assert!(
                        error < 0.00001,
                        "case={cases} lm={lm} transient={transient} channels={channels} spread={spread} choice={choice} channel={channel} index={index} expected={expected} actual={value} error={error}"
                    );
                }
            }
            cases += 1;
        }
        assert_eq!(cases, 256);
        assert!(maximum_error < 0.00001);
    }

    #[test]
    fn normative_anti_collapse_oracle() {
        let mut bytes = include_bytes!("testdata/bands-anticollapse-rfc6716.bin").as_slice();
        let word = |bytes: &mut &[u8]| {
            let (value, rest) = bytes.split_at(4);
            *bytes = rest;
            u32::from_le_bytes(value.try_into().unwrap())
        };
        let mut cases = 0;
        while !bytes.is_empty() {
            let lm = word(&mut bytes) as u8;
            let channels = word(&mut bytes) as usize;
            let mut values = [[0.0; 960]; 2];
            for (channel, values) in values.iter_mut().enumerate() {
                for (i, value) in values.iter_mut().enumerate() {
                    *value = (((channel * 960 + i) * 17) % 31) as f32 / 50.0 - 15.0 / 50.0;
                }
            }
            let mut current = [[0.0; BANDS]; 2];
            let mut previous = current;
            let mut older = current;
            let mut allocation = plan(0, channels == 2, BANDS, false);
            let mut workspace = ShapeWorkspace::new();
            workspace.seed = 0xabcdef01;
            for band in 0..BANDS {
                allocation.shape_eighth_bits[band] = band as i32 * 29;
                for channel in 0..2 {
                    current[channel][band] = (band as f32 - 10.0) / 4.0;
                    previous[channel][band] = (band as f32 - 12.0 - channel as f32) / 8.0;
                    older[channel][band] = (band as f32 - 9.0 + channel as f32) / 16.0;
                    workspace.collapse_masks[channel][band] =
                        ((band * 13 + channel * 37) & ((1 << lm) - 1)) as u8;
                }
            }
            let settings = ShapeSettings {
                lm,
                start: 0,
                end: BANDS,
                transient: true,
                spread: 2,
                total_eighth_bits: 6400,
            };
            let [left, right] = &mut values;
            anti_collapse(
                settings,
                CollapseFrame {
                    left,
                    right: (channels == 2).then_some(right),
                    current: &current,
                    previous: &previous,
                    older: &older,
                    allocation: &allocation,
                },
                &workspace,
            )
            .unwrap();
            for (channel, values) in values.iter().enumerate().take(channels) {
                for (index, &value) in values.iter().enumerate().take(100 << lm) {
                    let expected = f32::from_bits(word(&mut bytes));
                    assert!(
                        (expected - value).abs() < 0.00001,
                        "case={cases} channel={channel} index={index} expected={expected} actual={value}"
                    );
                }
            }
            assert_eq!(workspace.seed, 0xabcdef01);
            cases += 1;
        }
        assert_eq!(cases, 8);
    }

    #[test]
    fn hybrid_start_17_matches_updated_reference_folding() {
        let fixture = include_bytes!("testdata/bands-hybrid-libopus-1.6.1.bin");
        assert_eq!(&fixture[..8], b"SNKHYB01");
        let mut bytes = &fixture[8..];
        let word = |bytes: &mut &[u8]| {
            let (value, rest) = bytes.split_at(4);
            *bytes = rest;
            u32::from_le_bytes(value.try_into().unwrap())
        };
        let count = word(&mut bytes);
        assert_eq!(count, 24);
        let mut valid = 0;
        for case in 0..count {
            let lm = word(&mut bytes) as u8;
            let transient = word(&mut bytes) != 0;
            let channels = word(&mut bytes) as usize;
            let choice = word(&mut bytes) as usize;
            let expected_range = word(&mut bytes);
            let expected_tell = word(&mut bytes);
            let expected_seed = word(&mut bytes);
            let expected_error = word(&mut bytes);
            let (expected_masks, rest) = bytes.split_at(42);
            bytes = rest;
            let allocation = plan(
                [0, 72, 360][choice],
                channels == 2,
                [17, 19, 21][choice],
                channels == 2 && choice == 1,
            );
            let mut packet = [0u8; 4000];
            for (index, value) in packet.iter_mut().enumerate() {
                *value = (index * 73
                    + usize::from(lm) * 17
                    + usize::from(transient) * 31
                    + channels * 49
                    + 2 * 11
                    + choice * 29) as u8;
            }
            let settings = ShapeSettings {
                lm,
                start: 17,
                end: 21,
                transient,
                spread: 2,
                total_eighth_bits: 4000 * 64,
            };
            let amplitudes = [[1.0; BANDS]; 2];
            let tf = [0; BANDS];
            let mut workspace = ShapeWorkspace::new();
            workspace.seed = 0xc0dec0de;
            let mut values = [[0.0; 960]; 2];
            let [left, right] = &mut values;
            let mut decoder = range::Decoder::new(&packet);
            let result = decode_shapes(
                &mut decoder,
                settings,
                ShapeFrame {
                    left,
                    right: (channels == 2).then_some(right),
                    energies: &amplitudes,
                    tf: &tf,
                    allocation: &allocation,
                },
                &mut workspace,
            );
            if expected_error != 0 {
                assert!(
                    result.is_err(),
                    "case={case} accepted invalid reference input"
                );
                bytes = &bytes[channels * (60 << lm) * 4..];
                continue;
            }
            result.unwrap();
            valid += 1;
            assert_eq!(
                (decoder.range(), decoder.tell_frac() as u32, workspace.seed),
                (expected_range, expected_tell, expected_seed),
                "case={case}"
            );
            for band in 17..BANDS {
                for channel in 0..channels {
                    assert_eq!(
                        workspace.collapse_masks[channel][band],
                        expected_masks[band * channels + channel],
                        "case={case} band={band} channel={channel}"
                    );
                }
            }
            for (channel, values) in values.iter().enumerate().take(channels) {
                assert!(values[..(40 << lm)].iter().all(|&value| value == 0.0));
                for (index, &value) in values.iter().enumerate().take(100 << lm).skip(40 << lm) {
                    let expected = f32::from_bits(word(&mut bytes));
                    assert!(
                        (expected - value).abs() < 1e-5,
                        "case={case} channel={channel} index={index} expected={expected} actual={value}"
                    );
                }
            }
        }
        assert!(valid >= 20);
        assert!(bytes.is_empty());
    }
}
