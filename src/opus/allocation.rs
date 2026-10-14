// SPDX-License-Identifier: MIT AND BSD-3-Clause
//! CELT allocation in eighth-bit units, RFC 6716 sections 4.3.3 and 5.3.4.
//!
//! Algorithms are independently structured as quality-curve search, skip
//! signaling, coefficient redistribution, and energy/shape separation. The
//! integer constants below are normative format data from RFC 6716 Appendix A
//! (static_modes_float.h and rate.c), not an external codec implementation.
//! See the repository's RFC data license notice for their attribution.

use super::range::{Decoder, Encoder, Error};

pub(crate) const BANDS: usize = 21;
pub(crate) const BAND_EDGES: [usize; BANDS + 1] = [
    0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 12, 14, 16, 20, 24, 28, 34, 40, 48, 60, 78, 100,
];
const MAX_FINE_BITS: i32 = 8;
const FINE_OFFSET: i32 = 21;
const LOG2_BOUNDARIES: [i32; 22] = [
    0, 8, 13, 16, 19, 21, 23, 24, 26, 27, 28, 29, 30, 31, 32, 32, 33, 34, 34, 35, 36, 36,
];

// Table 57, transposed so the hot quality-curve search reads contiguous bands.
const STATIC_ALLOCATION: [[u8; BANDS]; 11] = [
    [
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        90, 80, 75, 69, 63, 56, 49, 40, 34, 29, 20, 18, 10, 0, 0, 0, 0, 0, 0, 0, 0,
    ],
    [
        110, 100, 90, 84, 78, 71, 65, 58, 51, 45, 39, 32, 26, 20, 12, 0, 0, 0, 0, 0, 0,
    ],
    [
        118, 110, 103, 93, 86, 80, 75, 70, 65, 59, 53, 47, 40, 31, 23, 15, 4, 0, 0, 0, 0,
    ],
    [
        126, 119, 112, 104, 95, 89, 83, 78, 72, 66, 60, 54, 47, 39, 32, 25, 17, 12, 1, 0, 0,
    ],
    [
        134, 127, 120, 114, 103, 97, 91, 85, 78, 72, 66, 60, 54, 47, 41, 35, 29, 23, 16, 10, 1,
    ],
    [
        144, 137, 130, 124, 113, 107, 101, 95, 88, 82, 76, 70, 64, 57, 51, 45, 39, 33, 26, 15, 1,
    ],
    [
        152, 145, 138, 132, 123, 117, 111, 105, 98, 92, 86, 80, 74, 67, 61, 55, 49, 43, 36, 20, 1,
    ],
    [
        162, 155, 148, 142, 133, 127, 121, 115, 108, 102, 96, 90, 84, 77, 71, 65, 59, 53, 46, 30, 1,
    ],
    [
        172, 165, 158, 152, 143, 137, 131, 125, 118, 112, 106, 100, 94, 87, 81, 75, 69, 63, 56, 45,
        20,
    ],
    [
        200, 200, 200, 200, 200, 200, 200, 200, 198, 193, 188, 183, 178, 173, 168, 163, 158, 153,
        148, 129, 104,
    ],
];

const BAND_LOG2_EIGHTHS: [i32; 21] = [
    0, 0, 0, 0, 0, 0, 0, 0, 8, 8, 8, 8, 16, 16, 16, 21, 21, 24, 29, 34, 36,
];

const CAPS: [u8; 168] = [
    224, 224, 224, 224, 224, 224, 224, 224, 160, 160, 160, 160, 185, 185, 185, 178, 178, 168, 134,
    61, 37, 224, 224, 224, 224, 224, 224, 224, 224, 240, 240, 240, 240, 207, 207, 207, 198, 198,
    183, 144, 66, 40, 160, 160, 160, 160, 160, 160, 160, 160, 185, 185, 185, 185, 193, 193, 193,
    183, 183, 172, 138, 64, 38, 240, 240, 240, 240, 240, 240, 240, 240, 207, 207, 207, 207, 204,
    204, 204, 193, 193, 180, 143, 66, 40, 185, 185, 185, 185, 185, 185, 185, 185, 193, 193, 193,
    193, 193, 193, 193, 183, 183, 172, 138, 65, 39, 207, 207, 207, 207, 207, 207, 207, 207, 204,
    204, 204, 204, 201, 201, 201, 188, 188, 176, 141, 66, 40, 193, 193, 193, 193, 193, 193, 193,
    193, 193, 193, 193, 193, 194, 194, 194, 184, 184, 173, 139, 65, 39, 204, 204, 204, 204, 204,
    204, 204, 204, 201, 201, 201, 201, 198, 198, 198, 187, 187, 175, 140, 66, 40,
];

const PULSE_INDEX: [i16; 105] = [
    -1, -1, -1, -1, -1, -1, -1, -1, 0, 0, 0, 0, 41, 41, 41, 82, 82, 123, 164, 200, 222, 0, 0, 0, 0,
    0, 0, 0, 0, 41, 41, 41, 41, 123, 123, 123, 164, 164, 240, 266, 283, 295, 41, 41, 41, 41, 41,
    41, 41, 41, 123, 123, 123, 123, 240, 240, 240, 266, 266, 305, 318, 328, 336, 123, 123, 123,
    123, 123, 123, 123, 123, 240, 240, 240, 240, 305, 305, 305, 318, 318, 343, 351, 358, 364, 240,
    240, 240, 240, 240, 240, 240, 240, 305, 305, 305, 305, 343, 343, 343, 351, 351, 370, 376, 382,
    387,
];

