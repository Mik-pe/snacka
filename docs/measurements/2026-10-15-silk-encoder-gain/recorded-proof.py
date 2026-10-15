#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
from pathlib import Path
import argparse, hashlib, json, math, os, subprocess

parser=argparse.ArgumentParser(description="Repeat exact owned before/after traces; no timing or quality scores.")
parser.add_argument('--artifacts',default=os.environ.get('SNACKA_BENCH_ARTIFACTS'))
parser.add_argument('--corpus',default=os.environ.get('SNACKA_SPEECH_CORPUS'),help='existing AN4 corpus folder with manifest.json; only development clips are read')
args=parser.parse_args()
if not args.artifacts:
    parser.error('set --artifacts or SNACKA_BENCH_ARTIFACTS to a recorded-build.py result')
root=Path(args.artifacts).resolve()
out=root/'equivalence'
out.mkdir(exist_ok=False)
(out/'raw').mkdir()
def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
source=json.loads((root/'source.json').read_text())
source_digest=sha(root/'source.json')
runner_digest=sha(Path(__file__))
source['proof_probe']={'path':source['proof_source'],
                       'sha256':sha(Path(source['proof_source'])),
                       'binaries':{role:sha(root/f'probe-{role}') for role in ['before','after']}}
cases=[]
durations=[('2.5',120),('5',240),('10',480),('20',960),('40',1920),('60',2880),('80',3840),('100',4800),('120',5760)]
for mode,bands in [('silk',['NB','MB','WB']),('hybrid',['SWB','FB']),('celt',['NB','WB','SWB','FB'])]:
    for ms,samples in durations:
        if mode!='celt' and samples<480:
            continue
        for band in bands:
            for channels in [1,2]:
                for signal in ['voice','noise','transient','silence','clipping','zeros']:
                    cases.append({'scope':'generated','mode':mode,'band':band,'ms':ms,
                                  'channels':channels,'signal':signal,'frames':96})
assert len(cases)==142*6
corpus=Path(args.corpus).resolve() if args.corpus else None
corpus_manifest=json.loads((corpus/'manifest.json').read_text()) if corpus else {'clips':[]}
clips=[]
dev_geometries=[('silk','NB','10',480,2,24000),('silk','MB','60',2880,1,16000),
                ('silk','WB','20',960,1,8000),('silk','WB','120',5760,2,48000),
                ('hybrid','SWB','10',480,2,64000),('hybrid','FB','40',1920,1,32000),
                ('hybrid','FB','120',5760,2,64000),('celt','FB','2.5',120,2,64000),
                ('celt','FB','20',960,1,32000)]
for clip in corpus_manifest['clips']:
    if clip['split']!='development':
        continue
    wav=corpus/clip['clip']
    if not wav.exists():
        wav=corpus/clip['path']
    assert sha(wav)==clip['sha256']
    raw=out/(Path(clip['clip']).stem+'.f32')
    command=['/opt/homebrew/bin/ffmpeg','-v','error','-y','-i',str(wav),'-ar','48000','-ac','1','-f','f32le',str(raw)]
    subprocess.run(command,check=True)
    n=raw.stat().st_size//4
    clips.append({'clip':clip['clip'],'wav_sha256':sha(wav),'raw_sha256':sha(raw),'samples_48000':n,'conversion':command})
    for mode,band,ms,samples,channels,bitrate in dev_geometries:
        cases.append({'scope':'development','clip':clip['clip'],'mode':mode,'band':band,'ms':ms,
                      'channels':channels,'signal':'file','frames':math.ceil(n/samples)+1,
                      'input':str(raw),'bitrate':bitrate})
assert corpus is None or len(clips)==8
rows=[]
totals={key:0 for key in ['encoded_packets','decoded_intervals','actual_fec_recoveries','fallbacks','traces','encoder_refusals','traces_with_refusals']}
with (out/'rows.jsonl').open('w') as sink:
    for index,case in enumerate(cases):
        args=[case['mode'],case['band'],case['ms'],str(case['channels']),case['signal'],str(case['frames'])]
        if 'input' in case:
            args+=['--input',case['input'],'--bitrate',str(case['bitrate'])]
        results={}
        for role in ['before','after']:
            command=[str(root/f'probe-{role}'),*args]
            done=subprocess.run(command,text=True,capture_output=True)
            (out/'raw'/f'{index:04d}-{role}.stdout').write_text(done.stdout)
            (out/'raw'/f'{index:04d}-{role}.stderr').write_text(done.stderr)
            if done.returncode:
                raise RuntimeError(f'{role} case {case}: {done.stderr}')
            results[role]=json.loads(done.stdout)
        assert results['before']==results['after'],case
        result=results['after']
        row={'case':case,'exact':True,'result':result,
             'result_sha256':hashlib.sha256(json.dumps(result,sort_keys=True).encode()).hexdigest()}
        for trace in result['traces']:
            data=trace['trace']
            totals['traces']+=1
            totals['encoded_packets']+=case['frames']
            totals['encoder_refusals']+=data['encoder_refusals']
            totals['traces_with_refusals']+=int(data['encoder_refusals']!=0)
            totals['actual_fec_recoveries']+=data['fec_return']['recovered_fec_packets']
            totals['fallbacks']+=data['fec_return']['recovery_fallbacks']
            for schedule in ['normal','plc_return','fec_return']:
                totals['decoded_intervals']+=data[schedule]['events']
        sink.write(json.dumps(row,separators=(',',':'))+'\n')
        sink.flush()
        if index%50==0:
            print(index+1,len(cases),case,flush=True)
assert sha(root/'source.json')==source_digest
assert sha(Path(__file__))==runner_digest
for role,row in source['variants'].items():
    for relative,expected in row['sources'].items():
        assert sha(Path(row['source'])/relative)==expected,(role,relative)
    assert sha(Path(row['target'])/'release/libsnacka.rlib')==row['library_sha256']
    assert sha(root/f'probe-{role}')==row['probe_sha256']
assert sha(Path(source['proof_source']))==source['proof_source_sha256']
summary={'base_commit':source['base_commit'],'changed_files':source['changed_files'],
         'source_sha256':source_digest,'probe':source['proof_probe'],
         'runner_sha256':runner_digest,'cases':len(cases),'generated_geometries':142,
         'generated_signals':6,'generated_cases':142*6,'development_cases':len(cases)-142*6,
         'development_clips':clips,'totals_per_variant':totals,'all_exact':True,
         'raw_rows_sha256':sha(out/'rows.jsonl'),
         'scope':f'CBR48k input; normal/FEC encoder modes, continuous/reset halves; ordinary,PLC-return,FEC-return output. Full142 generated geometries plus {len(clips)} development clips in9 selected geometries. Development inputs include a wrapped prefix; evaluation clips excluded.'}
(out/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps({key:value for key,value in summary.items() if key!='development_clips'},indent=2),flush=True)
