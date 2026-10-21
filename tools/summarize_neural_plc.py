#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Apply the pre-training waveform gates and group-bootstrap paired loss errors."""
import argparse
import csv
import hashlib
import json
import math
from pathlib import Path
import numpy as np


def mean(rows,key):
    values=[float(row[key]) for row in rows if row.get(key) not in (None,"")]
    return float(np.mean(values)) if values else None


def main():
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument("csv",type=Path)
    parser.add_argument("--protocol",type=Path,default=Path("docs/neural-plc-acceptance.json"))
    parser.add_argument("--output",type=Path,required=True)
    args=parser.parse_args()
    if json.loads(args.csv.with_suffix(".json").read_text()).get("status")!="COMPLETE":
        parser.error("evaluation is incomplete")
    protocol=json.loads(args.protocol.read_text()); gates=protocol["quality"]
    with args.csv.open() as stream: rows=list(csv.DictReader(stream))
    report=dict(rows=len(rows),csv_sha256=hashlib.sha256(args.csv.read_bytes()).hexdigest(),
                protocol_sha256=hashlib.sha256(args.protocol.read_bytes()).hexdigest(),
                failures=[],by_language=[],by_language_mode_duration=[])
    for language in sorted({row["language"] for row in rows}):
        selected=[r for r in rows if r["language"]==language]
        baseline=sum(float(r["baseline_lost_error_energy"]) for r in selected)
        current=sum(float(r["current_lost_error_energy"]) for r in selected)
        gain=10*math.log10(baseline/current)
        groups={}
        for row in selected:
            key=(row["source"],row["group_id"])
            entry=groups.setdefault(key,[0.0,0.0])
            entry[0]+=float(row["baseline_lost_error_energy"])
            entry[1]+=float(row["current_lost_error_energy"])
        matrix=np.asarray(list(groups.values())); rng=np.random.default_rng(6716)
        samples=matrix[rng.integers(0,len(matrix),size=(2000,len(matrix)))].sum(axis=1)
        bootstrap=10*np.log10(samples[:,0]/samples[:,1])
        entry=dict(language=language,groups=len(groups),aggregate_lost_snr_gain_db=gain,
                   group_bootstrap_95pct_db=np.quantile(bootstrap,[0.025,0.975]).tolist())
        report["by_language"].append(entry)
        if gain<gates["aggregate_lost_snr_gain_db_min_by_language"]:report["failures"].append(f"{language} lost SNR gain {gain:.4f} dB")
    for key in sorted({(r["language"],r["harmonic"],r["frame_ms"]) for r in rows}):
        selected=[r for r in rows if (r["language"],r["harmonic"],r["frame_ms"])==key]
        entry=dict(language=key[0],harmonic=key[1],frame_ms=int(key[2]))
        for metric,minimum in [("stoi",gates["mean_stoi_delta_min_by_language_mode_duration"]),("estoi",gates["mean_estoi_delta_min_by_language_mode_duration"])]:
            value=mean(selected,"delta_"+metric);entry["mean_delta_"+metric]=value
            if value is None or value<minimum:report["failures"].append(f"{key} {metric} delta {value}")
        value=mean(selected,"delta_lsd_db");entry["mean_delta_lsd_db"]=value
        if value is None or value>gates["mean_lsd_delta_db_max_by_language_mode_duration"]:report["failures"].append(f"{key} LSD delta {value}")
        report["by_language_mode_duration"].append(entry)
    for metric,minimum in [("onset",gates["mean_onset_snr_delta_db_min"]),("recovery",gates["mean_recovery_snr_delta_db_min"])]:
        value=mean(rows,"delta_"+metric+"_snr_db");report["mean_delta_"+metric+"_snr_db"]=value
        if value is None or value<minimum:report["failures"].append(f"{metric} SNR delta {value}")
    deltas=sorted(float(r["delta_lost_snr_db"]) for r in rows if r.get("delta_lost_snr_db") not in (None,""))
    worst=float(np.mean(deltas[:max(1,math.ceil(len(deltas)*0.1))]))
    report["worst_decile_mean_lost_snr_delta_db"]=worst
    if worst<gates["worst_decile_mean_lost_snr_delta_db_min"]:report["failures"].append(f"worst decile lost SNR delta {worst}")
    report["status"]="PASS" if not report["failures"] else "FAIL"
    args.output.write_text(json.dumps(report,indent=2)+"\n")
    print(json.dumps(report,indent=2))
    raise SystemExit(0 if report["status"]=="PASS" else 1)


if __name__=="__main__":main()