static PULSE_COSTS: [u8; 392] = [
    40, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 7,
    7, 7, 7, 7, 7, 7, 7, 7, 7, 7, 40, 15, 23, 28, 31, 34, 36, 38, 39, 41, 42, 43, 44, 45, 46, 47,
    47, 49, 50, 51, 52, 53, 54, 55, 55, 57, 58, 59, 60, 61, 62, 63, 63, 65, 66, 67, 68, 69, 70, 71,
    71, 40, 20, 33, 41, 48, 53, 57, 61, 64, 66, 69, 71, 73, 75, 76, 78, 80, 82, 85, 87, 89, 91, 92,
    94, 96, 98, 101, 103, 105, 107, 108, 110, 112, 114, 117, 119, 121, 123, 124, 126, 128, 40, 23,
    39, 51, 60, 67, 73, 79, 83, 87, 91, 94, 97, 100, 102, 105, 107, 111, 115, 118, 121, 124, 126,
    129, 131, 135, 139, 142, 145, 148, 150, 153, 155, 159, 163, 166, 169, 172, 174, 177, 179, 35,
    28, 49, 65, 78, 89, 99, 107, 114, 120, 126, 132, 136, 141, 145, 149, 153, 159, 165, 171, 176,
    180, 185, 189, 192, 199, 205, 211, 216, 220, 225, 229, 232, 239, 245, 251, 21, 33, 58, 79, 97,
    112, 125, 137, 148, 157, 166, 174, 182, 189, 195, 201, 207, 217, 227, 235, 243, 251, 17, 35,
    63, 86, 106, 123, 139, 152, 165, 177, 187, 197, 206, 214, 222, 230, 237, 250, 25, 31, 55, 75,
    91, 105, 117, 128, 138, 146, 154, 161, 168, 174, 180, 185, 190, 200, 208, 215, 222, 229, 235,
    240, 245, 255, 16, 36, 65, 89, 110, 128, 144, 159, 173, 185, 196, 207, 217, 226, 234, 242, 250,
    11, 41, 74, 103, 128, 151, 172, 191, 209, 225, 241, 255, 9, 43, 79, 110, 138, 163, 186, 207,
    227, 246, 12, 39, 71, 99, 123, 144, 164, 182, 198, 214, 228, 241, 253, 9, 44, 81, 113, 142,
    168, 192, 214, 235, 255, 7, 49, 90, 127, 160, 191, 220, 247, 6, 51, 95, 134, 170, 203, 234, 7,
    47, 87, 123, 155, 184, 212, 237, 6, 52, 97, 137, 174, 208, 240, 5, 57, 106, 151, 192, 231, 5,
    59, 111, 158, 202, 243, 5, 55, 103, 147, 187, 224, 5, 60, 113, 161, 206, 248, 4, 65, 122, 175,
    224, 4, 67, 127, 182, 234,
];

/// Validated standard CELT band range and frame geometry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AllocationConfig {
    pub(crate) start: usize,
    pub(crate) end: usize,
    pub(crate) lm: u8,
    pub(crate) channels: usize,
}

impl AllocationConfig {
    pub(crate) fn new(start: usize, end: usize, lm: u8, channels: usize) -> Result<Self, Error> {
        let config = Self {
            start,
            end,
            lm,
            channels,
        };
        config.validate()?;
        Ok(config)
    }

    fn validate(self) -> Result<(), Error> {
        if self.start >= self.end
            || self.end > BANDS
            || self.lm > 3
            || !(1..=2).contains(&self.channels)
        {
            return Err(Error::Argument);
        }
        Ok(())
    }

    fn bins(self, band: usize) -> i32 {
        (width(band) << self.lm) as i32
    }

    fn floor(self) -> i32 {
        self.channels as i32 * 8
    }

    fn mode_curves(self) -> &'static ModeCurves {
        &MODE_CURVES[usize::from(self.lm) * 2 + self.channels - 1]
    }

    fn caps(self) -> &'static [i32; BANDS] {
        &self.mode_curves().caps
    }
}

fn width(band: usize) -> usize {
    BAND_EDGES[band + 1] - BAND_EDGES[band]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AllocationSideInfo {
    pub(crate) boost_eighth_bits: [i32; BANDS],
    pub(crate) trim: u8,
}

impl Default for AllocationSideInfo {
    fn default() -> Self {
        Self {
            boost_eighth_bits: [0; BANDS],
            trim: 5,
        }
    }
}

/// Allocation before the final raw anti-collapse flag and fine-energy pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct AllocationPlan {
    pub(crate) shape_eighth_bits: [i32; BANDS],
    pub(crate) fine_energy_bits: [u8; BANDS],
    pub(crate) fine_priority: [u8; BANDS],
    pub(crate) coded_bands: usize,
    pub(crate) intensity: usize,
    pub(crate) dual_stereo: bool,
    pub(crate) balance_eighth_bits: i32,
}

