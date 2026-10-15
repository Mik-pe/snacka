// SPDX-License-Identifier: MIT
//! An independently analyzed, owned SILK encoder layer.

use super::analysis::{Spectrum, broaden, spectrum as analyze_spectrum};
use super::frame::{encode_gain_symbol, encode_zero_stereo};
use super::lpc::encode_nlsf_indices;
use super::pitch::{Pitch, analyze as analyze_pitch};
use super::quantize::{History, Parameters as Quantization, quantize, quantize_sparse};
use super::redundancy::{RedundancyCache, RedundancyPlan, encode_header};
use super::{
    Bandwidth, Frame, FrameDuration, FrameType, NlsfIndices, PacketDuration, QuantizationOffset,
    SignalType, encode_pulses, estimate_pulse_bits,
};
use crate::entropy::{Encoder as RangeEncoder, Error, Model};

const MAX_SAMPLES: usize = 320;
const MAX_CODED_FRAMES: usize = 6;
// A scratch admission cap for this new refusal path. Owned Opus payloads are
// capped at 1275 bytes; this is not a universal standalone SILK format limit.
const MAX_PREFLIGHT_BYTES: usize = 8192;
const SPARSE_MODELS: usize = 8;
// Nine sparse choices and three explicit zero-pulse choices per model. Requested
// gains still pass through the ordinary absolute/conditional wire gain rules.
const SPARSE_POINTS: [(i32, Option<f64>); 12] = [
    (57, Some(0.5)),
    (57, Some(1.25)),
    (57, Some(4.0)),
    (60, Some(0.5)),
    (60, Some(1.25)),
    (60, Some(4.0)),
    (63, Some(0.5)),
    (63, Some(1.25)),
    (63, Some(4.0)),
    (0, None),
    (60, None),
    (63, None),
];
const SEED: Model<'static> = Model::from_static(&[0, 64, 128, 192, 256]);
const INACTIVE: Model<'static> = Model::from_static(&[0, 26, 256]);
const ACTIVE: Model<'static> = Model::from_static(&[0, 24, 98, 246, 256]);

#[derive(Clone, Copy)]
pub(super) struct CodedFrame {
    pub(super) kind: FrameType,
    pub(super) spectrum: Spectrum,
    pub(super) gains: [u8; 4],
    pub(super) gain_index: i32,
    pub(super) pulses: [i16; MAX_SAMPLES],
    pub(super) conditional: bool,
    pub(super) previous_voiced: bool,
    pub(super) independent_scale: bool,
    pub(super) pitch: Pitch,
    pub(super) coded: bool,
}

impl CodedFrame {
    pub(super) const EMPTY: Self = Self {
        kind: FrameType {
            signal: SignalType::Inactive,
            offset: QuantizationOffset::Low,
        },
        spectrum: Spectrum {
            indices: NlsfIndices {
                first: 0,
                residual: [0; 16],
                interpolation: 4,
            },
            lpc_q12: [0; 16],
            order: 10,
        },
        gains: [0; 4],
        gain_index: 0,
        pulses: [0; MAX_SAMPLES],
        conditional: false,
        previous_voiced: false,
        independent_scale: true,
        pitch: Pitch::NONE,
        coded: false,
    };
}

/// Statistics from a completely encoded SILK layer.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EncodingStatistics {
    /// Number of meaningful native-rate samples per channel.
    pub samples: usize,
    /// Entropy bits consumed by this layer, rounded up from eighth bits.
    pub bits: u64,
    /// Mean squared quantization error in the coded mid/side representation.
    pub quantization_error: f64,
    /// Whether this layer actually includes previous-packet redundancy.
    pub redundancy: bool,
}

/// Native-rate mono/stereo SILK encoder with owned analysis and quantization.
///
/// Analysis, codebook search and causal scalar quantization use bounded scratch
/// and no heap allocation. It selects voiced, unvoiced or inactive frames,
/// transmitting spectral prediction, pitch filters and quantized excitation.
pub struct SilkEncoder {
    history: [History; 2],
    bandwidth: Option<Bandwidth>,
    channels: u8,
    fec: bool,
    redundancy: RedundancyCache,
}

impl Default for SilkEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl SilkEncoder {
    /// Construct an encoder with empty predictor and gain history.
    pub const fn new() -> Self {
        Self {
            history: [History::new(); 2],
            bandwidth: None,
            channels: 0,
            fec: false,
            redundancy: RedundancyCache::new(),
        }
    }

    /// Reset stream prediction and gain history.
    pub fn reset(&mut self) {
        let fec = self.fec;
        *self = Self::new();
        self.fec = fec;
    }

    /// Enable causal previous-packet LBRR when the packet budget permits it.
    pub fn set_fec(&mut self, enabled: bool) {
        if self.fec != enabled {
            self.redundancy.reset();
        }
        self.fec = enabled;
    }

