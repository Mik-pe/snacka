#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Cloud-only speaker-frozen VCTK/RixVox-v2 corpus; test audio stays unstaged."""
import argparse
from concurrent.futures import ThreadPoolExecutor
from collections import defaultdict
import hashlib
import json
from pathlib import Path
import re
import urllib.parse
import zipfile
import io
from prepare_neural_corpus import Corpus, RangeReader, VCTK, VCTK_SIZE, split_for, request

RIX_REV='1f5f37f5ec8740eae318eeae7bf190074454d0d1'
SALT='snacka-causal-envelope-20261022:'

def rank(key): return hashlib.sha256((SALT+key).encode()).hexdigest()

def rows(dataset,config,split,offset,length=100):
    query=urllib.parse.urlencode(dict(dataset=dataset,config=config,split=split,offset=offset,length=length))
    with request('https://datasets-server.huggingface.co/rows?'+query) as response:
        return json.load(response)['rows']

def rix_candidates(offset):
    return rows('KBLab/rixvox-v2','default','train',offset,30)

def main():
    import soundfile as sf
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('output',type=Path);p.add_argument('--metadata-cache',type=Path);a=p.parse_args()
    root=a.output
    if root.exists():p.error('fresh output required')
    root.mkdir(parents=True);(root/'sources').mkdir();corpus=Corpus(root)
    # Only metadata is inspected during deterministic speaker assignment.
    offsets=[3420000+i*1100 for i in range(128)]
    groups=defaultdict(list)
    if a.metadata_cache:
        batches=json.loads((a.metadata_cache/'rixvox-rows.json').read_text())
    else:
        with ThreadPoolExecutor(max_workers=2) as pool:batches=list(pool.map(rix_candidates,offsets))
    for batch in batches:
        for item in batch:
            row=item['row'];identity=row['riksdagen_id']
            if row['year']<2015 or not identity or row['speaker_id']!=identity:continue
            if row['is_silence'] or row['lang_prob_sv']<.99 or row['duration']<12000:continue
            # Pin the actual audio revision, not just current repository metadata.
            if f'/--/{RIX_REV}/--/' not in row['audio'][0]['src']:raise ValueError('RixVox audio revision drift')
            groups[identity].append(item)
    eligible={speaker:sorted({r['row_idx']:r for r in items}.values(),key=lambda r:rank(str(r['row_idx'])))
              for speaker,items in groups.items() if len({r['row_idx'] for r in items})>=4}
    ordered=sorted(eligible,key=rank)
    if len(ordered)<50:raise ValueError(f'need 50 identified Swedish speakers, got {len(ordered)}')
    assignment={s:('train' if i<32 else 'validation' if i<38 else 'test') for i,s in enumerate(ordered[:50])}
    selection=[]
    for speaker,split in assignment.items():
        count=4 if split=='train' else 2 if split=='validation' else 1
        for item in eligible[speaker][:count]:
            r=item['row'];selection.append(dict(source='rixvox-v2',speaker_id=speaker,split=split,row_idx=item['row_idx'],
                revision=RIX_REV,gender=r['gender'],name=r['name'],year=r['year'],speech_id=r['speech_id'],
                protocol_id=r['protocol_id'],metadata_duration=r['duration']))
    # Metadata freezes the holdout now; no test PCM is requested here.
    (root/'sources/rixvox-selection.json').write_text(json.dumps(selection,indent=2)+'\n')
    lookup={item['row_idx']:item['row'] for values in eligible.values() for item in values}
    for chosen in selection:
        if chosen['split']=='test':continue
        r=lookup[chosen['row_idx']]
        with request(r['audio'][0]['src']) as response: raw=response.read(2*1024*1024)
        values,rate=sf.read(io.BytesIO(raw),dtype='float32')
        if rate!=16000 or len(values)<12*rate:raise ValueError('RixVox duration/rate mismatch')
        start=(len(values)-12*rate)//2;values=values[start:start+12*rate]
        stream=io.BytesIO();sf.write(stream,values,rate,format='WAV',subtype='FLOAT')
        corpus.add(stream.getvalue(),'rixvox-v2',f"rixvox-{chosen['row_idx']}.wav",chosen['speaker_id'],chosen['split'],16000,'sv',
            revision=RIX_REV,split_claim='riksdagen_id-disjoint; canonical speaker_id equality verified',
            source_row_idx=chosen['row_idx'],original_audio_sha256=hashlib.sha256(raw).hexdigest(),crop_start_samples=start)
        corpus.rows[-1]['license']='ODC-BY-1.0'
        print('SV',chosen['split'],chosen['row_idx'],flush=True)
    reader=RangeReader(VCTK,VCTK_SIZE)
    with zipfile.ZipFile(reader) as archive:
        speakers=defaultdict(list)
        for member in archive.infolist():
            match=re.search(r'wav48_silence_trimmed/(p\d+)/[^/]+_mic2\.flac$',member.filename)
            if match and match[1] not in {'p280','p315'}:speakers[match[1]].append(member)
        en_assignment={}
        for split,count,clips in [('train',24,8),('validation',6,2)]:
            selected=sorted((s for s in speakers if split_for('vctk',s)==split),key=rank)[:count]
            if len(selected)!=count:raise ValueError('VCTK speaker count')
            for speaker in selected:
                members=sorted(speakers[speaker],key=lambda m:rank(m.filename))[:clips]
                reader.prefetch(members);en_assignment[speaker]=split
                for member in members:
                    corpus.add(archive.read(member),'vctk',member.filename,speaker,split,48000,'en',
                        revision='0.92',split_claim='speaker-disjoint',microphone='mic2')
                print('EN',speaker,split,flush=True)
    (root/'sources/vctk-speaker-splits.json').write_text(json.dumps(en_assignment,indent=2)+'\n')
    # LibriSpeech test-clean introduces a different English corpus; only metadata.
    lib=[]
    if a.metadata_cache:lib=json.loads((a.metadata_cache/'librispeech-rows.json').read_text())
    else:
        for offset in range(0,2620,100):lib.extend(rows('openslr/librispeech_asr','clean','test',offset,min(100,2620-offset)))
    by_speaker=defaultdict(list)
    for item in lib:by_speaker[str(item['row']['speaker_id'])].append(item['row'])
    holdout=[]
    for speaker in sorted(by_speaker,key=rank)[:12]:
        usable=[r for r in by_speaker[speaker] if 25<=len(r['text'].split())<=50]
        if not usable:raise ValueError('English holdout speaker lacks a 25–50 word segment')
        r=min(usable,key=lambda r:rank(r['id']))
        holdout.append(dict(source='librispeech-test-clean',speaker_id=speaker,id=r['id'],split='test',language='en'))
    (root/'sources/english-holdout-selection.json').write_text(json.dumps(holdout,indent=2)+'\n')
    for name,url in [('CC-BY-4.0.txt','https://creativecommons.org/licenses/by/4.0/legalcode.txt'),('RixVox-v2-README.md',f'https://huggingface.co/datasets/KBLab/rixvox-v2/raw/{RIX_REV}/README.md'),
                     ('ODC-BY-1.0.html','https://opendatacommons.org/licenses/by/1-0/')]:
        cached=a.metadata_cache/name if a.metadata_cache else None
        if cached and cached.is_file():(root/'sources'/name).write_bytes(cached.read_bytes())
        else:
            with request(url) as response:(root/'sources'/name).write_bytes(response.read(256*1024))
    corpus.finish(False,True)
    (root/'README.txt').write_text('Contains information from RixVox-v2, made available under the ODC Attribution License. RixVox-v2 contains information sourced from The Swedish Parliament open data.\nVCTK0.92: Yamagishi, Veaux, MacDonald, 2019, CC-BY4.0, DOI10.7488/ds/2645.\nNo test PCM staged. Speaker IDs are metadata identities, not biometric verification. Older FLEURS baseline speaker overlap cannot be audited.\n')

if __name__=='__main__':main()