/// Encoder choices do not change the implicit allocation rules.
#[derive(Clone, Copy, Debug)]
pub(crate) struct AllocationPreferences {
    pub(crate) previous_coded_bands: usize,
    pub(crate) intensity: usize,
    pub(crate) dual_stereo: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FrameBudget {
    pub(crate) remaining_eighth_bits: i32,
    pub(crate) anti_collapse_reserved: bool,
}

const MAX_FRAME_EIGHTHS: i32 = 1_275 * 64;
const TRIM_ICDF: [u8; 11] = [126, 124, 119, 109, 87, 41, 19, 9, 4, 2, 0];

/// The one-eighth-bit safety margin belongs outside the allocation stage.
pub(crate) fn available_budget(
    config: AllocationConfig,
    frame_eighth_bits: i32,
    used_eighth_bits: u64,
    transient: bool,
) -> Result<FrameBudget, Error> {
    config.validate()?;
    validate_frame_budget(frame_eighth_bits)?;
    let remaining =
        (i64::from(frame_eighth_bits) - i64::try_from(used_eighth_bits).unwrap_or(i64::MAX) - 1)
            .max(0) as i32;
    let reserve = transient && config.lm >= 2 && remaining >= (i32::from(config.lm) + 2) * 8;
    Ok(FrameBudget {
        remaining_eighth_bits: remaining - i32::from(reserve) * 8,
        anti_collapse_reserved: reserve,
    })
}

fn validate_frame_budget(bits: i32) -> Result<(), Error> {
    if !(0..=MAX_FRAME_EIGHTHS).contains(&bits) {
        return Err(Error::Argument);
    }
    Ok(())
}

/// Dynamic boost size includes both channels, as specified by Appendix A.
fn boost_quanta(config: AllocationConfig, band: usize) -> i32 {
    let coefficients = config.bins(band) * config.channels as i32;
    (coefficients * 8).min(coefficients.max(48))
}

trait BoostSymbols {
    fn tell_frac(&self) -> u64;
    fn boost(&mut self, band: usize, count: u8, logp: u32) -> Result<bool, Error>;
    fn trim(&mut self) -> Result<u8, Error>;
}

struct DecodeBoosts<'a, 'b>(&'a mut Decoder<'b>);

impl BoostSymbols for DecodeBoosts<'_, '_> {
    fn tell_frac(&self) -> u64 {
        self.0.tell_frac()
    }
    fn boost(&mut self, _: usize, _: u8, logp: u32) -> Result<bool, Error> {
        self.0.bit(logp)
    }
    fn trim(&mut self) -> Result<u8, Error> {
        self.0.icdf(&TRIM_ICDF, 7).map(|trim| trim as u8)
    }
}

struct EncodeBoosts<'a, 'b> {
    coder: &'a mut Encoder<'b>,
    requests: &'a [u8; BANDS],
    trim: u8,
}

impl BoostSymbols for EncodeBoosts<'_, '_> {
    fn tell_frac(&self) -> u64 {
        self.coder.tell_frac()
    }
    fn boost(&mut self, band: usize, count: u8, logp: u32) -> Result<bool, Error> {
        let wanted = count < self.requests[band];
        self.coder.bit(wanted, logp)?;
        Ok(wanted)
    }
    fn trim(&mut self) -> Result<u8, Error> {
        self.coder.icdf(usize::from(self.trim), &TRIM_ICDF, 7)?;
        Ok(self.trim)
    }
}

fn side_info(
    config: AllocationConfig,
    frame_eighth_bits: i32,
    symbols: &mut impl BoostSymbols,
) -> Result<AllocationSideInfo, Error> {
    config.validate()?;
    validate_frame_budget(frame_eighth_bits)?;
    let caps = config.caps();
    let mut info = AllocationSideInfo::default();
    let mut remaining = frame_eighth_bits;
    let mut first_probability = 6;
    for (band, &cap) in caps.iter().enumerate().take(config.end).skip(config.start) {
        let quantum = boost_quanta(config, band);
        let mut probability = first_probability;
        let mut count = 0;
        while symbols.tell_frac() + u64::from(probability) * 8 < remaining.max(0) as u64
            && info.boost_eighth_bits[band] < cap
        {
            if !symbols.boost(band, count, probability)? {
                break;
            }
            info.boost_eighth_bits[band] += quantum;
            remaining -= quantum;
            count = count.saturating_add(1);
            probability = 1;
        }
        if count > 0 {
            first_probability = (first_probability - 1).max(2);
        }
    }
    if symbols.tell_frac() + 48 <= remaining.max(0) as u64 {
        info.trim = symbols.trim()?;
    }
    Ok(info)
}

pub(crate) fn decode_side_info(
    config: AllocationConfig,
    decoder: &mut Decoder<'_>,
    frame_eighth_bits: i32,
) -> Result<AllocationSideInfo, Error> {
    side_info(config, frame_eighth_bits, &mut DecodeBoosts(decoder))
}

/// Requests count quanta rather than bits; infeasible boosts are omitted.
pub(crate) fn encode_side_info(
    config: AllocationConfig,
    encoder: &mut Encoder<'_>,
    frame_eighth_bits: i32,
    requests: &[u8; BANDS],
    trim: u8,
) -> Result<AllocationSideInfo, Error> {
    if trim > 10 {
        return Err(Error::Argument);
    }
    side_info(
        config,
        frame_eighth_bits,
        &mut EncodeBoosts {
            coder: encoder,
            requests,
            trim,
        },
    )
}

// All mode geometry and unadjusted quality costs depend only on LM and
// channel count. Build them once as immutable Rust data, without lazy state
// or allocation in any frame callback.
#[derive(Clone, Copy)]
struct ModeCurves {
    base: [[i32; BANDS]; STATIC_ALLOCATION.len()],
    threshold: [i32; BANDS],
    caps: [i32; BANDS],
}

static MODE_CURVES: [ModeCurves; 8] = mode_curves();

const fn mode_curves() -> [ModeCurves; 8] {
    let mut modes = [ModeCurves {
        base: [[0; BANDS]; STATIC_ALLOCATION.len()],
        threshold: [0; BANDS],
        caps: [0; BANDS],
    }; 8];
    let mut mode = 0;
    while mode < 8 {
        let lm = mode / 2;
        let channels = (mode % 2 + 1) as i32;
        let mut band = 0;
        while band < BANDS {
            let bins = ((BAND_EDGES[band + 1] - BAND_EDGES[band]) << lm) as i32;
            let threshold = bins * 3 / 2;
            modes[mode].threshold[band] = if threshold > channels * 8 {
                threshold
            } else {
                channels * 8
            };
            modes[mode].caps[band] = (CAPS[mode * BANDS + band] as i32 + 64) * channels * bins / 4;
            let mut quality = 0;
            while quality < STATIC_ALLOCATION.len() {
                modes[mode].base[quality][band] =
                    channels * bins * STATIC_ALLOCATION[quality][band] as i32 / 4;
                quality += 1;
            }
            band += 1;
        }
        mode += 1;
    }
    modes
}

/// A pair of adjacent quality curves after trim and boost adjustments.
struct Curves {
    low: [i32; BANDS],
    step: [i32; BANDS],
    threshold: &'static [i32; BANDS],
    caps: &'static [i32; BANDS],
    skip_boundary: usize,
}

impl Curves {
    fn new(config: AllocationConfig, info: &AllocationSideInfo, total: i32) -> Self {
        let mode = config.mode_curves();
        let caps = &mode.caps;
        let threshold = &mode.threshold;
        let trim: [i32; BANDS] = std::array::from_fn(|band| {
            if !(config.start..config.end).contains(&band) {
                return 0;
            }
            // Signed division rounds down: Appendix A specifies a right shift.
            let tilt = (i32::from(info.trim) - 5 - i32::from(config.lm))
                * config.channels as i32
                * config.bins(band)
                * (config.end - band - 1) as i32
                * 8;
            (tilt >> 6)
                - if config.bins(band) == 1 {
                    config.floor()
                } else {
                    0
                }
        });
        let adjusted = |row: &[i32; BANDS], band: usize| {
            let base = row[band];
            if base == 0 {
                0
            } else {
                (base + trim[band]).max(0)
            }
        };
        // Partition the 10 nonzero curves into those fitting and exceeding total.
        let mut first_over = 1;
        let mut limit = STATIC_ALLOCATION.len();
        while first_over < limit {
            let candidate = first_over + (limit - first_over) / 2;
            let row = &mode.base[candidate];
            let values = |band| adjusted(row, band) + info.boost_eighth_bits[band];
            if fits_budget(config, values, threshold, caps, total) {
                first_over = candidate + 1;
            } else {
                limit = candidate;
            }
        }
        let quality = first_over - 1;
        let lower = &mode.base[quality];
        let upper = mode.base.get(first_over).unwrap_or(caps);
        let low = std::array::from_fn(|band| {
            adjusted(lower, band)
                + if quality == 0 {
                    0
                } else {
                    info.boost_eighth_bits[band]
                }
        });
        let step = std::array::from_fn(|band| {
            (adjusted(upper, band) + info.boost_eighth_bits[band] - low[band]).max(0)
        });
        let skip_boundary = (config.start..config.end)
            .rfind(|&band| info.boost_eighth_bits[band] > 0)
            .unwrap_or(config.start);
        Self {
            low,
            step,
            threshold,
            caps,
            skip_boundary,
        }
    }

