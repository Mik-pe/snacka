# SPDX-License-Identifier: MIT
"""The guard must reject MSE-only wins and all incomplete/nonfinite evidence."""
import unittest
from itertools import product
from summarize_plc_representation import admission, audit, VARIANTS


class GuardTests(unittest.TestCase):
    def setUp(self):
        self.good=dict(lost_error_ratio=.97,full_envelope_ratio=.99,estoi_delta=.0003)

    def test_perceptual_gain_may_pass_small_error_improvement(self):
        self.assertTrue(admission(self.good,self.good)['passed'])

    def test_error_gain_cannot_buy_worse_envelope_or_intelligibility(self):
        for bad in (dict(full_envelope_ratio=1.00001),dict(estoi_delta=-.000001)):
            self.assertFalse(admission({**self.good,'lost_error_ratio':.5,**bad},self.good)['passed'])

    def test_both_languages_required(self):
        self.assertFalse(admission(self.good,{**self.good,'estoi_delta':-.0001})['passed'])

    def test_missing_and_nonfinite_rejected(self):
        for key in self.good:
            missing=self.good.copy();missing.pop(key)
            self.assertFalse(admission(missing,self.good)['passed'])
            for value in (float('nan'),float('inf'),float('-inf')):
                self.assertFalse(admission({**self.good,key:value},self.good)['passed'])


class CoverageTests(unittest.TestCase):
    def setUp(self):
        self.rows=[]
        for lang,i,c,d,m,p,v in product(('en','sv'),range(12),('clean','noise20','noise10'),('10','20'),('0','1'),
            ('periodic1','periodic3','periodic6','independent5','burst40','burst80'),VARIANTS):
            row=dict(source=lang,group_id=str(i),clip=lang+str(i),condition=c,frame_ms=d,harmonic=m,pattern=p,implementation=v,
                language=lang,native_rate='16000' if lang=='sv' else '48000',bitrate='24000',input_sha256='a'*64)
            row.update({k:'1.0' for k in ('lost_error_energy','lowband_mdct_error_energy','mrstft_magnitude_error','stoi','estoi','full_envelope_sum','low_envelope_sum','high_envelope_sum')})
            self.rows.append(row)

    def test_complete_matrix(self):
        self.assertEqual(len(audit(self.rows)),12096)

    def test_missing_duplicate_or_unregistered_pattern_rejected(self):
        for rows in (self.rows[:-1],self.rows+[self.rows[0]],
                     [{**r,'pattern':'new_pattern'} if r['pattern']=='periodic1' else r for r in self.rows]):
            with self.assertRaises(ValueError):audit(rows)

    def test_wrong_input_or_nonfinite_cannot_pass(self):
        for change in (dict(input_sha256='b'*64),dict(estoi='nan')):
            rows=[{**self.rows[0],**change},*self.rows[1:]]
            with self.assertRaises(ValueError):audit(rows)


if __name__=='__main__':
    unittest.main()
