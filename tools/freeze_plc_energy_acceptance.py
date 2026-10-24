#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Reserve only metadata for fresh LibriSpeech/RixVox identities before fits."""
import argparse,hashlib,json
from pathlib import Path
from collections import defaultdict
from concurrent.futures import ThreadPoolExecutor
from prepare_plc_causal_corpus import rix_candidates,RIX_REV
SALT='snacka-target-energy-20261023:'
def sha(b):return hashlib.sha256(b).hexdigest()
def rank(s):return sha((SALT+str(s)).encode())
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('prior',type=Path);p.add_argument('corpus',type=Path);p.add_argument('output',type=Path);p.add_argument('--metadata-cache',type=Path);a=p.parse_args()
 if a.output.exists():p.error('fresh output required')
 a.output.mkdir(parents=True)
 inv=json.loads((a.prior/'phase1-speaker-inventory.json').read_text());ex=set()
 for r in inv.values():ex.update(map(str,r['english_speakers']))
 old=json.loads((a.prior/'a-metadata/english-holdout-amended.json').read_text());ex.update(str(r['speaker_id']) for r in old['selection'])
 # Exclude the initial metadata-selected cohort conservatively as well.
 oldclean=a.prior/'a-metadata/corpus-registered/sources/english-holdout.json'
 if oldclean.exists():ex.update(str(r['speaker_id']) for r in json.loads(oldclean.read_text()))
 metadata=json.loads((a.prior/'a-metadata/english-holdout-amended-metadata.json').read_text());groups=defaultdict(list)
 for i,r in enumerate(metadata):
  if str(r['speaker_id']) not in ex and 25<=len(r['text'].split())<=50:groups[str(r['speaker_id'])].append({**r,'row_idx':i})
 assert len(groups)>=12
 en=[]
 for s in sorted(groups,key=rank)[:12]:
  r=min(groups[s],key=lambda r:rank(r['id']));en.append(dict(source='librispeech-test-other',speaker_id=s,id=r['id'],row_idx=r['row_idx'],native_rate=16000,words=len(r['text'].split())))
 rixold=json.loads((a.corpus/'corpus-registered/sources/rixvox-selection.json').read_text());rx={r['speaker_id'] for r in rixold};assert len(rx)==50
 # Fixed metadata window, no waveform column dereference or quality inspection.
 offsets=[3500000+1100*i for i in range(64)]
 if a.metadata_cache:
  batches=json.loads(a.metadata_cache.read_text());offsets=[batch[0]['row_idx'] for batch in batches]
 else:
  with ThreadPoolExecutor(max_workers=2) as pool:batches=list(pool.map(rix_candidates,offsets))
 groups=defaultdict(list);safe=[]
 for batch in batches:
  for item in batch:
   r=item['row'];s=r['riksdagen_id']
   if s in rx or not s or r['speaker_id']!=s or r['year']<2015 or r['is_silence'] or r['lang_prob_sv']<.99 or r['duration']<12000:continue
   assert '/--/'+RIX_REV+'/--/' in r['audio'][0]['src']
   record=dict(source='rixvox-v2',speaker_id=s,riksdagen_id=s,row_idx=item['row_idx'],year=r['year'],duration=r['duration'],lang_prob_sv=r['lang_prob_sv'],revision=RIX_REV,native_rate=16000,speech_id=r['speech_id'])
   groups[s].append(record);safe.append(record)
 assert len(groups)>=12, f'only {len(groups)} fresh Swedish identities'
 sv=[min(groups[s],key=lambda r:rank(r['row_idx'])) for s in sorted(groups,key=rank)[:12]]
 result=dict(schema=1,salt=SALT,stage='metadata only; no acceptance waveform requested',english_excluded=sorted(ex),swedish_excluded=sorted(rx),english_metadata_sha256=sha((a.prior/'a-metadata/english-holdout-amended-metadata.json').read_bytes()),rix_offsets=offsets,rix_safe_metadata_sha256=sha(json.dumps(safe,sort_keys=True).encode()),selection=en+sv,licenses=dict(librispeech='CC-BY-4.0',rixvox='ODC-BY-1.0'),rule='hash speaker then eligible row; new12 per language; RixVox middle12seconds after admission',slr83_reservation_sha256='82ae9129666bdb4ea839cf6d237f77d168501f5dceceda7009262eb689d434ec')
 (a.output/'safe-rix-metadata.json').write_text(json.dumps(safe,indent=2)+'\n');(a.output/'reservation.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(dict(english=len(en),swedish=len(sv),rix_eligible_groups=len(groups),excluded_english=len(ex))))
if __name__=='__main__':main()
