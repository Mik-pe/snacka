#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Freeze SLR83 metadata only; never download or inspect test waveforms.

The publisher's five-digit IDs recur across dialect/gender prefixes. Group them
conservatively across all prefixes and choose one prefix per selected group.
This avoids splitting a potentially shared user ID; it is not biometric proof.
"""
import argparse
from collections import defaultdict
import csv
import hashlib
import json
from pathlib import Path
import re

INDEX_SHA='c4edc0328629278b1ba4288eec8615ba2bdff43137c88823629a56171a404da3'
LICENSE_SHA='bae5ccc863235f984a52c07d011e48a86c0c12f923be2dedd8624c7abdc22c6c'
SALT='snacka-target-energy-20261023:'


def rank(value):return hashlib.sha256((SALT+value).encode()).hexdigest()


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('metadata',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
    if a.output.exists():p.error('fresh output required')
    index=a.metadata/'line_index_all.csv';license_file=a.metadata/'LICENSE'
    if hashlib.sha256(index.read_bytes()).hexdigest()!=INDEX_SHA:raise ValueError('publisher index changed')
    if hashlib.sha256(license_file.read_bytes()).hexdigest()!=LICENSE_SHA:raise ValueError('publisher license changed')
    groups=defaultdict(lambda:defaultdict(list));seen=set();total=0
    for line_id,uid,text in csv.reader(index.read_text().splitlines()):
        line_id,uid,text=line_id.strip(),uid.strip(),text.strip();total+=1
        match=re.fullmatch(r'([a-z]{3})_(\d{5})_(\d{11})',uid)
        if not match or uid in seen:raise ValueError('utterance ID schema or duplicate')
        seen.add(uid)
        groups[match[2]][match[1]].append(dict(utterance_id=uid,sentence_id=line_id,words=len(text.split()),transcript_sha256=hashlib.sha256(text.encode()).hexdigest()))
    eligible={user:{prefix:[r for r in rows if 15<=r['words']<=40] for prefix,rows in prefixes.items()}
              for user,prefixes in groups.items()}
    eligible={user:{prefix:rows for prefix,rows in prefixes.items() if len(rows)>=2} for user,prefixes in eligible.items()}
    eligible={user:prefixes for user,prefixes in eligible.items() if prefixes}
    if len(eligible)<12:raise ValueError('need12 conservative user groups')
    selection=[]
    for user in sorted(eligible,key=lambda s:rank('user:'+s))[:12]:
        prefix=min(eligible[user],key=lambda s:rank('prefix:'+user+':'+s))
        for row in sorted(eligible[user][prefix],key=lambda r:rank('clip:'+r['utterance_id']))[:2]:
            selection.append(dict(source='openslr83',split='acceptance',language='en',nominal_rate=48000,
                speaker_id=f'{prefix}_{user}',conservative_user_group=user,**row))
    result=dict(schema=1,kind='metadata-only fullband holdout reservation',license='CC-BY-SA-4.0',
        publisher='https://www.openslr.org/83/',index_sha256=INDEX_SHA,license_sha256=LICENSE_SHA,
        salt=SALT,rule='12 hash-ranked five-digit user groups; one hash-ranked prefix with>=2 lines15..40words per group;2 hash-ranked utterances',
        inventory=dict(rows=total,prefix_user_pairs=sum(len(p) for p in groups.values()),conservative_groups=len(groups),ids_recurring_across_prefixes=sum(len(p)>1 for p in groups.values())),
        separation='No SLR83 corpus appears in documented prior Snacka datasets. Published anonymous IDs cannot prove cross-corpus biometric disjointness.',
        staging='No audio requested; after sole-candidate selection verify publisher headers48000Hz mono and full checksums; no clip replacement after quality unblinding',selection=selection)
    a.output.write_text(json.dumps(result,indent=2)+'\n')


if __name__=='__main__':main()
