#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Synthetic gate-contract tests, never corpus or quality evidence."""
import csv
import itertools
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest

ROOT=Path(__file__).resolve().parents[1]


class GateContract(unittest.TestCase):
    def run_gate(self, estoi_delta, lost_error, lowband_ratio=1.0, omit_last=False, supplement=None):
        protocol=json.loads((ROOT/'docs/neural-plc-loss-ablation-protocol.json').read_text())
        evaluation=protocol['evaluation'];rows=[]
        for language,clip,rate,duration,mode,condition,pattern in itertools.product(
                ('en','sv'),range(12),evaluation['bitrates'],evaluation['durations_ms'],evaluation['modes'],evaluation['conditions'],evaluation['loss_patterns']):
            row={'clip':f'{language}{clip}','language':language,'source':language,'group_id':str(clip),
                 'bitrate':rate,'frame_ms':duration,'harmonic':mode,'condition':condition,'pattern':pattern,
                 'input_sha256':'synthetic-fixture-only'}
            for label in ('baseline','current'):
                row.update({label+'_lost_error_energy':1.0,label+'_stoi':.85,label+'_estoi':.70,
                            label+'_lowband_mdct_error_energy':.2,label+'_log_envelope_squared_sum':.5,
                            label+'_log_envelope_count':16,label+'_mrstft_magnitude_error':.4,
                            label+'_onset_snr_db':10.,label+'_recovery_snr_db':10.,label+'_lost_snr_db':10.})
            rows.append(row)
        with tempfile.TemporaryDirectory() as temporary:
            root=Path(temporary)
            for name,candidate in [('installed',False),('candidate',True)]:
                selected=rows[:-1] if omit_last and candidate else rows
                with (root/(name+'.csv')).open('w') as stream:
                    writer=csv.DictWriter(stream,fieldnames=rows[0]);writer.writeheader()
                    for original in selected:
                        row=original.copy()
                        if candidate:
                            row['current_lost_error_energy']=lost_error
                            row['current_estoi']+=estoi_delta;row['current_stoi']+=.001
                            row['current_lowband_mdct_error_energy']*=lowband_ratio
                            row['current_log_envelope_squared_sum']*=.95
                            row['current_mrstft_magnitude_error']*=.95
                            row['current_lost_snr_db']+=.1
                        writer.writerow(row)
                (root/(name+'.json')).write_text(json.dumps({'status':'COMPLETE','binary_sha256':name}))
            command=[sys.executable,str(ROOT/'tools/summarize_plc_ablation.py'),str(root/'installed.csv'),str(root/'candidate.csv'),
                     '--protocol',str(ROOT/'docs/neural-plc-loss-ablation-protocol.json'),'--output',str(root/'report.json')]
            if supplement:
                for name in ('installed','candidate'):
                    with (root/(name+'.csv')).open() as stream:extra=list(csv.DictReader(stream))
                    for row in extra:
                        row['current_highband_log_envelope_squared_sum']=.3 if name=='installed' else .27
                        row['current_highband_log_envelope_count']=8
                    if supplement=='wrong-input' and name=='candidate':extra[0]['input_sha256']='wrong-fixture'
                    with (root/(name+'-bands.csv')).open('w') as stream:
                        writer=csv.DictWriter(stream,fieldnames=extra[0]);writer.writeheader();writer.writerows(extra)
                    binary='wrong-model' if supplement=='wrong-binary' and name=='candidate' else name
                    (root/(name+'-bands.json')).write_text(json.dumps({'status':'COMPLETE','binary_sha256':binary}))
                command+=['--installed-bands',str(root/'installed-bands.csv'),'--candidate-bands',str(root/'candidate-bands.csv')]
            result=subprocess.run(command,capture_output=True,text=True)
            report=json.loads((root/'report.json').read_text()) if (root/'report.json').exists() else None
            return result.returncode,report

    def test_better_mse_cannot_hide_estoi_regression(self):
        code,report=self.run_gate(-.002,.7)
        self.assertNotEqual(code,0)
        self.assertEqual(report['quality_status'],'FAIL')
        self.assertTrue(any('ESTOI' in failure['gate'] for failure in report['failures']))

    def test_perceptual_gain_can_pass_with_worse_total_mse(self):
        code,report=self.run_gate(.002,1.05)
        self.assertEqual(code,0)
        self.assertEqual(report['quality_status'],'PASS')

    def test_lowband_regression_cannot_hide_behind_estoi(self):
        code,report=self.run_gate(.002,.7,lowband_ratio=1.02)
        self.assertNotEqual(code,0)
        self.assertTrue(any('lowband' in failure['gate'] for failure in report['failures']))

    def test_incomplete_case_matrix_fails_closed(self):
        code,report=self.run_gate(.002,.7,omit_last=True)
        self.assertNotEqual(code,0)
        self.assertIsNone(report)

    def test_nonfinite_intelligibility_cannot_pass(self):
        code,report=self.run_gate(float('nan'),.7)
        self.assertNotEqual(code,0)
        self.assertEqual(report['quality_status'],'FAIL')

    def test_highband_supplement_from_another_model_fails_closed(self):
        code,report=self.run_gate(.002,.7,supplement='wrong-binary')
        self.assertNotEqual(code,0)
        self.assertIsNone(report)

    def test_highband_supplement_from_another_input_fails_closed(self):
        code,report=self.run_gate(.002,.7,supplement='wrong-input')
        self.assertNotEqual(code,0)
        self.assertIsNone(report)

    def test_identical_native_replay_can_add_highband_diagnostics(self):
        code,report=self.run_gate(.002,.7,supplement='matching')
        self.assertEqual(code,0)
        self.assertEqual(report['highband_supplement_audit']['status'],'PASS')
        self.assertAlmostEqual(report['highband_envelope'][0]['candidate_over_installed_ratio'],.9)


if __name__=='__main__':unittest.main()
