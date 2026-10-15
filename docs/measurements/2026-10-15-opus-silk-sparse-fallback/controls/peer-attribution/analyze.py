"""Frozen-packet numerical attribution only; no encoder or perceptual scoring."""
import csv
import ctypes as C
import hashlib
import json
from pathlib import Path
import struct
import numpy as np

HERE=Path(__file__).resolve().parent
ARTIFACTS=HERE.parent.parent
INPUT=ARTIFACTS/'refusal-sparse'
opus=C.CDLL('/opt/homebrew/lib/libopus.dylib')
opus.opus_get_version_string.restype=C.c_char_p
opus.opus_decoder_create.argtypes=[C.c_int,C.c_int,C.POINTER(C.c_int)]
opus.opus_decoder_create.restype=C.c_void_p
opus.opus_decoder_destroy.argtypes=[C.c_void_p]
opus.opus_decode_float.argtypes=[C.c_void_p,C.c_void_p,C.c_int,C.POINTER(C.c_float),C.c_int,C.c_int]
opus.opus_decode_float.restype=C.c_int
opus.opus_decoder_ctl.argtypes=[C.c_void_p,C.c_int]
opus.opus_decoder_ctl.restype=C.c_int

def read_packets(path):
    data=path.read_bytes();offset=0;result=[]
    while offset<len(data):
        n,=struct.unpack_from('<I',data,offset);offset+=4
        result.append(data[offset:offset+n]);offset+=n
    assert offset==len(data)
    return result
def read_pcm(path): return np.fromfile(path,dtype='<f4').astype(np.float64)
def statistics(a):
    return {'rms':float(np.sqrt(np.mean(a*a))),'peak_abs':float(np.max(np.abs(a))),'min':float(np.min(a)),'max':float(np.max(a))}
def error(a,b):
    e=a-b
    return {'samples':len(e),'max_abs':float(np.max(np.abs(e))) if len(e) else None,'rmse':float(np.sqrt(np.mean(e*e))) if len(e) else None,'mean':float(np.mean(e)) if len(e) else None}
def best_lag(a,b,maximum):
    # Every candidate uses the same owned sample interval; positive lag compares
    # own[t] with native[t+lag]. Ties prefer the smaller absolute shift.
    order=sorted(range(-maximum,maximum+1),key=lambda x:(abs(x),x))
    start=maximum;end=min(len(a),len(b))-maximum
    best=min(order,key=lambda lag:np.mean((a[start:end]-b[start+lag:end+lag])**2))
    return {'lag':best,'search_max':maximum,'fixed_samples':end-start,'metrics':error(a[start:end],b[start+best:end+best])}
