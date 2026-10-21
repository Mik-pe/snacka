#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Merge only complete, disjoint shards of the exact same oracle protocol."""
import argparse
import csv
import hashlib
import json
from pathlib import Path
import shutil


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('selection',type=Path);p.add_argument('output',type=Path);p.add_argument('shards',type=Path,nargs='+');p.add_argument('--repeat-parts',type=Path)
    a=p.parse_args()
    if a.output.exists():p.error('use a fresh output directory')
    protocols=[json.loads((s/'protocol.json').read_text()) for s in a.shards]
    count=protocols[0]['task_shard'][1];assert sorted(x['task_shard'] for x in protocols)==[[i,count] for i in range(count)]
    common={k:v for k,v in protocols[0].items() if k!='task_shard'}
    assert common['limit'] is None and common['quality']
    assert common['selection_sha256']==hashlib.sha256(a.selection.read_bytes()).hexdigest()
    for x in protocols:assert {k:v for k,v in x.items() if k!='task_shard'}==common
    selected=json.loads(a.selection.read_text())
    expected={hashlib.sha256(json.dumps(dict(source=r['source'],group=r['group_id'],path=r['path'],rate=rate,duration=ms,harmonic=bool(mode)),sort_keys=True).encode()).hexdigest()[:20]
              for r in selected for rate in (12000,24000,32000) for ms in (10,20) for mode in (0,1)}
    observed=set();parts=[];repeat=0;repeat_quality_max_delta=0.0
    for shard in a.shards:
        assert json.loads((shard/'status.json').read_text())['status']=='COMPLETE'
        for path in sorted((shard/'parts').glob('*.json')):
            assert path.stem in expected and path.stem not in observed;observed.add(path.stem)
            data=json.loads(path.read_text());assert data['status']=='COMPLETE'
            stats=path.with_name(data['stats_file']);assert hashlib.sha256(stats.read_bytes()).hexdigest()==data['stats_sha256']
            if a.repeat_parts and (a.repeat_parts/path.name).exists():
                old=json.loads((a.repeat_parts/path.name).read_text());assert old['native']==data['native'];repeat+=1
                oldrows={(r['condition'],r['pattern'],r['implementation']):r for r in old['rows']}
                for row in data['rows']:
                    if row['implementation']=='baseline':continue
                    before=oldrows[(row['condition'],row['pattern'],row['implementation'])]
                    for metric in ('stoi','estoi','lsd_db'):repeat_quality_max_delta=max(repeat_quality_max_delta,abs(row[metric]-before[metric]))
            parts.append((path,stats,data))
    assert observed==expected,'missing tasks'
    a.output.mkdir(parents=True);(a.output/'parts').mkdir();rows=[];native=[]
    for path,stats,saved in parts:
        for source in (path,stats):shutil.copyfile(source,a.output/'parts'/source.name)
        rows.extend(saved['rows'])
        native.extend(dict(x,source=saved['source']['source'],group_id=saved['source']['group_id'],language=saved['source']['language'],bitrate=saved['identity']['rate'],frame_ms=saved['identity']['duration'],harmonic=saved['identity']['harmonic']) for x in saved['native'])
    key=lambda r:tuple(str(r[k]) for k in ('source','group_id','clip','condition','bitrate','frame_ms','harmonic','pattern','implementation'))
    assert len({key(x) for x in rows})==len(rows)==len(expected)*3*6*6
    rows.sort(key=key)
    for name,data in [('waveforms.csv',rows),('native-objectives.csv',native)]:
        with (a.output/name).open('w') as stream:
            fields=list(dict.fromkeys(k for row in data for k in row));w=csv.DictWriter(stream,fieldnames=fields);w.writeheader();w.writerows(data)
    protocol=dict(common,task_shard=[0,1],merged_task_shards=[x['task_shard'] for x in protocols])
    (a.output/'protocol.json').write_text(json.dumps(protocol,indent=2)+'\n')
    (a.output/'status.json').write_text(json.dumps(dict(status='COMPLETE',tasks=len(expected),rows=len(rows),cases_per_variant=len(rows)//6,repeated_jobs_native_exact=repeat,repeated_quality_max_abs_delta=repeat_quality_max_delta))+'\n')
    print('Merged',len(expected),'tasks;',len(rows)//6,'cases per variant;',repeat,'repeated native results match exactly; quality delta',repeat_quality_max_delta)


if __name__=='__main__':main()