    /// Encode one SILK layer into an existing Opus entropy frame.
    ///
    /// Input is normalized native-rate mono or interleaved left/right PCM.
    /// Its length must exactly match the internal frame grouping and channel
    /// count. `target_bits` bounds the estimated complete layer cost; if even
    /// the coarsest quantizer cannot fit, failure occurs before emitting bits.
    /// The caller owns range-coder termination, Opus framing and resampling.
    /// Encoder history commits only after the layer has been emitted fully.
    /// An output error can partially advance the caller's range encoder.
    pub fn encode_layer(
        &mut self,
        encoder: &mut RangeEncoder<'_>,
        frame: Frame,
        duration: PacketDuration,
        channels: u8,
        pcm: &[f32],
        target_bits: u32,
    ) -> Result<EncodingStatistics, Error> {
        if !(1..=2).contains(&channels)
            || (frame.duration == FrameDuration::Ms10) != (duration == PacketDuration::Ms10)
            || pcm.len() != frame.samples() * duration.frames() * usize::from(channels)
            || pcm
                .iter()
                .any(|sample| !sample.is_finite() || !(-1.0..=1.0).contains(sample))
            || target_bits == 0
        {
            return Err(Error::Argument);
        }
        let intervals = duration.frames();
        let samples = frame.samples();
        let channel_count = usize::from(channels);
        let mut history = if self.bandwidth == Some(frame.bandwidth) {
            self.history
        } else {
            [History::new(); 2]
        };
        if self.channels != channels {
            history[1] = History::new();
        }
        let mut activity = [[false; 3]; 2];
        let mut side_coded = [false; 3];
        let mut coded_frames = intervals;
        let mut mono = [0.0; MAX_SAMPLES];
        for interval in 0..intervals {
            for (channel, active) in activity.iter_mut().enumerate().take(channel_count) {
                extract(pcm, channels, samples, interval, channel, &mut mono);
                let energy = mono[..samples]
                    .iter()
                    .map(|&value| f64::from(value).powi(2))
                    .sum::<f64>();
                active[interval] = energy > samples as f64 * 1e-10;
                if channel == 1 {
                    side_coded[interval] = active[interval];
                    coded_frames += usize::from(side_coded[interval]);
                }
            }
        }
        let header_cost = channel_count * (intervals + 1);
        let stereo_cost = if channels == 2 { intervals * 12 } else { 0 };
        let cost = target_bits as usize;
        if cost <= header_cost + stereo_cost {
            return Err(Error::Output);
        }
        let mut redundancy = self
            .redundancy
            .for_packet(frame, duration, channels, self.fec)?;
        let redundancy_cost = if let Some(plan) = &redundancy {
            plan.estimate_bits()? + 8.0
        } else {
            0.0
        };
        // Low-rate packets may not have room for a prior packet's LBRR. The
        // regular stream takes priority and the statistics report omission.
        let redundancy_cost = if redundancy_cost < target_bits as f32 * 0.55 {
            redundancy_cost
        } else {
            redundancy = None;
            0.0
        };
        let per_frame = (((cost - header_cost - stereo_cost) as f32 - redundancy_cost)
            / coded_frames as f32
            - 2.0)
            .max(0.0);
        let mut stages = [CodedFrame::EMPTY; MAX_CODED_FRAMES];
        let mut redundant_stages = [CodedFrame::EMPTY; MAX_CODED_FRAMES];
        let mut redundant_history = history;
        let redundant_budget =
            (target_bits as f32 * 0.45 - stereo_cost as f32 - 12.0).max(0.0) / coded_frames as f32;
        let mut distortion = 0.0;
        let mut previous_coded = [false; 2];
        let mut used_sparse = false;
        for interval in 0..intervals {
            for channel in 0..channel_count {
                if channel == 1 && !side_coded[interval] {
                    history[1] = History::new();
                    redundant_history[1] = History::new();
                    previous_coded[1] = false;
                    continue;
                }
                extract(pcm, channels, samples, interval, channel, &mut mono);
                let mut kind = FrameType {
                    signal: if activity[channel][interval] {
                        SignalType::Unvoiced
                    } else {
                        SignalType::Inactive
                    },
                    offset: QuantizationOffset::Low,
                };
                let spectrum = analyze_spectrum(&mono[..samples], frame.bandwidth);
                let pitch = if kind.signal == SignalType::Inactive {
                    Pitch::NONE
                } else {
                    analyze_pitch(
                        &mono[..samples],
                        frame,
                        &spectrum.lpc_q12[..spectrum.order],
                        &history[channel],
                    )
                };
                if pitch.lag != 0 {
                    kind.signal = SignalType::Voiced;
                }
                let conditional = interval > 0 && previous_coded[channel];
                let mut parameters = StageParameters {
                    frame,
                    kind,
                    spectrum,
                    pitch,
                    previous: history[channel],
                    conditional,
                    independent_scale: interval == 0,
                };
                let mut best = choose_stage(&mono[..samples], parameters, per_frame);
                if best.is_none() {
                    // A highly peaked short-term model can have a pulse-rate
                    // floor even at the largest gain. Retry a broad spectrum
                    // before declaring that the packet budget cannot fit.
                    parameters.pitch = Pitch::NONE;
                    parameters.kind.signal = if kind.signal == SignalType::Inactive {
                        SignalType::Inactive
                    } else {
                        SignalType::Unvoiced
                    };
                    for chirp in [0.95, 0.9, 0.8] {
                        parameters.spectrum = broaden(spectrum, frame.bandwidth, chirp);
                        best = choose_stage(&mono[..samples], parameters, per_frame);
                        if best.is_some() {
                            break;
                        }
                    }
                    if best.is_none() {
                        parameters.spectrum =
                            analyze_spectrum(&[0.0; MAX_SAMPLES][..samples], frame.bandwidth);
                        best = choose_stage(&mono[..samples], parameters, per_frame);
                    }
                }
                if let Some((_, _, error)) = &best {
                    let energy = mono[..samples]
                        .iter()
                        .map(|&value| f64::from(value).powi(2))
                        .sum::<f64>();
                    // A predictor can fit the packet while leaving too few
                    // bits for its excitation. Compare a broad, cheaper model
                    // before accepting a frame with more than 25% error power.
                    if *error > energy * 0.25 {
                        let alternative = StageParameters {
                            spectrum: broaden(spectrum, frame.bandwidth, 0.6),
                            pitch: Pitch::NONE,
                            kind: FrameType {
                                signal: if kind.signal == SignalType::Inactive {
                                    SignalType::Inactive
                                } else {
                                    SignalType::Unvoiced
                                },
                                ..kind
                            },
                            ..parameters
                        };
                        if let Some(candidate) =
                            choose_stage(&mono[..samples], alternative, per_frame)
                            && candidate.2 < *error
                        {
                            best = Some(candidate);
                            parameters = alternative;
                        }
                        if kind.signal != SignalType::Inactive && per_frame <= samples as f32 * 1.5
                        {
                            // A broad envelope can still benefit from pitch.
                            // Permit weaker coarse correlation here; the full
                            // voiced stage must fit and improve reconstruction.
                            let pitch = super::pitch::analyze_with_threshold(
                                &mono[..samples],
                                frame,
                                &alternative.spectrum.lpc_q12[..alternative.spectrum.order],
                                &history[channel],
                                0.15,
                            );
                            if pitch.lag != 0 {
                                let voiced = StageParameters {
                                    pitch,
                                    kind: FrameType {
                                        signal: SignalType::Voiced,
                                        ..kind
                                    },
                                    ..alternative
                                };
                                if let Some(candidate) =
                                    choose_stage(&mono[..samples], voiced, per_frame)
                                    && candidate.2 < best.as_ref().unwrap().2
                                {
                                    best = Some(candidate);
                                    parameters = voiced;
                                }
                            }
                        }
                    }
                }
                if best.is_none() {
                    // Ordinary model/gain decisions above remain the complete
                    // fast path. Only its final refusal enables sparse cells;
                    // reconstruct the original model/history here, rather than
                    // copying another predictor history on successful frames.
                    best = choose_sparse_fallback(
                        &mono[..samples],
                        StageParameters {
                            frame,
                            kind,
                            spectrum,
                            pitch,
                            previous: history[channel],
                            conditional,
                            independent_scale: interval == 0,
                        },
                        per_frame,
                    );
                    if let Some((stage, _, _)) = &best {
                        // The optional shadow gets the chosen envelope with its
                        // own pretrial redundancy history, never the primary's
                        // newly reconstructed sparse history.
                        parameters.kind = stage.kind;
                        parameters.spectrum = stage.spectrum;
                        parameters.pitch = stage.pitch;
                        used_sparse = true;
                    }
                }
                let Some((stage, committed, error)) = best else {
                    return Err(Error::Output);
                };
                if self.fec && kind.signal != SignalType::Inactive {
                    let conditional =
                        interval > 0 && redundant_stages[(interval - 1) * 2 + channel].coded;
                    if let Some((coarse, committed, _)) = choose_stage(
                        &mono[..samples],
                        // Redundancy must tolerate an imperfect loss-recovered
                        // prefix. Its short-term model carries speech without
                        // propagating prior reconstruction errors through LTP.
                        StageParameters {
                            kind: FrameType {
                                signal: SignalType::Unvoiced,
                                ..kind
                            },
                            pitch: Pitch::NONE,
                            previous: redundant_history[channel],
                            conditional,
                            independent_scale: !conditional,
                            ..parameters
                        },
                        redundant_budget,
                    ) {
                        redundant_stages[interval * 2 + channel] = coarse;
                        redundant_history[channel] = committed;
                    }
                }
                history[channel] = committed;
                distortion += error;
                stages[interval * 2 + channel] = stage;
                previous_coded[channel] = true;
            }
        }
        if used_sparse
            && !sparse_layer_fits(
                LayerPlan {
                    frame,
                    duration,
                    channels,
                    activity: &activity,
                    side_coded: &side_coded,
                    stages: &stages,
                    redundancy: redundancy.as_ref(),
                },
                target_bits,
            )
        {
            return Err(Error::Output);
        }
        let before = encoder.tell_eighths();
        encode_header(encoder, duration, channels, &activity, redundancy.as_ref())?;
        if let Some(plan) = &redundancy {
            plan.encode_frames(encoder)?;
        }
        for interval in 0..intervals {
            if channels == 2 {
                encode_zero_stereo(
                    encoder,
                    if activity[1][interval] {
                        None
                    } else {
                        Some(!side_coded[interval])
                    },
                )?;
            }
            for channel in 0..channel_count {
                let stage = &stages[interval * 2 + channel];
                if stage.coded {
                    encode_frame(encoder, frame, stage)?;
                }
            }
        }
        let consumed = encoder.tell_eighths() - before;
        self.history = history;
        self.bandwidth = Some(frame.bandwidth);
        self.channels = channels;
        if self.fec {
            self.redundancy
                .remember(frame, duration, channels, &redundant_stages, activity)?;
        }
        Ok(EncodingStatistics {
            samples: samples * intervals,
            bits: consumed.div_ceil(8),
            quantization_error: distortion / (samples * coded_frames) as f64,
            redundancy: redundancy.is_some(),
        })
    }
}

