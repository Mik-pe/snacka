#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Stage only the frozen, amended acceptance cohort after causal selection.

Run on cloud storage. The RixVox metadata cache is private transient provider
state and must not be placed in an evidence archive.
"""
import argparse
import hashlib
import io
import json
from pathlib import Path
import shutil
import pyarrow.parquet as pq
import soundfile as sf
from freeze_plc_unseen_english import Remote
from prepare_neural_corpus import Corpus,request
from prepare_plc_causal_corpus import RIX_REV


class AudioRemote(Remote):
    def read(self,size=-1):
        count=min(self.size-self.position,size if size>=0 else self.size-self.position)
        if count<=0:return b''
        if count>128*1024*1024 or self.transferred+count>512*1024*1024:raise ValueError('bounded selective audio transfer exceeded')
        start=self.position;end=start+count-1
        with request(self.url,{'Range':f'bytes={start}-{end}'}) as response:
            if response.status!=206 or response.headers.get('Content-Range')!=f'bytes {start}-{end}/{self.size}':raise ValueError('audio range differs')
            data=response.read(count+1)
        if len(data)!=count:raise ValueError('short audio range')
        self.position+=count;self.transferred+=count;return data


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('output',type=Path)
    for name in ['decision','english','rix-selection','rix-cache','licenses']:p.add_argument('--'+name,type=Path,required=True)
    a=p.parse_args()
    if a.output.exists():p.error('fresh cloud output required')
    decision=json.loads(a.decision.read_text())
    if decision['selected'] is None or decision['test_audio_seen']:p.error('frozen admitted selection required')
    english_bytes=a.english.read_bytes()
    if hashlib.sha256(english_bytes).hexdigest()!='1629cad9629ac05b359b351dbeb9a65be077d7407d5f1dfa02408368fdc03b21':p.error('amended English cohort differs')
    english=json.loads(english_bytes);selected=english['selection']
    if len(selected)!=12 or len({r['speaker_id'] for r in selected})!=12:raise ValueError('English speaker coverage')
    a.output.mkdir();(a.output/'sources').mkdir();corpus=Corpus(a.output)
    shutil.copy2(a.english,a.output/'sources/english-holdout-amended.json')
    shutil.copy2(a.rix_selection,a.output/'sources/rixvox-selection.json')
    for name in ['CC-BY-4.0.txt','ODC-BY-1.0.txt','RixVox-v2-README.md']:
        shutil.copy2(a.licenses/name,a.output/'sources'/name)
    remote=AudioRemote();parquet=pq.ParquetFile(remote);starts=[];at=0
    for i in range(parquet.metadata.num_row_groups):starts.append((at,at+parquet.metadata.row_group(i).num_rows,i));at=starts[-1][1]
    groups={next(i for lo,hi,i in starts if lo<=row['row_idx']<hi) for row in selected}
    for group in sorted(groups):
        lo=starts[group][0];table=parquet.read_row_group(group,columns=['id','speaker_id','audio'],use_threads=False)
        for row in selected:
            if not lo<=row['row_idx']<starts[group][1]:continue
            item=table.slice(row['row_idx']-lo,1).to_pylist()[0]
            if item['id']!=row['id'] or str(item['speaker_id'])!=row['speaker_id']:raise ValueError('English frozen row differs')
            raw=item['audio']['bytes']
            corpus.add(raw,'librispeech-test-other',row['id']+'.flac',row['speaker_id'],'test',16000,'en',revision='OpenSLR12',split_claim='metadata speaker absent from72 documented legacy IDs',source_row_idx=row['row_idx'])
            print('EN staged',row['id'],flush=True)
    cache=json.loads(a.rix_cache.read_text());lookup={item['row_idx']:item['row'] for batch in cache for item in batch}
    swedish=[row for row in json.loads(a.rix_selection.read_text()) if row['split']=='test']
    if len(swedish)!=12 or len({r['speaker_id'] for r in swedish})!=12:raise ValueError('Swedish speaker coverage')
    for chosen in swedish:
        row=lookup[chosen['row_idx']]
        if row['speaker_id']!=chosen['speaker_id'] or row['riksdagen_id']!=chosen['speaker_id']:raise ValueError('Swedish frozen identity differs')
        url=row['audio'][0]['src']
        if f'/--/{RIX_REV}/--/' not in url:raise ValueError('Swedish revision differs')
        with request(url) as response:raw=response.read(2*1024*1024+1)
        if len(raw)>2*1024*1024:raise ValueError('Swedish audio budget exceeded')
        values,rate=sf.read(io.BytesIO(raw),dtype='float32')
        if rate!=16000 or len(values)<12*rate:raise ValueError('Swedish duration/rate mismatch')
        start=(len(values)-12*rate)//2;stream=io.BytesIO();sf.write(stream,values[start:start+12*rate],rate,format='WAV',subtype='FLOAT')
        corpus.add(stream.getvalue(),'rixvox-v2',f"rixvox-{chosen['row_idx']}.wav",chosen['speaker_id'],'test',16000,'sv',revision=RIX_REV,split_claim='canonical RixVox speaker-disjoint',original_audio_sha256=hashlib.sha256(raw).hexdigest(),crop_start_samples=start,source_row_idx=chosen['row_idx'])
        corpus.rows[-1]['license']='ODC-BY-1.0';print('SV staged',chosen['row_idx'],flush=True)
    corpus.finish(False)
    (a.output/'README.txt').write_text('Contains information from RixVox-v2, made available under the ODC Attribution License. RixVox-v2 contains information sourced from The Swedish Parliament open data.\nLibriSpeech ASR corpus: Panayotov et al., 2015, OpenSLR12, CC-BY-4.0.\nFrozen independent acceptance audio; native16kHz; Swedish middle12second crops. Speaker separation uses published metadata identities.\n')
    rows=[dict(row,corpus_root=str(a.output.resolve())) for row in corpus.rows]
    (a.output/'selection.json').write_text(json.dumps(rows,indent=2)+'\n')
    (a.output/'staging-receipt.json').write_text(json.dumps(dict(decision_sha256=hashlib.sha256(a.decision.read_bytes()).hexdigest(),english_amendment_sha256=hashlib.sha256(english_bytes).hexdigest(),parquet_bytes_transferred=remote.transferred,test_metrics_seen=False,clips=len(rows)),indent=2)+'\n')


if __name__=='__main__':main()
