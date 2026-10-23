#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Matched bounded-action distillation primitives; no trainer or test data reads.

L_pred is measured at the *teacher phase* for the offline objective. Production
conversion always measures the actual student-phase spectrum. This deliberate
proxy is evaluated with matched phase-off/on validation, not called identical.
"""
import math
import torch
import torch.nn.functional as F

LOW = math.log(0.7875)
HIGH = math.log(1.2125)
MODEL_KIND = "target-log-power-phase-v1"


def log_gain(raw, representation, last, predicted):
    if representation == "gain_ratio":
        return torch.log(1 + 0.2125 * torch.tanh(raw))
    if representation == "target_energy":
        return 0.5 * (raw + last - predicted)
    raise ValueError("unknown representation")


def objective(raw, representation, last, predicted, target, mask, phase, phase_target):
    # Clip the TEACHER target, not the student, for supervision. This preserves
    # a corrective gradient when a student saturates at the wrong bound.
    desired = (0.5 * (target - predicted)).clamp(LOW, HIGH)
    actual = log_gain(raw, representation, last, predicted)
    error = F.smooth_l1_loss(actual, desired, beta=0.05, reduction="none")
    # Anchor in action space, including the current classic fade, never d=0.
    gain = actual.clamp(LOW, HIGH).exp()
    anchor = 0.01 * (gain - 1).square()
    denominator = mask.sum(-1)
    if (denominator <= 0).any().item():
        raise ValueError("no supported bands")
    bands = (((error + anchor) * mask).sum(-1) / denominator).mean()
    phase_loss = 0.1 * F.smooth_l1_loss(phase / 8, phase_target / 8)
    phase_anchor = 0.001 * (phase / 8).square().mean()
    return bands + phase_loss + phase_anchor


def analytic_gains(coefficients, last, delta, edges):
    current = torch.stack([
        torch.log(coefficients[..., left:right].square().mean(-1) + 1e-12)
        for left, right in zip(edges[:-1], edges[1:])
    ], -1)
    gain = (0.5 * (delta + last - current)).clamp(LOW, HIGH).exp()
    valid = torch.isfinite(coefficients).all(-1) & torch.isfinite(last).all(-1) & torch.isfinite(delta).all(-1)
    return torch.where(valid.unsqueeze(-1), gain, torch.ones_like(gain))
