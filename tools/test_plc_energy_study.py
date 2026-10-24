#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Guard/ranking tests: aggregate gains must not hide a failing bitrate."""
import copy,unittest
from select_plc_energy_study import choose,summarize,ORDER
class SelectionTests(unittest.TestCase):
 def fixture(self):
  groups={n:[] for n in ['classical','pr31',*ORDER]}
  for cell in ('en16k','en48k','sv16k'):
   for rate in ('12000','24000','32000'):
    for dur in ('10','20'):
     for mode in ('0','1'):
      row=dict(clip='a',cell=cell,condition='clean',bitrate=rate,frame_ms=dur,harmonic=mode,pattern='periodic1',input_sha256='same',wave=1.,env=1.,env_count=1.,mr=1.,estoi=.5)
      for n in groups:groups[n].append({**row,**({} if n in ('classical','pr31') else dict(wave=.9,env=.9,mr=.9,estoi=.501))})
  return groups
 def test_per_rate_failure_is_not_bought_by_other_rates(self):
  g=self.fixture()
  for r in g[ORDER[0]]:
   if r['cell']=='en16k' and r['bitrate']=='12000':r['env']=1.01
  result=summarize(g,ORDER[0]);self.assertFalse(result['admitted']);self.assertTrue(any(r['slice']=='en16k:12000' and r['metric']=='env_ratio' for r in result['failed_guards']))
 def test_failed_best_score_is_never_selected(self):
  g=self.fixture();results={n:summarize(g,n) for n in ORDER};results[ORDER[0]]['admitted']=False
  self.assertEqual(choose(results),ORDER[1])
  for r in results.values():r['admitted']=False
  self.assertIsNone(choose(results))
 def test_reference_pairing_is_required(self):
  g=self.fixture();g['pr31'][0]['input_sha256']='other'
  with self.assertRaises(AssertionError):summarize(g,ORDER[0])
if __name__=='__main__':unittest.main()
