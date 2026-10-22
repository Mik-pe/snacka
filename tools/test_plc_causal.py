#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Native sequence-context checks and pre-ranking guard contract."""
import argparse
import copy
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
import numpy as np
from prepare_plc_sequences import parse
from select_plc_causal import choose
from summarize_plc_representation import admission

class GuardTest(unittest.TestCase):
    def result(self,wave=.99,env=.99,estoi=.001):
        m=dict(lost_error_ratio=wave,full_envelope_ratio=env,estoi_delta=estoi,mrstft_ratio=.99)
        return dict(metrics=dict(en=copy.copy(m),sv=copy.copy(m)),admission=admission(m,m))
    def test_lower_mse_cannot_buy_envelope_or_estoi(self):
        for bad in (self.result(.8,1.001,.003),self.result(.8,.9,-.000001),self.result(.8,.9,float('nan'))):
            self.assertEqual(choose(dict(control=bad,envelope=self.result())), 'envelope')
    def test_no_fallback_without_admission(self):
        self.assertIsNone(choose(dict(control=self.result(1.01),envelope=self.result(env=1.01))))
    def test_both_languages_required(self):
        value=self.result();value['metrics']['sv']['estoi_delta']=-1e-9
        value['admission']=admission(value['metrics']['en'],value['metrics']['sv'])
        self.assertIsNone(choose(dict(envelope=value)))
    def test_envelope_ranks_only_after_admission(self):
        self.assertEqual(choose(dict(control=self.result(.99,.98),envelope=self.result(.8,.99))),'control')


def native(binary):
    with tempfile.TemporaryDirectory(prefix='snacka-context-check-') as tmp:
        root=Path(tmp);subprocess.run([str(binary),'basis',str(root)],check=True)
        samples=np.arange(48000*3,dtype=np.float32)
        pcm=(.2*np.sin(samples*.017)+.05*np.sin(samples*.041)).astype('<f4');pcm.tofile(root/'input.f32')
        receipts=[]
        for n,ms in [(480,10),(960,20)]:
            subprocess.run([str(binary),str(root/'input.f32'),str(root/'events.bin'),'24000',str(ms),'1','24','0'],check=True)
            events=list(parse(root/'events.bin',True));assert events
            basis=np.fromfile(root/f'basis-{n}.f32',dtype='<f4').reshape(n,n+120);worst=0
            for width,h,e in events:
                assert width==n
                assert np.array_equal(e['preceding'],e['target_preceding'])
                coefficients=np.concatenate((e['prediction'],e['recovery'][None,:]));blocks=coefficients@basis
                values=blocks[:,:n].copy();values[0,:120]+=e['overlap'];values[1:,:120]+=blocks[:-1,n:]
                gap=float(np.max(np.abs(np.clip(values,-1,1).flatten()-e['baseline'])));worst=max(worst,gap)
                assert gap<2e-5,gap
            try:list(parse(root/'events.bin',False))
            except ValueError:pass
            else:raise AssertionError('v2 data accepted as v1')
            receipts.append(dict(samples=n,events=len(events),native_identity_max_abs=worst,preceding_matches_received=True))
        print(json.dumps(receipts,indent=2))

if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('--binary',type=Path);a=p.parse_args()
    if a.binary:native(a.binary.resolve())
    result=unittest.TextTestRunner().run(unittest.defaultTestLoader.loadTestsFromTestCase(GuardTest))
    sys.exit(0 if result.wasSuccessful() else 1)
