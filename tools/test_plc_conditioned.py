#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Synthetic safety checks; never select fits or hyperparameters."""
import unittest,torch
from train_plc_conditioned import Adapter
class AdapterSafety(unittest.TestCase):
 def setUp(self):
  torch.manual_seed(71)
  self.h=torch.rand(32,64);self.c=torch.randn(32,16);self.base=.7875+.425*torch.rand(32,16)
 def test_zero_head_preserves_baseline_bits(self):
  for conditioned in (False,True):
   for damping in (False,True):
    m=Adapter(conditioned,damping)
    for strength in (.25,.5,1.):
     self.assertTrue(torch.equal(m.gains(self.h,self.c,self.base,strength),self.base))
 def test_zero_initialized_damping_head_has_corrective_gradient(self):
  m=Adapter(True,True);delta=m.delta(self.h,self.c)
  loss=((self.base.log()+delta-(self.base.log()-.03))**2).mean()
  loss.backward()
  self.assertGreater(float(m.bias.grad.abs().sum()),0.)
  self.assertTrue(torch.isfinite(m.wh.grad).all())
 def test_damping_never_amplifies_and_controls_stay_bounded(self):
  for damping in (False,True):
   m=Adapter(True,damping)
   with torch.no_grad():
    m.wh.normal_(0,.02);m.wc.normal_(0,.02);m.bias.normal_(0,.02)
    for strength in (.25,.5,1.):
     gains=m.gains(self.h,self.c,self.base,strength)
     self.assertTrue((gains>=.78749).all() and (gains<=1.21251).all())
     if damping:self.assertTrue((gains<=self.base+2e-7).all())
if __name__=='__main__':unittest.main()