    fn interpolate(&self, config: AllocationConfig, total: i32) -> [i32; BANDS] {
        // All 64 interpolation points are represented by six binary decisions.
        // The upper endpoint belongs to the next quality interval.
        let mut fraction = 0;
        for step in [32, 16, 8, 4, 2, 1] {
            let candidate = fraction + step;
            // step and candidate are nonnegative. Evaluate only active bands
            // directly in the projection, retaining the exact integer result.
            let values = |band| self.low[band] + ((self.step[band] * candidate) >> 6);
            if fits_budget(config, values, self.threshold, self.caps, total) {
                fraction = candidate;
            }
        }
        let mut values =
            std::array::from_fn(|band| self.low[band] + ((self.step[band] * fraction) >> 6));
        threshold_projection(config, &mut values, self.threshold, self.caps);
        values
    }
}

/// Below the last shape-coded band, all lower bands may use their full budget.
/// Above it, retain at most one fine-energy bit per channel.
#[inline]
fn projected_band(
    value: i32,
    threshold: i32,
    cap: i32,
    floor: i32,
    shape_started: &mut bool,
) -> i32 {
    *shape_started |= value >= threshold;
    if *shape_started {
        value.min(cap)
    } else if value >= floor {
        floor.min(cap)
    } else {
        0
    }
}

fn threshold_projection(
    config: AllocationConfig,
    values: &mut [i32; BANDS],
    threshold: &[i32; BANDS],
    caps: &[i32; BANDS],
) {
    let mut shape_started = false;
    let floor = config.floor();
    for band in (config.start..config.end).rev() {
        values[band] = projected_band(
            values[band],
            threshold[band],
            caps[band],
            floor,
            &mut shape_started,
        );
    }
    values[..config.start].fill(0);
    values[config.end..].fill(0);
}

#[inline]
fn fits_budget(
    config: AllocationConfig,
    values: impl Fn(usize) -> i32,
    threshold: &[i32; BANDS],
    caps: &[i32; BANDS],
    budget: i32,
) -> bool {
    // Quality search needs only the total. Evaluate the same projection in
    // place in the accumulator, avoiding a copied and zeroed 21-band array.
    let mut total = 0;
    let mut shape_started = false;
    let floor = config.floor();
    for band in (config.start..config.end).rev() {
        total += projected_band(
            values(band),
            threshold[band],
            caps[band],
            floor,
            &mut shape_started,
        );
        // Validated boosts and adjusted curves are nonnegative, as is every
        // projected band. Remaining bands cannot make an excess fit again.
        if total > budget {
            return false;
        }
    }
    true
}

struct Reservations {
    bands: i32,
    skip: i32,
    intensity: i32,
    dual: i32,
}

impl Reservations {
    fn new(config: AllocationConfig, mut total: i32) -> Self {
        let skip = if total >= 8 { 8 } else { 0 };
        total -= skip;
        let mut intensity = 0;
        let mut dual = 0;
        if config.channels == 2 {
            let required = LOG2_BOUNDARIES[config.end - config.start];
            if total >= required {
                intensity = required;
                total -= intensity;
                if total >= 8 {
                    dual = 8;
                    total -= dual;
                }
            }
        }
        Self {
            bands: total,
            skip,
            intensity,
            dual,
        }
    }
}

trait AllocationSymbols {
    fn keep_band(&mut self, band: usize, bits: i32, width: usize, lm: u8) -> Result<bool, Error>;
    fn intensity(&mut self, start: usize, end: usize) -> Result<usize, Error>;
    fn dual(&mut self) -> Result<bool, Error>;
}

struct DecodeAllocation<'a, 'b>(&'a mut Decoder<'b>);

impl AllocationSymbols for DecodeAllocation<'_, '_> {
    fn keep_band(&mut self, _: usize, _: i32, _: usize, _: u8) -> Result<bool, Error> {
        self.0.bit(1)
    }
    fn intensity(&mut self, start: usize, end: usize) -> Result<usize, Error> {
        self.0
            .uniform((end + 1 - start) as u32)
            .map(|value| start + value as usize)
    }
    fn dual(&mut self) -> Result<bool, Error> {
        self.0.bit(1)
    }
}

struct EncodeAllocation<'a, 'b> {
    coder: &'a mut Encoder<'b>,
    preferences: AllocationPreferences,
}

impl AllocationSymbols for EncodeAllocation<'_, '_> {
    fn keep_band(&mut self, band: usize, bits: i32, width: usize, lm: u8) -> Result<bool, Error> {
        let hysteresis = if band < self.preferences.previous_coded_bands {
            7
        } else {
            9
        };
        let threshold = (hysteresis * (width << lm) * 8 / 16) as i32;
        let keep = bits > threshold;
        self.coder.bit(keep, 1)?;
        Ok(keep)
    }
    fn intensity(&mut self, start: usize, end: usize) -> Result<usize, Error> {
        let intensity = self.preferences.intensity.min(end);
        self.coder
            .uniform((intensity - start) as u32, (end + 1 - start) as u32)?;
        Ok(intensity)
    }
    fn dual(&mut self) -> Result<bool, Error> {
        self.coder.bit(self.preferences.dual_stereo, 1)?;
        Ok(self.preferences.dual_stereo)
    }
}

fn allocation(
    config: AllocationConfig,
    info: &AllocationSideInfo,
    remaining_eighth_bits: i32,
    symbols: &mut impl AllocationSymbols,
) -> Result<AllocationPlan, Error> {
    config.validate()?;
    validate_frame_budget(remaining_eighth_bits)?;
    if info.trim > 10
        || info
            .boost_eighth_bits
            .iter()
            .any(|&value| !(0..=MAX_FRAME_EIGHTHS).contains(&value))
    {
        return Err(Error::Argument);
    }
    let mut reserves = Reservations::new(config, remaining_eighth_bits);
    let curves = Curves::new(config, info, reserves.bands);
    let mut bits = curves.interpolate(config, reserves.bands);
    let mut assigned: i32 = bits.iter().sum();
    let mut coded = config.end;
    while coded - 1 > curves.skip_boundary {
        let band = coded - 1;
        let coefficients = BAND_EDGES[coded] - BAND_EDGES[config.start];
        let band_width = width(band);
        let leftover = reserves.bands - assigned;
        if leftover < 0 {
            return Err(Error::Corrupt);
        }
        let per_bin = leftover / coefficients as i32;
        let remainder = leftover % coefficients as i32;
        let extra = (remainder - (BAND_EDGES[band] - BAND_EDGES[config.start]) as i32).max(0);
        let mut budget = bits[band] + per_bin * band_width as i32 + extra;
        if budget >= curves.threshold[band].max(config.floor() + 8) {
            if symbols.keep_band(band, budget, band_width, config.lm)? {
                break;
            }
            assigned += 8;
            budget -= 8;
        }
        let new_intensity = if reserves.intensity == 0 {
            0
        } else {
            LOG2_BOUNDARIES[band - config.start]
        };
        let retained = if budget >= config.floor() {
            config.floor()
        } else {
            0
        };
        assigned += retained - bits[band] + new_intensity - reserves.intensity;
        reserves.intensity = new_intensity;
        bits[band] = retained;
        coded -= 1;
    }
    if coded - 1 <= curves.skip_boundary {
        reserves.bands += reserves.skip;
    }
    let intensity = if reserves.intensity > 0 {
        symbols.intensity(config.start, coded)?
    } else {
        0
    };
    if intensity <= config.start {
        reserves.bands += reserves.dual;
        reserves.dual = 0;
    }
    let dual_stereo = reserves.dual > 0 && symbols.dual()?;
    redistribute(config, coded, &mut bits, reserves.bands - assigned)?;
    let mut plan = AllocationPlan {
        shape_eighth_bits: bits,
        fine_energy_bits: [0; BANDS],
        fine_priority: [0; BANDS],
        coded_bands: coded,
        intensity,
        dual_stereo,
        balance_eighth_bits: 0,
    };
    separate_energy(config, curves.caps, &mut plan);
    Ok(plan)
}

fn redistribute(
    config: AllocationConfig,
    coded: usize,
    bits: &mut [i32; BANDS],
    leftover: i32,
) -> Result<(), Error> {
    if leftover < 0 {
        return Err(Error::Corrupt);
    }
    let coefficients = (BAND_EDGES[coded] - BAND_EDGES[config.start]) as i32;
    let quotient = leftover / coefficients;
    let mut remainder = leftover % coefficients;
    for (band, bits) in bits.iter_mut().enumerate().take(coded).skip(config.start) {
        let count = width(band) as i32;
        let extra = remainder.min(count);
        *bits += quotient * count + extra;
        remainder -= extra;
    }
    Ok(())
}

fn separate_energy(config: AllocationConfig, caps: &[i32; BANDS], plan: &mut AllocationPlan) {
    let fine_cost = config.floor();
    let mut carried = 0;
    for (band, &cap) in caps
        .iter()
        .enumerate()
        .take(plan.coded_bands)
        .skip(config.start)
    {
        let budget = plan.shape_eighth_bits[band] + carried;
        let bins = config.bins(band);
        let (shape, mut fine, mut priority, mut excess) = if bins == 1 {
            (budget.min(fine_cost), 0, 1, (budget - fine_cost).max(0))
        } else {
            let usable = budget.min(cap);
            let coupled =
                config.channels == 2 && bins > 2 && !plan.dual_stereo && band < plan.intensity;
            let dimensions = config.channels as i32 * bins + i32::from(coupled);
            let logarithm = dimensions * (BAND_LOG2_EIGHTHS[band] + i32::from(config.lm) * 8);
            let mut bias = logarithm / 2 - dimensions * FINE_OFFSET;
            if bins == 2 {
                bias += dimensions * 2;
            }
            if usable + bias < dimensions * 16 {
                bias += logarithm / 4;
            } else if usable + bias < dimensions * 24 {
                bias += logarithm / 8;
            }
            let fine = ((usable + bias + dimensions * 4).max(0) / (dimensions * 8))
                .min(usable / fine_cost)
                .min(MAX_FINE_BITS);
            let priority = u8::from(fine * dimensions * 8 >= usable + bias);
            (usable - fine * fine_cost, fine, priority, budget - usable)
        };
        if excess > 0 {
            let extra_fine = (excess / fine_cost).min(MAX_FINE_BITS - fine);
            fine += extra_fine;
            let spent = extra_fine * fine_cost;
            priority = u8::from(spent >= excess - carried);
            excess -= spent;
        }
        plan.shape_eighth_bits[band] = shape;
        plan.fine_energy_bits[band] = fine as u8;
        plan.fine_priority[band] = priority;
        carried = excess;
    }
    for band in plan.coded_bands..config.end {
        let fine = plan.shape_eighth_bits[band] / fine_cost;
        plan.shape_eighth_bits[band] = 0;
        plan.fine_energy_bits[band] = fine as u8;
        plan.fine_priority[band] = u8::from(fine < 1);
    }
    plan.balance_eighth_bits = carried;
}

pub(crate) fn decode_allocation(
    config: AllocationConfig,
    info: &AllocationSideInfo,
    remaining_eighth_bits: i32,
    decoder: &mut Decoder<'_>,
) -> Result<AllocationPlan, Error> {
    allocation(
        config,
        info,
        remaining_eighth_bits,
        &mut DecodeAllocation(decoder),
    )
}

pub(crate) fn encode_allocation(
    config: AllocationConfig,
    info: &AllocationSideInfo,
    remaining_eighth_bits: i32,
    encoder: &mut Encoder<'_>,
    preferences: AllocationPreferences,
) -> Result<AllocationPlan, Error> {
    if preferences.intensity < config.start
        || preferences.intensity > config.end
        || preferences.previous_coded_bands > BANDS
    {
        return Err(Error::Argument);
    }
    allocation(
        config,
        info,
        remaining_eighth_bits,
        &mut EncodeAllocation {
            coder: encoder,
            preferences,
        },
    )
}

// Derived from our exact nearest-cost search at compile time. The standard
// has 23 distinct rows. Each reaches its final count by budget 256 (asserted
// below), so higher budgets can saturate there. Repeated-cost ties stay unchanged.
#[derive(Clone, Copy)]
struct PulseRow {
    costs: &'static [u8],
    nearest: [u8; 257],
}

const fn nearest_index(costs: &[u8], budget_eighths: i32) -> u8 {
    let target = budget_eighths - 1;
    let mut lower = 0usize;
    let mut upper = costs[0] as usize;
    while upper - lower > 1 {
        let middle = lower + (upper - lower).div_ceil(2);
        if costs[middle] as i32 >= target {
            upper = middle;
        } else {
            lower = middle;
        }
    }
    let lower_cost = if lower == 0 { -1 } else { costs[lower] as i32 };
    let upper_cost = costs[upper] as i32;
    if target - lower_cost <= upper_cost - target {
        lower as u8
    } else {
        upper as u8
    }
}

const fn pulse_rows() -> ([PulseRow; 23], [u8; 105]) {
    let mut rows = [PulseRow {
        costs: &[],
        nearest: [0; 257],
    }; 23];
    let mut geometry = [u8::MAX; 105];
    let mut offset = 0;
    let mut row = 0;
    while offset < PULSE_COSTS.len() {
        let costs = PULSE_COSTS.split_at(offset).1;
        let costs = costs.split_at(costs[0] as usize + 1).0;
        rows[row].costs = costs;
        let mut budget = 0;
        while budget <= 256 {
            rows[row].nearest[budget] = nearest_index(costs, budget as i32);
            budget += 1;
        }
        assert!(rows[row].nearest[256] == costs[0]);
        let mut i = 0;
        while i < PULSE_INDEX.len() {
            if PULSE_INDEX[i] == offset as i16 {
                geometry[i] = row as u8;
            }
            i += 1;
        }
        offset += costs.len();
        row += 1;
    }
    assert!(row == rows.len());
    (rows, geometry)
}

static PULSE_ROWS: ([PulseRow; 23], [u8; 105]) = pulse_rows();

/// One validated immutable band/size cache, shared by split, selection and
/// budget correction rather than looking up and checking its row repeatedly.
pub(crate) struct PulseCache {
    row: &'static PulseRow,
}

pub(crate) struct PulseSelection {
    pub(crate) index: u8,
    pub(crate) eighth_bits: i32,
}

impl PulseCache {
    #[inline]
    pub(crate) fn new(band: usize, lm: i8) -> Result<Self, Error> {
        if band >= BANDS || !(-1..=3).contains(&lm) {
            return Err(Error::Argument);
        }
        let row = PULSE_ROWS.1[(lm + 1) as usize * BANDS + band];
        Ok(Self {
            row: PULSE_ROWS.0.get(usize::from(row)).ok_or(Error::Argument)?,
        })
    }