#[derive(Clone, Copy)]
struct StageParameters {
    frame: Frame,
    kind: FrameType,
    spectrum: Spectrum,
    pitch: Pitch,
    previous: History,
    conditional: bool,
    independent_scale: bool,
}

fn choose_stage(
    samples: &[f32],
    parameters: StageParameters,
    budget: f32,
) -> Option<(CodedFrame, History, f64)> {
    let mut best: Option<(CodedFrame, History, f64)> = None;
    let mut lower = 0;
    let mut upper = 63;
    while lower <= upper {
        let gain = (lower + upper) / 2;
        let candidate = quantize(
            samples,
            Quantization {
                frame: parameters.frame,
                kind: parameters.kind,
                coefficients: &parameters.spectrum.lpc_q12[..parameters.spectrum.order],
                previous: parameters.previous,
                requested_gain: gain,
                conditional: parameters.conditional,
                seed: 0,
                pitch: parameters.pitch,
                // At generous budgets, preserve ordinary fine quantization.
                low_rate: budget <= samples.len() as f32 * 1.5,
            },
        );
        let stage = CodedFrame {
            kind: parameters.kind,
            spectrum: parameters.spectrum,
            gains: candidate.gain_symbols,
            gain_index: candidate.gain_index,
            pulses: candidate.pulses,
            conditional: parameters.conditional,
            previous_voiced: parameters.previous.voiced,
            independent_scale: parameters.independent_scale,
            pitch: parameters.pitch,
            coded: true,
        };
        if estimated_cost(parameters.frame, &stage).unwrap_or(f32::INFINITY) <= budget {
            // A smaller gain is not necessarily less distorted once causal
            // reconstruction changes future predictor history. Keep the best
            // fitting reconstruction among the candidates already evaluated.
            if best
                .as_ref()
                .is_none_or(|(_, _, error)| candidate.distortion <= *error)
            {
                best = Some((stage, candidate.history, candidate.distortion));
            }
            upper = gain - 1;
        } else {
            lower = gain + 1;
        }
    }
    best
}