def decode_native(label,rate,packets,owned_trace):
    e=C.c_int();decoder=opus.opus_decoder_create(rate,1,C.byref(e));assert decoder and e.value==0
    chunks=[]
    try:
        for packet,trace in zip(packets,owned_trace):
            output=(C.c_float*(rate*120//1000))();storage=C.create_string_buffer(packet)
            samples=opus.opus_decode_float(decoder,storage,len(packet),output,len(output),0)
            assert samples==trace['samples']
            interval=C.c_uint32();assert opus.opus_decoder_ctl(decoder,4031,C.byref(interval))==0
            assert interval.value==trace['range']
            chunk=np.ctypeslib.as_array(output)[:samples].copy();assert np.all(np.isfinite(chunk));chunks.append(chunk)
    finally: opus.opus_decoder_destroy(decoder)
    data=np.concatenate(chunks);data.astype('<f4').tofile(HERE/f'{label}.native-{rate}.f32')
    return data.astype(np.float64)

report={'candidate_commit':'8f723ba1f35c053e1cc840f38a4b20446a127dde','libopus':opus.opus_get_version_string().decode(),'scope':'Same frozen packets, native/48k decoder numerical diagnostics only; no encoding, tuning, perceptual scoring or fresh corpus','alignment_convention':'Compare own[t] with native[t+lag]; fixed common interior for every candidate lag; zero-shift metrics retain all samples','profiles':{}}
rows=[]
for label,native_rate,packet_ms in [('nb',8000,10),('wb',16000,20)]:
    packets=read_packets(INPUT/f'{label}.packets')
    expected_ranges=np.fromfile(INPUT/f'{label}.ranges',dtype='<u4')
    result={'packets':len(packets),'output_rates':{}}
    for rate in [native_rate,48000]:
        traces=[json.loads(line) for line in (HERE/f'{label}.owned-{rate}.jsonl').read_text().splitlines()]
        assert len(packets)==len(traces)==len(expected_ranges)
        assert all(t['range']==int(r) for t,r in zip(traces,expected_ranges))
        own=read_pcm(HERE/f'{label}.owned-{rate}.f32')
        native=decode_native(label,rate,packets,traces)
        assert len(own)==len(native)==sum(t['samples'] for t in traces)
        aligned=best_lag(own,native,24 if rate==native_rate else 144)
        maximum_index=int(np.argmax(np.abs(own-native)))
        q={'samples':len(own),'owned':statistics(own),'native':statistics(native),'zero_shift':error(own,native),'best_alignment':aligned,'max_error_location':{'sample':maximum_index,'packet':maximum_index//(rate*packet_ms//1000),'owned':float(own[maximum_index]),'native':float(native[maximum_index])},'exact_ranges_counts':True}
        clipped=np.clip(own,-1.0,1.0);mask=np.abs(own)>1.0;overshoot=own-clipped
        q['clamp_counterfactual']={'overshoot_samples':int(np.sum(mask)),'overshoot_fraction':float(np.mean(mask)),'clamp_own_only':error(clipped,native),'raw_error_on_overshoot':error(own[mask],native[mask]),'raw_error_elsewhere':error(own[~mask],native[~mask]),'overshoot_error_energy_fraction':float(np.sum((own[mask]-native[mask])**2)/np.sum((own-native)**2)) if np.any(own!=native) else 0.0,'overshoot_component_rms':float(np.sqrt(np.mean(overshoot**2)))}
        if rate==48000:
            q['existing_48k_owned_exact']=np.array_equal(own,read_pcm(INPUT/f'{label}.owned.f32'))
            q['existing_48k_native_exact']=np.array_equal(native,read_pcm(INPUT/f'{label}.native.f32'))
            assert q['existing_48k_owned_exact'] and q['existing_48k_native_exact']
        offset=0
        for index,t in enumerate(traces):
            end=offset+t['samples'];a=own[offset:end];b=native[offset:end];m=error(a,b);cm=error(np.clip(a,-1,1),b)
            row={'profile':label,'rate':rate,'packet':index,'samples':len(a),'owned_rms':statistics(a)['rms'],'native_rms':statistics(b)['rms'],'owned_peak':statistics(a)['peak_abs'],'native_peak':statistics(b)['peak_abs'],'max_abs_error':m['max_abs'],'rmse':m['rmse'],'clamped_max_abs_error':cm['max_abs'],'clamped_rmse':cm['rmse'],'overshoot_samples':int(np.sum(np.abs(a)>1))}
            rows.append(row);offset=end
        result['output_rates'][str(rate)]=q
    report['profiles'][label]=result
with (HERE/'per-packet.csv').open('w') as f:
    writer=csv.DictWriter(f,fieldnames=list(rows[0]));writer.writeheader();writer.writerows(rows)
source_files=[INPUT/f'{label}.{suffix}' for label in ['nb','wb'] for suffix in ['packets','ranges','owned.f32','native.f32']]
source_files += [ARTIFACTS/'libsnacka_sparse.rlib',HERE/'decode.rs',HERE/'analyze.py']
report['input_sha256']={str(path):hashlib.sha256(path.read_bytes()).hexdigest() for path in source_files}
recorded=json.loads((HERE.parent/'source-hashes.json').read_text())
recorded_hashes={str(Path(path).resolve()):digest for path,digest in recorded['sha256'].items()}
assert report['input_sha256'][str(ARTIFACTS/'libsnacka_sparse.rlib')]==recorded_hashes[str(ARTIFACTS/'libsnacka_sparse.rlib')]
(HERE/'summary.json').write_text(json.dumps(report,indent=2)+'\n')
for label,p in report['profiles'].items():
    for rate,q in p['output_rates'].items():
        print(json.dumps({'profile':label,'rate':rate,'raw':q['zero_shift'],'owned_peak':q['owned']['peak_abs'],'native_peak':q['native']['peak_abs'],'best_lag':q['best_alignment']['lag'],'aligned':q['best_alignment']['metrics'],'clamped':q['clamp_counterfactual']['clamp_own_only'],'overshoot_samples':q['clamp_counterfactual']['overshoot_samples'],'overshoot_error_energy_fraction':q['clamp_counterfactual']['overshoot_error_energy_fraction']}))
