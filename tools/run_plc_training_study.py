#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Run a validation-only convergence study without changing installed weights."""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import sys
import tarfile


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('features',type=Path)
    p.add_argument('baseline',type=Path,help='trusted original checkpoint')
    p.add_argument('protocol',type=Path)
    p.add_argument('output',type=Path)
    a=p.parse_args()
    if a.output.exists():p.error('use a new output directory')
    a.output.mkdir(parents=True)
    protocol=json.loads(a.protocol.read_text());results=[]
    for case in protocol['first_round']:
        name=case['name']
        if not name or Path(name).name!=name or name.startswith('.'):raise ValueError('invalid experiment name')
        out=a.output/name
        command=[sys.executable,str(Path(__file__).with_name('train_neural_plc.py')),str(a.features),str(out),
                 '--epochs',str(case['epochs']),'--learning-rate',str(case['lr']),'--schedule',case['schedule'],
                 '--checkpoint-epochs','120','200',str(case['epochs'])]
        if case.get('resume'):command+=['--resume',str(a.baseline)]
        if 'seed' in case:command+=['--seed',str(case['seed'])]
        with (a.output/(name+'.log')).open('w') as log:
            subprocess.run(command,stdout=log,stderr=subprocess.STDOUT,check=True)
        report=json.loads((out/'training-report.json').read_text())
        history=report['history'];last=history[-1]
        earlier=next((x for x in history if x['epoch']>=last['epoch']-50),history[0])
        improvement=(earlier['balanced_objective']-last['balanced_objective'])/earlier['balanced_objective']
        entry=dict(name=name,selected_epoch=report['selected_epoch'],validation=report['validation'],
                   last_50_epoch_objective_improvement_fraction=improvement,last_epoch=last,
                   report_sha256=hashlib.sha256((out/'training-report.json').read_bytes()).hexdigest())
        results.append(entry)
        (a.output/'summary.json').write_text(json.dumps(dict(protocol=protocol,experiments=results),indent=2)+'\n')
        print(name,'selected epoch',entry['selected_epoch'],'validation objective',report['validation']['balanced_objective'],flush=True)
    # Only result files enter this archive. Signed input URLs and raw features do not.
    archive=a.output.parent/'experiments.tar.gz'
    with tarfile.open(archive,'w:gz') as tar:tar.add(a.output,arcname='experiments')
    print('Study archive',archive.stat().st_size,hashlib.sha256(archive.read_bytes()).hexdigest(),flush=True)


if __name__=='__main__':main()