fn sparse_model(
    samples: &[f32],
    original: StageParameters,
    index: usize,
) -> Option<StageParameters> {
    let unvoiced = StageParameters {
        kind: FrameType {
            signal: if original.kind.signal == SignalType::Inactive {
                SignalType::Inactive
            } else {
                SignalType::Unvoiced
            },
            ..original.kind
        },
        pitch: Pitch::NONE,
        ..original
    };
    match index {
        0 => Some(original),
        1..=4 => Some(StageParameters {
            spectrum: broaden(
                original.spectrum,
                original.frame.bandwidth,
                [0.95, 0.9, 0.8, 0.6][index - 1],
            ),
            ..unvoiced
        }),
        5 => Some(StageParameters {
            spectrum: analyze_spectrum(
                &[0.0; MAX_SAMPLES][..samples.len()],
                original.frame.bandwidth,
            ),
            ..unvoiced
        }),
        6..=7 if original.kind.signal == SignalType::Voiced && original.pitch.lag != 0 => {
            Some(StageParameters {
                spectrum: broaden(
                    original.spectrum,
                    original.frame.bandwidth,
                    [0.8, 0.6][index - 6],
                ),
                ..original
            })
        }
        _ => None,
    }
}

fn finite_history(history: &History) -> bool {
    history.shaping_error.is_finite()
        && history
            .lpc
            .iter()
            .chain(&history.output)
            .all(|value| value.is_finite())
}

/// At most eight models times twelve causal quantizations: 96 candidates.
/// Every trial starts from the original pretrial history and native PCM. Model
/// construction is lazy; scratch is independent of the fixed candidate count.
/// Ordinary successful frames never enter this separately compiled cold path.
#[cold]
#[inline(never)]
fn choose_sparse_fallback(
    samples: &[f32],
    original: StageParameters,
    budget: f32,
) -> Option<(CodedFrame, History, f64)> {
    if !budget.is_finite() || budget <= 0.0 {
        return None;
    }
    let mut best = None;
    let mut best_ranking_error = f64::INFINITY;
    for model in 0..SPARSE_MODELS {
        let Some(parameters) = sparse_model(samples, original, model) else {
            continue;
        };
        for (requested_gain, penalty) in SPARSE_POINTS {
            let candidate = quantize_sparse(
                samples,
                Quantization {
                    frame: parameters.frame,
                    kind: parameters.kind,
                    coefficients: &parameters.spectrum.lpc_q12[..parameters.spectrum.order],
                    previous: parameters.previous,
                    requested_gain,
                    conditional: parameters.conditional,
                    seed: 0,
                    pitch: parameters.pitch,
                    low_rate: budget <= samples.len() as f32 * 1.5,
                },
                penalty,
            );
            let Some(ranking_error) = candidate.ranking_error else {
                continue;
            };
            if !ranking_error.is_finite()
                || !candidate.distortion.is_finite()
                || !finite_history(&candidate.history)
            {
                continue;
            }
            let stage = CodedFrame {
                kind: parameters.kind,
                spectrum: parameters.spectrum,
                gains: candidate.gain_symbols,
                gain_index: candidate.gain_index,
                pulses: candidate.pulses,
                conditional: parameters.conditional,
                previous_voiced: parameters.previous.voiced,
                independent_scale: parameters.independent_scale,
                pitch: parameters.pitch,
                coded: true,
            };
            if ranking_error < best_ranking_error
                && estimated_cost(parameters.frame, &stage).unwrap_or(f32::INFINITY) <= budget
                && sparse_frame_fits(parameters.frame, &stage, budget)
            {
                best_ranking_error = ranking_error;
                // Keep the existing public unclamped distortion statistic;
                // only fallback ranking uses the clamped reconstruction proxy.
                best = Some((stage, candidate.history, candidate.distortion));
            }
        }
    }
    best
}

