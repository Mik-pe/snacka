#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Paired AB/BA cache-only decoder preflight on fixed synthetic packets."""
import argparse
from collections import defaultdict
import csv
import hashlib
import json
from pathlib import Path
import statistics
import subprocess


def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('staged',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
    if a.output.exists():p.error('fresh output required')
    a.output.mkdir(parents=True)
    binaries={name:(a.staged/name/'target/release/examples/plc_target_energy_probe').resolve() for name in ('accepted','cache')}
    footprints={};traces={}
    for name,binary in binaries.items():
        fp=subprocess.check_output([str(binary.parent/'plc_footprint')],text=True).strip()
        footprints[name]=fp
        path=a.output/f'{name}.trace'
        subprocess.run([str(binary),'trace',str(path)],check=True)
        traces[name]=dict(bytes=path.stat().st_size,sha256=sha(path))
    if traces['accepted']!=traces['cache']:raise ValueError('packet/PCM/feature/reset parity failed')
    for name in binaries:(a.output/f'{name}.trace').unlink()
    fixture=a.output/'analytic-gain-parity.bin'
    subprocess.run([str(binaries['cache']),'parity',str(fixture)],check=True)
    blocks=defaultdict(list);row_counts=defaultdict(int);raw_files={}
    for block in range(9):
        order=['accepted','cache'] if block%2==0 else ['cache','accepted'];paired={}
        for name in order:
            path=a.output/f'block-{block}-{name}.csv'
            subprocess.run([str(binaries[name]),'time',str(path)],check=True)
            raw_files[path.name]=sha(path)
            with path.open() as stream:rows=list(csv.DictReader(stream))
            if len(rows)!=3456:raise ValueError('incomplete synthetic factorial')
            paired[name]={tuple(r[k] for k in ('signal','bitrate','frame_ms','harmonic','frame','operation')):int(r['elapsed_ns'])/1000 for r in rows}
        if paired['accepted'].keys()!=paired['cache'].keys():raise ValueError('case pairing differs')
        if block==0:continue
        by_group=defaultdict(lambda: {'accepted':[],'cache':[],'difference':[]})
        for key,baseline in paired['accepted'].items():
            group=(key[2],key[-1]);values=by_group[group];current=paired['cache'][key]
            values['accepted'].append(baseline);values['cache'].append(current);values['difference'].append(current-baseline)
            row_counts[group]+=1
        for group,values in by_group.items():
            blocks[group].append(dict(block=block,order=order,
                accepted_median_us=statistics.median(values['accepted']),cache_median_us=statistics.median(values['cache']),
                paired_difference_median_us=statistics.median(values['difference'])))
    summary=[]
    for (duration,operation),values in sorted(blocks.items()):
        differences=[v['paired_difference_median_us'] for v in values]
        summary.append(dict(frame_ms=int(duration),operation=operation,paired_calls=row_counts[(duration,operation)],
            median_block_difference_us=statistics.median(differences),min_block_difference_us=min(differences),max_block_difference_us=max(differences),blocks=values))
    report=dict(schema=1,kind='cache-only synthetic decoder; not new model acceptance',
        unchanged_packet_pcm_feature_trace=traces['cache'],footprints=footprints,summary=summary,
        binaries={k:sha(v) for k,v in binaries.items()},raw_csv_sha256=raw_files,
        source_receipt_sha256=sha(a.staged/'source-receipt.json'),analytic_gain_fixture_sha256=sha(fixture),
        design='one discarded warmup block, eight measured paired AB/BA blocks;12/24/32kbps,10/20ms,modes0/1,silence/noisy tone/noise;96frames;1/2/6-frame losses and recovery',
        limits='Wall time on a shared machine. Paired differences include scheduling/cache noise. No newly trained target-energy control runs here and no absolute25us adoption claim.')
    (a.output/'cache-preflight.json').write_text(json.dumps(report,indent=2)+'\n')
    print(json.dumps(dict(footprints=footprints,parity=traces['cache'],summary=[{k:v for k,v in s.items() if k!='blocks'} for s in summary]),indent=2))


if __name__=='__main__':main()