    #[inline]
    pub(crate) fn maximum_bits(&self) -> i32 {
        i32::from(*self.row.costs.last().expect("nonempty pulse cache")) + 1
    }

    #[inline]
    pub(crate) fn cost(&self, index: u8) -> Result<i32, Error> {
        if index == 0 {
            return Ok(0);
        }
        self.row
            .costs
            .get(usize::from(index))
            .map(|&cost| i32::from(cost) + 1)
            .ok_or(Error::Argument)
    }

    /// Nearest supported pulse count, with downward ties. Return its cost from
    /// the immutable row, so the band coder does not repeat the nearest search.
    #[inline]
    pub(crate) fn nearest(&self, budget_eighths: i32) -> Result<PulseSelection, Error> {
        if budget_eighths < 0 {
            return Err(Error::Argument);
        }
        let index = self.row.nearest[budget_eighths.min(256) as usize];
        Ok(PulseSelection {
            index,
            eighth_bits: if index == 0 {
                0
            } else {
                i32::from(self.row.costs[usize::from(index)]) + 1
            },
        })
    }
}

pub(crate) fn band_log2_eighths(band: usize) -> Result<i32, Error> {
    BAND_LOG2_EIGHTHS.get(band).copied().ok_or(Error::Argument)
}

#[cfg(test)]
pub(crate) fn max_pulse_index(band: usize, lm: i8) -> Result<u8, Error> {
    PulseCache::new(band, lm).map(|cache| cache.row.costs[0])
}

pub(crate) fn pulses_from_index(index: u8) -> Result<u32, Error> {
    if index > 40 {
        return Err(Error::Argument);
    }
    Ok(if index < 8 {
        u32::from(index)
    } else {
        u32::from(8 + (index & 7)) << ((index >> 3) - 1)
    })
}

#[cfg(test)]
pub(crate) fn pulse_bits(band: usize, lm: i8, index: u8) -> Result<i32, Error> {
    PulseCache::new(band, lm)?.cost(index)
}

/// Nearest supported PVQ pulse count, with downward ties. Budget enforcement
/// against the frame remainder happens in the band coder after this selection.
#[cfg(test)]
pub(crate) fn pulse_index(band: usize, lm: i8, budget_eighths: i32) -> Result<u8, Error> {
    PulseCache::new(band, lm)?
        .nearest(budget_eighths)
        .map(|selected| selected.index)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_cache_preserves_all_choices_and_repeated_cost_ties() {
        for lm in -1..=3 {
            for band in 0..BANDS {
                let offset = PULSE_INDEX[(lm + 1) as usize * BANDS + band];
                if offset < 0 {
                    assert!(PulseCache::new(band, lm).is_err());
                    continue;
                }
                let costs = &PULSE_COSTS[offset as usize..];
                let costs = &costs[..=usize::from(costs[0])];
                let cache = PulseCache::new(band, lm).unwrap();
                for budget in (0..=257).chain([MAX_FRAME_EIGHTHS, i32::MAX]) {
                    // Independent exhaustive nearest-distance oracle. A tie
                    // between distinct costs takes the lower one; repeated
                    // costs take their last index only when below the budget.
                    let mut expected_index = 0;
                    let mut expected_cost = 0;
                    let mut distance = budget;
                    for (index, &stored) in costs.iter().enumerate().skip(1) {
                        let cost = i32::from(stored) + 1;
                        let candidate = (budget - cost).abs();
                        if candidate < distance
                            || (candidate == distance && cost == expected_cost && cost < budget)
                        {
                            expected_index = index as u8;
                            expected_cost = cost;
                            distance = candidate;
                        }
                    }
                    let selected = cache.nearest(budget).unwrap();
                    assert_eq!(
                        selected.index, expected_index,
                        "band {band}, lm {lm}, budget {budget}"
                    );
                    assert_eq!(selected.eighth_bits, expected_cost);
                }
            }
        }
    }

    fn preferences(config: AllocationConfig) -> AllocationPreferences {
        AllocationPreferences {
            previous_coded_bands: config.end,
            intensity: config.end,
            dual_stereo: false,
        }
    }

    fn band_cost(config: AllocationConfig, plan: &AllocationPlan) -> i32 {
        plan.shape_eighth_bits.iter().sum::<i32>()
            + plan
                .fine_energy_bits
                .iter()
                .map(|&bits| i32::from(bits) * config.floor())
                .sum::<i32>()
            + plan.balance_eighth_bits
    }

    #[test]
    fn standard_geometry_and_single_bin_caps() {
        let widths: [usize; BANDS] = std::array::from_fn(width);
        assert_eq!(
            widths,
            [
                1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 4, 4, 4, 6, 6, 8, 12, 18, 22
            ]
        );
        for channels in [1, 2] {
            let config = AllocationConfig::new(0, BANDS, 0, channels).unwrap();
            // A sign bit plus eight energy bits per channel.
            assert_eq!(&config.caps()[..8], &[channels as i32 * 72; 8]);
        }
    }

    #[test]
    fn pulse_costs_are_monotone_and_nearest_ties_choose_down() {
        for lm in -1..=3 {
            for band in 0..BANDS {
                let Ok(maximum) = max_pulse_index(band, lm) else {
                    continue;
                };
                let mut previous_index = 0;
                let mut previous_cost = 0;
                for budget in 0..=270 {
                    let index = pulse_index(band, lm, budget).unwrap();
                    let cost = pulse_bits(band, lm, index).unwrap();
                    assert!(index >= previous_index);
                    assert!(cost >= previous_cost);
                    let distance = (cost - budget).abs();
                    for candidate in 0..=maximum {
                        let candidate_cost = pulse_bits(band, lm, candidate).unwrap();
                        assert!(distance <= (candidate_cost - budget).abs());
                        if distance == (candidate_cost - budget).abs() {
                            assert!(cost <= candidate_cost);
                        }
                    }
                    previous_index = index;
                    previous_cost = cost;
                }
            }
        }
        assert_eq!(pulses_from_index(0), Ok(0));
        assert_eq!(pulses_from_index(7), Ok(7));
        assert_eq!(pulses_from_index(8), Ok(8));
        assert_eq!(pulses_from_index(16), Ok(16));
        assert_eq!(pulses_from_index(40), Ok(128));
    }

    #[test]
    fn interpolated_totals_are_monotone_in_budget() {
        for lm in 0..=3 {
            for channels in [1, 2] {
                let config = AllocationConfig::new(0, BANDS, lm, channels).unwrap();
                for trim in 0..=10 {
                    let info = AllocationSideInfo {
                        trim,
                        ..Default::default()
                    };
                    let mut previous = 0;
                    for total in 0..=4_096 {
                        let curves = Curves::new(config, &info, total);
                        let sum: i32 = curves.interpolate(config, total).iter().sum();
                        assert!(sum <= total, "{config:?}, trim={trim}, total={total}");
                        assert!(sum >= previous, "{config:?}, trim={trim}, total={total}");
                        previous = sum;
                    }
                }
            }
        }
    }

    #[test]
    fn plans_conserve_band_budget_and_bound_fine_energy() {
        let totals = [
            0,
            1,
            7,
            8,
            9,
            15,
            16,
            23,
            24,
            31,
            32,
            47,
            48,
            63,
            64,
            127,
            128,
            255,
            256,
            511,
            512,
            1_024,
            2_048,
            4_096,
            8_192,
            16_384,
            32_768,
            MAX_FRAME_EIGHTHS,
        ];
        for (start, end) in [(0, 13), (0, 17), (0, 19), (0, 21), (17, 21)] {
            for lm in 0..=3 {
                for channels in [1, 2] {
                    let config = AllocationConfig::new(start, end, lm, channels).unwrap();
                    for trim in 0..=10 {
                        let info = AllocationSideInfo {
                            trim,
                            ..Default::default()
                        };
                        for total in totals {
                            let mut output = [0; 1_275];
                            let mut encoder = Encoder::new(&mut output);
                            let plan = encode_allocation(
                                config,
                                &info,
                                total,
                                &mut encoder,
                                preferences(config),
                            )
                            .unwrap();
                            assert!(plan.coded_bands > start && plan.coded_bands <= end);
                            assert!(plan.shape_eighth_bits.iter().all(|&bits| bits >= 0));
                            assert!(plan.fine_energy_bits.iter().all(|&bits| bits <= 8));
                            assert!(plan.fine_priority.iter().all(|&priority| priority <= 1));
                            assert!(plan.balance_eighth_bits >= 0);
                            assert!(
                                band_cost(config, &plan) <= total,
                                "{config:?}, trim={trim}, total={total}, {plan:?}"
                            );
                            assert!(
                                plan.shape_eighth_bits[..start]
                                    .iter()
                                    .all(|&bits| bits == 0)
                            );
                            assert!(plan.shape_eighth_bits[end..].iter().all(|&bits| bits == 0));
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn boost_trim_and_allocation_signals_roundtrip() {
        for lm in 0..=3 {
            for channels in [1, 2] {
                for frame_bytes in [1, 2, 8, 20, 80, 160, 320] {
                    let config = AllocationConfig::new(0, BANDS, lm, channels).unwrap();
                    let frame_bits = frame_bytes as i32 * 64;
                    let mut output = [0; 320];
                    let mut encoder = Encoder::new(&mut output[..frame_bytes]);
                    let mut requests = [0; BANDS];
                    requests[6] = 2;
                    requests[12] = 1;
                    let sent =
                        encode_side_info(config, &mut encoder, frame_bits, &requests, 3).unwrap();
                    let budget =
                        available_budget(config, frame_bits, encoder.tell_frac(), false).unwrap();
                    let planned = encode_allocation(
                        config,
                        &sent,
                        budget.remaining_eighth_bits,
                        &mut encoder,
                        preferences(config),
                    )
                    .unwrap();
                    let tell = encoder.tell_frac();
                    encoder.finish_padded().unwrap();
                    let mut decoder = Decoder::new(&output[..frame_bytes]);
                    let got = decode_side_info(config, &mut decoder, frame_bits).unwrap();
                    assert_eq!(got, sent);
                    let recovered_budget =
                        available_budget(config, frame_bits, decoder.tell_frac(), false).unwrap();
                    assert_eq!(recovered_budget, budget);
                    let recovered = decode_allocation(
                        config,
                        &got,
                        recovered_budget.remaining_eighth_bits,
                        &mut decoder,
                    )
                    .unwrap();
                    assert_eq!(recovered, planned);
                    assert_eq!(decoder.tell_frac(), tell);
                }
            }
        }
    }

    #[test]
    fn anti_collapse_requires_short_blocks_and_room() {
        for lm in 0..=3 {
            let config = AllocationConfig::new(0, BANDS, lm, 1).unwrap();
            let threshold = (i32::from(lm) + 2) * 8;
            assert!(
                !available_budget(config, threshold, 0, true)
                    .unwrap()
                    .anti_collapse_reserved
            );
            assert_eq!(
                available_budget(config, threshold + 1, 0, true)
                    .unwrap()
                    .anti_collapse_reserved,
                lm >= 2
            );
            assert!(
                !available_budget(config, 1_024, 0, false)
                    .unwrap()
                    .anti_collapse_reserved
            );
            assert_eq!(
                available_budget(config, 0, u64::MAX, true)
                    .unwrap()
                    .remaining_eighth_bits,
                0
            );
        }
    }

    #[test]
    fn invalid_inputs_are_rejected_before_entropy_use() {
        assert_eq!(AllocationConfig::new(0, 22, 0, 1), Err(Error::Argument));
        assert_eq!(AllocationConfig::new(1, 1, 0, 1), Err(Error::Argument));
        assert_eq!(AllocationConfig::new(0, 21, 4, 1), Err(Error::Argument));
        assert_eq!(AllocationConfig::new(0, 21, 0, 3), Err(Error::Argument));
        let config = AllocationConfig::new(0, BANDS, 0, 1).unwrap();
        let mut output = [0; 128];
        let mut encoder = Encoder::new(&mut output);
        let before = encoder.tell_frac();
        assert_eq!(
            encode_side_info(config, &mut encoder, 128, &[0; BANDS], 11),
            Err(Error::Argument)
        );
        let mut bad = AllocationSideInfo::default();
        bad.boost_eighth_bits[0] = -1;
        assert_eq!(
            encode_allocation(config, &bad, 128, &mut encoder, preferences(config)),
            Err(Error::Argument)
        );
        assert_eq!(encoder.tell_frac(), before);
        assert_eq!(pulse_index(0, -1, 10), Err(Error::Argument));
        assert_eq!(pulse_index(BANDS, 0, 10), Err(Error::Argument));
        assert_eq!(pulse_index(0, 4, 10), Err(Error::Argument));
        assert_eq!(pulse_index(0, 0, -1), Err(Error::Argument));
        assert_eq!(pulses_from_index(41), Err(Error::Argument));
    }
}
