#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Admission/ranking and rejection of incomplete or substituted validation matrices."""
import copy
import csv
import hashlib
from itertools import product
import json
from pathlib import Path
import tempfile
import unittest
from select_plc_causal import summarize
from select_plc_causal_final import choose
from summarize_plc_phase import admission

class Selection(unittest.TestCase):
    def candidate(self,en,sv,env=.99,wave=.99):
        m={lang:dict(estoi_delta=score,full_envelope_ratio=env,lost_error_ratio=wave,mrstft_ratio=.99) for lang,score in [('en',en),('sv',sv)]}
        return dict(metrics=m,admission=admission(m['en'],m['sv']))
    def test_worst_language_precedes_average(self):
        self.assertEqual(choose(dict(envelope=self.candidate(.006,.0001),distill_phase=self.candidate(.002,.002))),'distill_phase')
    def test_better_mse_cannot_override_failed_admission(self):
        self.assertEqual(choose(dict(envelope=self.candidate(.001,.001),distill_phase=self.candidate(.01,.01,env=1.001,wave=.8))),'envelope')
    def test_no_admitted_candidate_means_no_test(self):
        self.assertIsNone(choose(dict(distill_phase=self.candidate(.01,-1e-9))))
    def test_exact_tie_prefers_smaller_head(self):
        self.assertEqual(choose(dict(distill_phase=self.candidate(.001,.001),distill_gain=self.candidate(.001,.001))),'distill_gain')

class Coverage(unittest.TestCase):
    def setUp(self):
        self.temp=tempfile.TemporaryDirectory();self.root=Path(self.temp.name);self.selection=self.root/'selection.json';self.path=self.root/'quality.csv'
        self.settings=dict(conditions=['clean','noise20','noise10'],bitrates=[24000],durations=[10,20],modes=[0,1],patterns=['periodic1','periodic3','periodic6','independent5','burst40','burst80'])
        sources=[dict(source='source-'+lang,group_id=str(i//2),path=f'{lang}-{i}.flac',language=lang,native_rate=16000 if lang=='sv' else 48000) for lang in ['en','sv'] for i in range(12)]
        self.selection.write_text(json.dumps(sources));self.rows=[]
        for source in sources:
            for condition,rate,ms,mode,pattern in product(*[self.settings[k] for k in ['conditions','bitrates','durations','modes','patterns']]):
                row=dict(source=source['source'],group_id=source['group_id'],clip=Path(source['path']).stem,language=source['language'],native_rate=source['native_rate'],condition=condition,bitrate=rate,frame_ms=ms,harmonic=mode,pattern=pattern,input_sha256='0'*64,delta_estoi=.001,delta_stoi=.001)
                for label in ['baseline','current']:
                    for name,value in dict(lost_error_energy=1.,lowband_mdct_error_energy=1.,log_envelope_squared_sum=1.,log_envelope_count=16,mrstft_magnitude_error=1.,estoi=.6,stoi=.7).items():row[label+'_'+name]=value
                row['current_lost_error_energy']=.99;row['current_log_envelope_squared_sum']=.99;row['current_estoi']=.601
                self.rows.append(row)
    def tearDown(self):self.temp.cleanup()
    def save(self,rows=None,status='COMPLETE'):
        rows=self.rows if rows is None else rows
        with self.path.open('w') as f:
            writer=csv.DictWriter(f,fieldnames=list(self.rows[0]));writer.writeheader();writer.writerows(rows)
        self.path.with_suffix('.json').write_text(json.dumps(dict(status=status,rows=len(rows),selection_sha256=hashlib.sha256(self.selection.read_bytes()).hexdigest())))
    def test_complete_registered_matrix(self):
        self.save();self.assertTrue(summarize(self.path,self.selection,self.settings)['admission']['passed'])
    def test_incomplete_status_even_with_all_rows(self):
        self.save(status='INCOMPLETE')
        with self.assertRaises(ValueError):summarize(self.path,self.selection,self.settings)
    def test_duplicate_case_replacing_another_case(self):
        rows=copy.deepcopy(self.rows);rows[-1]=rows[0];self.save(rows)
        with self.assertRaises(ValueError):summarize(self.path,self.selection,self.settings)
    def test_wrong_configuration_cannot_replace_missing_case(self):
        rows=copy.deepcopy(self.rows);rows[0]['bitrate']=12000;self.save(rows)
        with self.assertRaises(ValueError):summarize(self.path,self.selection,self.settings)
    def test_nonfinite_metric_fails_before_ranking(self):
        rows=copy.deepcopy(self.rows);rows[0]['current_estoi']=float('nan');self.save(rows)
        with self.assertRaises(ValueError):summarize(self.path,self.selection,self.settings)

if __name__=='__main__':unittest.main()
