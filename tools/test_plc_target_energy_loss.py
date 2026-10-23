#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Autograd/action-space checks and actual Rust converter parity; no speech."""
import argparse
import hashlib
import json
import math
from pathlib import Path
import struct
import unittest
import torch
from plc_target_energy_loss import LOW, HIGH, objective, analytic_gains


class LossTests(unittest.TestCase):
    def score(self, raw, last=None, predicted=None, target=None, mask=None):
        zero = torch.zeros_like(raw)
        return objective(raw, "target_energy", zero if last is None else last,
                         zero if predicted is None else predicted, zero if target is None else target,
                         torch.ones_like(raw) if mask is None else mask,
                         torch.zeros(raw.shape[0]), torch.zeros(raw.shape[0]))

    def test_wrong_saturation_retains_corrective_gradient(self):
        for prediction, target, sign in [(8.0, -8.0, 1), (-8.0, 8.0, -1)]:
            raw = torch.full((2,16), prediction, requires_grad=True)
            self.score(raw, target=torch.full_like(raw, target)).backward()
            self.assertTrue((raw.grad * sign > 0).all())
        # The naive post-clamp supervision really does lose that gradient.
        raw = torch.tensor([8.0], requires_grad=True)
        ((0.5 * raw).clamp(LOW, HIGH) - LOW).square().sum().backward()
        self.assertEqual(raw.grad.item(), 0.0)

    def test_irrelevant_teacher_extremes_have_identical_loss_and_gradient(self):
        values = []
        for target in [-8.0, -80.0, -800.0]:
            raw = torch.zeros((2,16), requires_grad=True)
            loss = self.score(raw, target=torch.full_like(raw, target))
            loss.backward(); values.append((loss.detach(), raw.grad))
        for loss, grad in values[1:]:
            self.assertTrue(torch.equal(loss, values[0][0]))
            self.assertTrue(torch.equal(grad, values[0][1]))

    def test_identity_tracks_classical_fade_instead_of_last_energy(self):
        last = torch.zeros((2,16))
        predicted = torch.full_like(last, 2 * math.log(0.5))
        correct = self.score(predicted - last, last, predicted, predicted)
        hold_last = self.score(torch.zeros_like(last), last, predicted, predicted)
        self.assertAlmostEqual(correct.item(), 0.0)
        self.assertGreater(hold_last.item(), 0.0)

    def test_mask_is_per_record_and_each_record_has_equal_weight(self):
        raw = torch.full((2,16), 0.1, requires_grad=True)
        mask = torch.ones_like(raw); mask[1,13:] = 0
        self.score(raw, mask=mask).backward()
        self.assertTrue((raw.grad[1,13:] == 0).all())
        self.assertAlmostEqual(raw.grad[0].sum().item(), raw.grad[1].sum().item(), places=6)
        with self.assertRaises(ValueError): self.score(raw, mask=torch.zeros_like(raw))

    def test_ratio_and_energy_have_the_same_objective_for_the_same_action(self):
        ratio = torch.full((2,16), 0.2)
        z = torch.log(1 + 0.2125 * ratio.tanh())
        zeros = torch.zeros_like(ratio); mask = torch.ones_like(ratio)
        phase = torch.zeros(2)
        a = objective(ratio, "gain_ratio", zeros, zeros, zeros, mask, phase, phase)
        b = objective(2*z, "target_energy", zeros, zeros, zeros, mask, phase, phase)
        self.assertTrue(torch.equal(a,b))


def parity(path):
    raw = path.read_bytes(); at = 8
    if raw[:8] != b"STEG\x01\0\0\0": raise ValueError("fixture schema")
    maximum = 0.0; records = 0
    hz = [0,200,400,600,800,1000,1300,1600,2000,2500,3200,4000,5200,6800,9000,13000,24000]
    while at < len(raw):
        n, = struct.unpack_from('<I', raw, at); at += 4
        if n not in (480,960): raise ValueError("frame bins")
        def values(count):
            nonlocal at
            data = torch.tensor(struct.unpack_from('<'+str(count)+'f',raw,at), dtype=torch.float32)
            at += 4 * count
            return data
        coefficients, last, delta, expected = values(n), values(16), values(16), values(16)
        edges = [h*n//24000 for h in hz]
        actual = analytic_gains(coefficients,last,delta,edges)
        maximum = max(maximum, (actual-expected).abs().max().item()); records += 1
    if records != 32 or maximum > 5e-6: raise ValueError(f"native parity {records=} {maximum=}")
    return dict(records=records,max_absolute_gain_error=maximum,fixture_sha256=hashlib.sha256(raw).hexdigest(),torch=torch.__version__)


if __name__ == '__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--parity',type=Path,required=True)
    parser.add_argument('--report',type=Path,required=True)
    args=parser.parse_args()
    suite=unittest.defaultTestLoader.loadTestsFromTestCase(LossTests)
    result=unittest.TextTestRunner(verbosity=2).run(suite)
    if not result.wasSuccessful(): raise SystemExit(1)
    report=parity(args.parity);report['autograd_tests_passed']=result.testsRun
    args.report.write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(report))
