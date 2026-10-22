#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Merge complete mode partitions and apply the frozen independent quality gate.

The language/mode/duration ESTOI strata follow the earlier Step2 convention.
Only one validation-selected candidate is evaluated; rejection has no fallback.
Runtime, allocation, production parity and resource receipts remain separate.
"""
import argparse
import csv
import hashlib
import json
from pathlib import Path
import numpy as np
from select_plc_causal import canonical_case,canonical_mode,summarize


def stratum_guards(rows,floor):
    strata=[]
    for language in ('en','sv'):
        for mode in ('0','1'):
            for duration in ('10','20'):
                values=[float(row['delta_estoi']) for row in rows if (row['language'],canonical_mode(row['harmonic']),row['frame_ms'])==(language,mode,duration)]
                if not values or not np.isfinite(values).all():raise ValueError('missing or nonfinite acceptance stratum')
                score=float(np.mean(values));strata.append(dict(language=language,harmonic=int(mode),frame_ms=int(duration),cases=len(values),estoi_delta=score,passed=score>=floor))
    return strata


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('selection',type=Path);p.add_argument('decision',type=Path)
    p.add_argument('output',type=Path);p.add_argument('--parts',type=Path,nargs='+',required=True)
    p.add_argument('--protocol',type=Path,required=True);p.add_argument('--capture-receipt',type=Path,required=True);a=p.parse_args()
    csv_path=a.output.with_name(a.output.stem+'-quality.csv')
    if a.output.exists() or csv_path.exists():p.error('immutable output requires a fresh path')
    decision=json.loads(a.decision.read_text());protocol=json.loads(a.protocol.read_text());capture=json.loads(a.capture_receipt.read_text())
    decision_sha=hashlib.sha256(a.decision.read_bytes()).hexdigest()
    if decision['selected']!='distill_phase' or capture['production_receipt']['selection_sha256']!=decision_sha:raise ValueError('candidate differs from frozen validation selection')
    expected_binary=capture['binaries']['plc_ablation_compare'];selection_sha=hashlib.sha256(a.selection.read_bytes()).hexdigest()
    rows=[];sources=[];fields=None;probe=None
    for path in a.parts:
        meta=json.loads(path.with_suffix('.json').read_text());digest=hashlib.sha256(path.read_bytes()).hexdigest()
        if meta['status']!='COMPLETE' or meta['selection_sha256']!=selection_sha or meta['quality_sha256']!=digest or meta['binary_sha256']!=expected_binary or not meta['frozen_trace_checked']:raise ValueError('incomplete, changed, or unbound native acceptance partition')
        if probe is not None and probe!=meta['baseline_probe_sha256']:raise ValueError('baseline executable differs across partitions')
        probe=meta['baseline_probe_sha256']
        with path.open() as f:
            reader=csv.DictReader(f);part=list(reader)
            if fields is not None and fields!=reader.fieldnames:raise ValueError('partition schemas differ')
            fields=reader.fieldnames
        if len(part)!=meta['rows']:raise ValueError('partition row count differs')
        rows.extend(part);sources.append(dict(sha256=digest,rows=len(part),metadata_sha256=hashlib.sha256(path.with_suffix('.json').read_bytes()).hexdigest()))
    if len({canonical_case(row) for row in rows})!=len(rows):raise ValueError('duplicate acceptance cases')
    rows.sort(key=canonical_case)
    with csv_path.open('w') as f:
        writer=csv.DictWriter(f,fieldnames=fields);writer.writeheader();writer.writerows(rows)
    metadata=csv_path.with_suffix('.json')
    # summarize() requires the native completeness sidecar. Keep it beside the
    # merged CSV; the final decision is a distinct -report.json file.
    metadata.write_text(json.dumps(dict(status='COMPLETE',rows=len(rows),selection_sha256=selection_sha,binary_sha256=expected_binary,parts=sources),indent=2)+'\n')
    settings=dict(protocol['validation']);settings['bitrates']=protocol['acceptance']['bitrates']
    result=summarize(csv_path,a.selection,settings)
    strata=stratum_guards(rows,protocol['acceptance']['per_stratum_estoi_floor'])
    result.update(schema=1,selected=decision['selected'],validation_decision_sha256=decision_sha,
        protocol_sha256=hashlib.sha256(a.protocol.read_bytes()).hexdigest(),cases=len(rows),parts=sources,
        baseline_probe_sha256=probe,frozen_packet_and_uninterrupted_traces_checked=True,
        per_language_mode_duration=strata,quality_passed=result['admission']['passed'] and all(x['passed'] for x in strata),
        runtime_status='separate allocation, M5 CPU and production replay receipts required',
        no_fallback=True)
    a.output.write_text(json.dumps(result,indent=2,allow_nan=False)+'\n');print(json.dumps(result,indent=2))


if __name__=='__main__':main()
