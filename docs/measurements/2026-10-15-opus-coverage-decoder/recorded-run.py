# SPDX-License-Identifier: MIT
# Recorded macOS run; use an artifact directory outside the checkout.
from pathlib import Path
import subprocess,json,statistics,hashlib,platform,os
os.environ["SNACKA_BENCH_QOS"]="user-initiated"

BASE=Path(os.environ["SNACKA_BENCH_ARTIFACTS"]).resolve()
OUT=BASE/'accepted-final'
OUT.mkdir(exist_ok=True)
CASES=[('after', 'SILK', 'WB', '20', 1, 8000, 'decode'), ('after', 'SILK', 'WB', '20', 1, 8000, 'fec-return'), ('after', 'SILK', 'WB', '20', 2, 8000, 'decode'), ('after', 'SILK', 'WB', '10', 1, 8000, 'plc-return'), ('after', 'SILK', 'WB', '60', 2, 8000, 'decode'), ('after', 'SILK', 'NB', '20', 1, 48000, 'decode'), ('after', 'CELT', 'FB', '2.5', 1, 48000, 'decode'), ('after', 'CELT', 'WB', '10', 1, 24000, 'decode'), ('after', 'CELT', 'FB', '20', 2, 48000, 'decode'), ('after', 'CELT', 'FB', '20', 2, 48000, 'plc-return'), ('after', 'Hybrid', 'FB', '20', 2, 48000, 'decode')]
rows=[]
for number,(variant,mode,band,ms,ch,rate,schedule) in enumerate(CASES):
    name=f'{variant}-{mode}-{band}-{ms}-{ch}-{rate}-{schedule}'
    packet=BASE/'baseline-matrix'/f'{mode}-{band}-{ms}ms-{ch}ch.bit'
    repeats=[]
    for repetition in range(3):
        pair={}
        for role in (['baseline',variant] if repetition%2==0 else [variant,'baseline']):
            cmd=[str(BASE/('clean-before-decode' if role=='baseline' else 'clean-after-decode')),'matrix',str(rate),str(ch),schedule,'1000','200000',str(packet)]
            result=subprocess.run(cmd,check=True,capture_output=True,text=True)
            (OUT/f'{name}-{repetition}-{role}.json').write_text(result.stdout)
            pair[role]=json.loads(result.stdout)
            assert pair[role]['all_batches_meet_minimum'], name
        assert pair['baseline']['owned_pcm_hash']==pair[variant]['owned_pcm_hash'], name
        a=pair['baseline']['p50_ns_per_operation'];b=pair[variant]['p50_ns_per_operation']
        repeats.append({'change':b['ratio']/a['ratio']-1,'baseline':a['ratio'],'current':b['ratio'],
          'p95':pair[variant]['p95_ns_per_operation']['ratio'],'p99':pair[variant]['p99_ns_per_operation']['ratio']})
    row={'case':name,'variant':variant,'repetitions':repeats,
         **{key:statistics.median(r[key] for r in repeats) for key in repeats[0]}}
    rows.append(row)
    (OUT/'summary.json').write_text(json.dumps(rows,indent=2)+'\n')
    print(f'{number+1}/{len(CASES)} {name}: {row["change"]:+.2%}; vs libopus {row["baseline"]:.3f} -> {row["current"]:.3f}',flush=True)
manifest={'host':platform.platform(),'rustc':json.loads((BASE/'clean/source.json').read_text())['rustc'],
 'binaries':{p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in BASE.glob('*-decode')},
 'scope':'Selected 11 complete-decoder cases, 3 reversed-role repetitions, 1000 alternating batches per executable, >=200us; unrelated host activity uncontrolled; normalized against interleaved libopus 1.6.1; cached generated streams; not callback or quality parity',
 'baseline_commit':'87062cd','build_manifest':'../clean/source.json','macos_qos_requested':'user-initiated','script_sha256':hashlib.sha256(Path(__file__).read_bytes()).hexdigest()}
(OUT/'source.json').write_text(json.dumps(manifest,indent=2)+'\n')