#[cold]
#[inline(never)]
fn sparse_frame_fits(frame: Frame, stage: &CodedFrame, budget: f32) -> bool {
    let capacity = (f64::from(budget) / 8.0).ceil() as usize;
    if capacity == 0 || capacity > MAX_PREFLIGHT_BYTES {
        return false;
    }
    let mut scratch = [0; MAX_PREFLIGHT_BYTES];
    let mut encoder = RangeEncoder::new(&mut scratch[..capacity]);
    if encode_frame(&mut encoder, frame, stage).is_err()
        || encoder.tell_eighths() as f64 > f64::from(budget) * 8.0
    {
        return false;
    }
    encoder.finish().is_ok()
}

struct LayerPlan<'a> {
    frame: Frame,
    duration: PacketDuration,
    channels: u8,
    activity: &'a [[bool; 3]; 2],
    side_coded: &'a [bool; 3],
    stages: &'a [CodedFrame; MAX_CODED_FRAMES],
    redundancy: Option<&'a RedundancyPlan>,
}

/// Validate the complete chosen fresh layer, including prior LBRR and arithmetic
/// termination, before any real emission or persistent predictor commit.
///
/// The ordinary Opus caller starts a fresh entropy frame. A direct layer caller
/// may append to a nonempty coder: that interval/capacity cannot be cloned here,
/// so this scratch check does not promise append-state transactionality. A real
/// late Output still propagates once, retaining the existing reset requirement.
#[cold]
#[inline(never)]
fn sparse_layer_fits(plan: LayerPlan<'_>, target_bits: u32) -> bool {
    let capacity = u64::from(target_bits).div_ceil(8);
    if capacity == 0 || capacity > MAX_PREFLIGHT_BYTES as u64 {
        return false;
    }
    let mut scratch = [0; MAX_PREFLIGHT_BYTES];
    let mut encoder = RangeEncoder::new(&mut scratch[..capacity as usize]);
    let before = encoder.tell_eighths();
    if encode_header(
        &mut encoder,
        plan.duration,
        plan.channels,
        plan.activity,
        plan.redundancy,
    )
    .is_err()
    {
        return false;
    }
    if let Some(redundancy) = plan.redundancy
        && redundancy.encode_frames(&mut encoder).is_err()
    {
        return false;
    }
    for interval in 0..plan.duration.frames() {
        if plan.channels == 2
            && encode_zero_stereo(
                &mut encoder,
                if plan.activity[1][interval] {
                    None
                } else {
                    Some(!plan.side_coded[interval])
                },
            )
            .is_err()
        {
            return false;
        }
        for channel in 0..usize::from(plan.channels) {
            let stage = &plan.stages[interval * 2 + channel];
            if stage.coded && encode_frame(&mut encoder, plan.frame, stage).is_err() {
                return false;
            }
        }
    }
    if encoder.tell_eighths() - before > u64::from(target_bits) * 8 {
        return false;
    }
    encoder.finish().is_ok()
}

fn extract(
    pcm: &[f32],
    channels: u8,
    samples: usize,
    interval: usize,
    channel: usize,
    output: &mut [f32],
) {
    let start = interval * samples * usize::from(channels);
    for (index, destination) in output[..samples].iter_mut().enumerate() {
        *destination = if channels == 1 {
            pcm[start + index]
        } else {
            let left = pcm[start + index * 2];
            let right = pcm[start + index * 2 + 1];
            if channel == 0 {
                0.5 * (left + right)
            } else {
                0.5 * (left - right)
            }
        };
    }
}

fn encode_metadata(
    encoder: &mut RangeEncoder<'_>,
    frame: Frame,
    stage: &CodedFrame,
) -> Result<(), Error> {
    encoder.symbol(
        usize::from(stage.kind.offset == QuantizationOffset::High)
            + if stage.kind.signal == SignalType::Voiced {
                2
            } else {
                0
            },
        if stage.kind.signal == SignalType::Inactive {
            INACTIVE
        } else {
            ACTIVE
        },
    )?;
    let subframes = if frame.duration == FrameDuration::Ms10 {
        2
    } else {
        4
    };
    for subframe in 0..subframes {
        encode_gain_symbol(
            encoder,
            stage.kind.signal,
            stage.gains[subframe],
            subframe == 0 && !stage.conditional,
        )?;
    }
    encode_nlsf_indices(encoder, frame, stage.kind.signal, stage.spectrum.indices)?;
    stage.pitch.encode(
        encoder,
        frame,
        stage.conditional,
        stage.previous_voiced,
        stage.independent_scale,
    )?;
    encoder.symbol(0, SEED)
}

pub(super) fn encode_frame(
    encoder: &mut RangeEncoder<'_>,
    frame: Frame,
    stage: &CodedFrame,
) -> Result<(), Error> {
    encode_metadata(encoder, frame, stage)?;
    encode_pulses(
        encoder,
        frame,
        stage.kind,
        &stage.pulses[..frame.coded_samples()],
    )
}

