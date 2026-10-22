#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Amended holdout: metadata-only test-other speakers absent from legacy inventories."""
import argparse
from collections import defaultdict
import hashlib
import io
import json
from pathlib import Path
import urllib.request
from prepare_plc_causal_corpus import SALT,rank

URL='https://huggingface.co/api/datasets/openslr/librispeech_asr/parquet/other/test/0.parquet'

class Remote(io.RawIOBase):
    def __init__(self,url=URL):
        self.url=url;self.position=0;self.transferred=0
        with urllib.request.urlopen(urllib.request.Request(url,headers={'Range':'bytes=0-0'}),timeout=45) as f:
            if f.status!=206:raise ValueError('metadata range unsupported')
            self.size=int(f.headers['Content-Range'].split('/')[-1]);f.read(1)
    def readable(self):return True
    def seekable(self):return True
    def tell(self):return self.position
    def seek(self,offset,whence=0):
        self.position=offset if whence==0 else self.position+offset if whence==1 else self.size+offset
        return self.position
    def read(self,size=-1):
        count=min(self.size-self.position,size if size>=0 else self.size-self.position)
        if count<=0:return b''
        if count>16*1024*1024 or self.transferred+count>24*1024*1024:raise ValueError('metadata-only transfer budget exceeded')
        end=self.position+count-1
        with urllib.request.urlopen(urllib.request.Request(self.url,headers={'Range':f'bytes={self.position}-{end}'}),timeout=45) as f:
            if f.status!=206 or f.headers.get('Content-Range')!=f'bytes {self.position}-{end}/{self.size}':raise ValueError('metadata range differs')
            data=f.read(count+1)
        if len(data)!=count:raise ValueError('short metadata range')
        self.position+=count;self.transferred+=count;return data


def main():
    import pyarrow.parquet as pq
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('inventory',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
    if a.output.exists():p.error('fresh holdout output required')
    inventories=json.loads(a.inventory.read_text());excluded=set()
    for row in inventories.values():excluded.update(row['english_speakers'])
    remote=Remote();table=pq.ParquetFile(remote).read(columns=['id','speaker_id','text'],use_threads=False)
    rows=table.to_pylist();groups=defaultdict(list)
    for index,row in enumerate(rows):
        speaker=str(row['speaker_id'])
        if speaker not in excluded and 25<=len(row['text'].split())<=50:groups[speaker].append({**row,'row_idx':index})
    if len(groups)<12:raise ValueError('not enough historically unexposed speakers')
    selected=[]
    for speaker in sorted(groups,key=rank)[:12]:
        row=min(groups[speaker],key=lambda r:rank(r['id']))
        selected.append(dict(source='librispeech-test-other',speaker_id=speaker,id=row['id'],row_idx=row['row_idx'],split='test',language='en',native_rate=16000))
    data=json.dumps(rows,sort_keys=True).encode();metadata=a.output.with_name(a.output.stem+'-metadata.json');metadata.write_bytes(data)
    report=dict(schema=1,dataset='openslr/librispeech_asr',config='other',split='test',source=URL,
        metadata_sha256=hashlib.sha256(data).hexdigest(),inventory_sha256=hashlib.sha256(a.inventory.read_bytes()).hexdigest(),salt=SALT,
        excluded_legacy_speakers=sorted(excluded),selection=selected,metadata_rows=len(rows),eligible_unexposed_speakers=len(groups),
        metadata_bytes_transferred=remote.transferred,source_parquet_bytes=remote.size,
        stage='metadata only; waveform column not read; no test metrics',
        amendment='initial test-clean cohort invalidated by legacy evaluation exposure; training,ranking and gates unchanged',license='CC-BY-4.0')
    a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(dict(speakers=len(selected),metadata_bytes=remote.transferred,excluded=len(excluded))))

if __name__=='__main__':main()