pub(super) fn estimated_cost(frame: Frame, stage: &CodedFrame) -> Result<f32, Error> {
    let mut scratch = [0; 1024];
    let mut encoder = RangeEncoder::new(&mut scratch);
    encode_metadata(&mut encoder, frame, stage)?;
    let metadata = encoder.tell_eighths() as f32 / 8.0;
    Ok(metadata + estimate_pulse_bits(frame, stage.kind, &stage.pulses[..frame.coded_samples()])?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::opus::silk::SilkDecoder;

    fn sparse_parameters(frame: Frame, signal: SignalType) -> StageParameters {
        StageParameters {
            frame,
            kind: FrameType {
                signal,
                offset: QuantizationOffset::Low,
            },
            spectrum: analyze_spectrum(&[0.0; MAX_SAMPLES][..frame.samples()], frame.bandwidth),
            pitch: if signal == SignalType::Voiced {
                Pitch {
                    lag: frame.samples() / 4,
                    ..Pitch::NONE
                }
            } else {
                Pitch::NONE
            },
            previous: History::new(),
            conditional: false,
            independent_scale: true,
        }
    }

    #[test]
    fn sparse_bank_is_bounded_and_never_relabels_active_audio_as_inactive() {
        assert_eq!(SPARSE_MODELS * SPARSE_POINTS.len(), 96);
        assert_eq!(
            SPARSE_POINTS
                .iter()
                .filter(|(_, penalty)| penalty.is_none())
                .count(),
            3
        );
        for frame in [
            Frame {
                bandwidth: Bandwidth::Narrow,
                duration: FrameDuration::Ms10,
            },
            Frame {
                bandwidth: Bandwidth::Wide,
                duration: FrameDuration::Ms20,
            },
        ] {
            for signal in [
                SignalType::Inactive,
                SignalType::Unvoiced,
                SignalType::Voiced,
            ] {
                let mut original = sparse_parameters(frame, signal);
                original.previous.gain = Some(40);
                original.previous.output[305] = 0.125;
                original.previous.lpc[0] = -0.25;
                original.previous.shaping_error = -0.0;
                let mut count = 0;
                for index in 0..SPARSE_MODELS {
                    let Some(model) =
                        sparse_model(&[0.0; MAX_SAMPLES][..frame.samples()], original, index)
                    else {
                        continue;
                    };
                    count += 1;
                    assert_eq!(
                        model.kind.signal == SignalType::Inactive,
                        signal == SignalType::Inactive
                    );
                    assert_eq!(model.previous.gain, original.previous.gain);
                    assert_eq!(model.previous.output, original.previous.output);
                    assert_eq!(model.previous.lpc, original.previous.lpc);
                    assert_eq!(
                        model.previous.shaping_error.to_bits(),
                        original.previous.shaping_error.to_bits()
                    );
                    if (1..=5).contains(&index) {
                        assert_eq!(model.pitch.lag, 0);
                    }
                    if index >= 6 {
                        assert_eq!(model.pitch.lag, original.pitch.lag);
                    }
                }
                assert_eq!(count, if signal == SignalType::Voiced { 8 } else { 6 });
            }
        }
    }

    #[test]
    fn sparse_history_guard_checks_unclamped_predictor_and_shaping_state() {
        let mut history = History::new();
        assert!(finite_history(&history));
        history.lpc[15] = f64::INFINITY;
        assert!(!finite_history(&history));
        history.lpc[15] = 0.0;
        history.output[305] = f64::NAN;
        assert!(!finite_history(&history));
        history.output[305] = 0.0;
        history.shaping_error = f64::NEG_INFINITY;
        assert!(!finite_history(&history));
    }

    #[test]
    fn sparse_cells_fit_active_clipped_nb_and_terminate_with_exact_ranges() {
        let frame = Frame {
            bandwidth: Bandwidth::Narrow,
            duration: FrameDuration::Ms10,
        };
        let input: [f32; 80] =
            std::array::from_fn(|index| if index / 4 % 2 == 0 { 1.0 } else { -1.0 });
        let mut parameters = sparse_parameters(frame, SignalType::Unvoiced);
        // Every nearest pulse at the largest gain is nonzero for this literal
        // full-scale waveform. Its dense shell/sign cost exceeds the budget.
        assert!(choose_stage(&input, parameters, 100.0).is_none());
        let mut decoder = SilkDecoder::new();
        for _ in 0..2 {
            let (stage, history, distortion) =
                choose_sparse_fallback(&input, parameters, 100.0).unwrap();
            assert_eq!(stage.kind.signal, SignalType::Unvoiced);
            assert!(distortion.is_finite());
            assert!(finite_history(&history));
            assert!(sparse_frame_fits(frame, &stage, 100.0));
            let mut stages = [CodedFrame::EMPTY; MAX_CODED_FRAMES];
            stages[0] = stage;
            let activity = [[true, false, false], [false; 3]];
            let side_coded = [false; 3];
            let plan = || LayerPlan {
                frame,
                duration: PacketDuration::Ms10,
                channels: 1,
                activity: &activity,
                side_coded: &side_coded,
                stages: &stages,
                redundancy: None,
            };
            assert!(sparse_layer_fits(plan(), 104));
            assert!(!sparse_layer_fits(plan(), 1));
            assert!(!sparse_layer_fits(plan(), u32::MAX));
            let mut packet = [0; 13];
            let mut entropy = RangeEncoder::new(&mut packet);
            let before = entropy.tell_eighths();
            encode_header(&mut entropy, PacketDuration::Ms10, 1, &activity, None).unwrap();
            encode_frame(&mut entropy, frame, &stage).unwrap();
            let required_bits = (entropy.tell_eighths() - before).div_ceil(8) as u32;
            assert!(!sparse_layer_fits(plan(), required_bits - 1));
            let final_range = entropy.range();
            let length = entropy.finish().unwrap();
            assert!(length <= 13);
            let mut metadata = crate::entropy::Decoder::new(&packet[..length]);
            let header =
                super::super::Header::decode(&mut metadata, PacketDuration::Ms10, 1).unwrap();
            assert_eq!(header.voice_activity(0, 0), Some(true));
            assert_eq!(
                FrameType::decode(&mut metadata, true).signal,
                SignalType::Unvoiced
            );
            let mut entropy = crate::entropy::Decoder::new(&packet[..length]);
            let mut output = [0.0; 80];
            assert_eq!(
                decoder
                    .decode(&mut entropy, frame, PacketDuration::Ms10, 1, &mut output)
                    .unwrap(),
                80
            );
            assert_eq!(entropy.range(), final_range);
            assert!(output.iter().all(|sample| sample.is_finite()));
            parameters.previous = history;
        }
    }

    #[test]
    fn impossible_sparse_budget_leaves_existing_entropy_prefix_and_history_untouched() {
        let frame = Frame {
            bandwidth: Bandwidth::Narrow,
            duration: FrameDuration::Ms10,
        };
        let mut owned = SilkEncoder::new();
        let mut packet = [0xa5; 128];
        let mut expected = packet;
        let mut encoder = RangeEncoder::new(&mut packet);
        encoder.uniform(3, 7).unwrap();
        let before = (encoder.range(), encoder.tell_eighths());
        assert_eq!(
            owned.encode_layer(&mut encoder, frame, PacketDuration::Ms10, 1, &[1.0; 80], 20),
            Err(Error::Output)
        );
        assert_eq!((encoder.range(), encoder.tell_eighths()), before);
        assert_eq!(owned.bandwidth, None);
        assert_eq!(owned.history[0].gain, None);
        assert_eq!(owned.history[0].lpc, [0.0; 16]);
        assert_eq!(owned.history[0].output, [0.0; 306]);
        let length = encoder.finish().unwrap();
        let mut encoder = RangeEncoder::new(&mut expected);
        encoder.uniform(3, 7).unwrap();
        assert_eq!(encoder.finish().unwrap(), length);
        assert_eq!(packet, expected);
    }

    #[test]
    fn low_rate_formant_signal_has_bounded_reconstruction_error() {
        // Independent voiced excitation with three broad formant regions and
        // a changing amplitude. A sine-only test misses spectral side-info
        // consuming the bits that a low-rate rest signal needs.
        let frame = Frame {
            bandwidth: Bandwidth::Wide,
            duration: FrameDuration::Ms20,
        };
        let mut encoder = SilkEncoder::new();
        let mut decoder = SilkDecoder::new();
        let mut signal = [0.0; 320];
        let mut decoded = [0.0; 320];
        let mut last_input = 0.0;
        let mut energy = 0.0;
        let mut error = 0.0;
        for packet in 0..24 {
            for (index, sample) in signal.iter_mut().enumerate() {
                let time = (packet * 320 + index) as f64 / 16_000.0;
                let amplitude = 0.02 * (1.0 + 0.6 * (time * 13.0).sin());
                let mut value = 0.0;
                for harmonic in 1..=28 {
                    let frequency = harmonic as f64 * 137.0;
                    let envelope = [650.0, 1200.0, 2500.0]
                        .into_iter()
                        .map(|center| 1.0 / (1.0 + ((frequency - center) / 160.0).powi(2)))
                        .sum::<f64>();
                    value += envelope * (time * std::f64::consts::TAU * frequency).sin();
                }
                *sample = (amplitude * value) as f32;
            }
            let mut bytes = [0; 64];
            let mut entropy = RangeEncoder::new(&mut bytes);
            let stats = encoder
                .encode_layer(&mut entropy, frame, PacketDuration::Ms20, 1, &signal, 232)
                .unwrap();
            assert!(stats.bits <= 232);
            let expected_range = entropy.range();
            let length = entropy.finish().unwrap();
            let mut entropy = crate::entropy::Decoder::new(&bytes[..length]);
            decoder
                .decode(&mut entropy, frame, PacketDuration::Ms20, 1, &mut decoded)
                .unwrap();
            assert_eq!(entropy.range(), expected_range);
            for (&input, &output) in signal.iter().zip(&decoded) {
                energy += f64::from(last_input).powi(2);
                error += f64::from(output - last_input).powi(2);
                last_input = input;
            }
        }
        let snr = 10.0 * (energy / error).log10();
        // A waveform regression guard, independent of the encoder's retry
        // trigger: error must stay below 40% of this bright signal's energy.
        // The old fitting-but-coarse model has more error power than signal.
        assert!(
            error < 0.4 * energy,
            "low-rate formant reconstruction SNR={snr}"
        );
    }

    #[test]
    fn real_audio_encodes_all_rates_durations_and_channels_with_exact_ranges() {
        for bandwidth in [Bandwidth::Narrow, Bandwidth::Medium, Bandwidth::Wide] {
            for (duration, milliseconds) in [
                (PacketDuration::Ms10, 10),
                (PacketDuration::Ms20, 20),
                (PacketDuration::Ms40, 40),
                (PacketDuration::Ms60, 60),
            ] {
                let frame = Frame {
                    bandwidth,
                    duration: if milliseconds == 10 {
                        FrameDuration::Ms10
                    } else {
                        FrameDuration::Ms20
                    },
                };
                let sample_rate = match bandwidth {
                    Bandwidth::Narrow => 8000,
                    Bandwidth::Medium => 12000,
                    Bandwidth::Wide => 16000,
                };
                for channels in 1..=2 {
                    let mut owned = SilkEncoder::new();
                    let mut decoded = SilkDecoder::new();
                    let total = frame.samples() * duration.frames();
                    let mut signal = [0.0; 1920];
                    let mut output = [0.0; 1920];
                    let mut previous = [0.0; 2];
                    let mut error = 0.0;
                    let mut energy = 0.0;
                    for packet in 0..4 {
                        for index in 0..total {
                            let time = (packet * total + index) as f32 / sample_rate as f32;
                            for channel in 0..usize::from(channels) {
                                signal[index * usize::from(channels) + channel] = 0.18
                                    * ((time
                                        * 2.0
                                        * std::f32::consts::PI
                                        * (180.0 + channel as f32 * 30.0))
                                        .sin()
                                        + 0.25 * (time * 2.0 * std::f32::consts::PI * 540.0).sin());
                            }
                        }
                        let mut bytes = [0; 4096];
                        let mut encoder = RangeEncoder::new(&mut bytes);
                        let budget = 24 * milliseconds * u32::from(channels);
                        let statistics = owned
                            .encode_layer(
                                &mut encoder,
                                frame,
                                duration,
                                channels,
                                &signal[..total * usize::from(channels)],
                                budget,
                            )
                            .unwrap_or_else(|error| panic!("{bandwidth:?}/{milliseconds}ms/{channels}ch packet{packet}: {error}"));
                        assert_eq!(statistics.samples, total);
                        assert!(statistics.bits <= u64::from(budget));
                        let final_range = encoder.range();
                        let length = encoder.finish().unwrap();
                        let mut decoder = crate::entropy::Decoder::new(&bytes[..length]);
                        assert_eq!(
                            decoded
                                .decode(&mut decoder, frame, duration, channels, &mut output)
                                .unwrap(),
                            total
                        );
                        assert_eq!(decoder.range(), final_range);
                        for index in 0..total {
                            for (channel, previous) in
                                previous.iter_mut().enumerate().take(usize::from(channels))
                            {
                                let sample = index * usize::from(channels) + channel;
                                let difference = f64::from(output[sample] - *previous);
                                error += difference * difference;
                                energy += f64::from(*previous).powi(2);
                                *previous = signal[sample];
                            }
                        }
                    }
                    let snr = 10.0 * (energy / error).log10();
                    assert!(
                        snr > 20.0,
                        "{bandwidth:?}/{milliseconds}ms/{channels}ch SNR={snr}"
                    );
                }
            }
        }
    }

    #[test]
    fn uncoded_side_frames_reset_history_before_reappearing_in_same_packet() {
        let frame = Frame {
            bandwidth: Bandwidth::Wide,
            duration: FrameDuration::Ms20,
        };
        let mut owned = SilkEncoder::new();
        let mut decoded = SilkDecoder::new();
        let mut signal = [0.0; 1920];
        for (index, sample) in signal.as_chunks_mut::<2>().0.iter_mut().enumerate() {
            let middle = 0.15 * (index as f32 * 0.1).sin();
            let side = if (320..640).contains(&index) {
                0.0
            } else {
                0.05 * (index as f32 * 0.07).cos()
            };
            sample[0] = middle + side;
            sample[1] = middle - side;
        }
        let mut bytes = [0; 4096];
        let mut encoder = RangeEncoder::new(&mut bytes);
        owned
            .encode_layer(&mut encoder, frame, PacketDuration::Ms60, 2, &signal, 2880)
            .unwrap();
        let range = encoder.range();
        let length = encoder.finish().unwrap();
        let mut decoder = crate::entropy::Decoder::new(&bytes[..length]);
        decoded
            .decode(
                &mut decoder,
                frame,
                PacketDuration::Ms60,
                2,
                &mut [0.0; 1920],
            )
            .unwrap();
        assert_eq!(range, decoder.range());
    }

    #[test]
    fn invalid_input_and_unfit_budget_do_not_emit_or_commit_history() {
        let frame = Frame {
            bandwidth: Bandwidth::Narrow,
            duration: FrameDuration::Ms10,
        };
        let mut owned = SilkEncoder::new();
        let mut bytes = [0; 512];
        let mut entropy = RangeEncoder::new(&mut bytes);
        let state = (entropy.range(), entropy.tell_eighths());
        assert_eq!(
            owned.encode_layer(
                &mut entropy,
                frame,
                PacketDuration::Ms10,
                1,
                &[f32::NAN; 80],
                240
            ),
            Err(Error::Argument)
        );
        assert_eq!(
            owned.encode_layer(&mut entropy, frame, PacketDuration::Ms10, 1, &[0.0; 80], 1),
            Err(Error::Output)
        );
        assert_eq!((entropy.range(), entropy.tell_eighths()), state);
        assert_eq!(owned.bandwidth, None);
        assert_eq!(owned.history[0].gain, None);
    }
}
