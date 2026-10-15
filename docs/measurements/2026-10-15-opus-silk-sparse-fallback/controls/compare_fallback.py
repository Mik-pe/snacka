import ctypes as C
import hashlib
import json
import math
from pathlib import Path
import struct
import sys

base,candidate=(Path(x) for x in sys.argv[1:3])
root=Path(__file__).resolve().parent
opus=C.CDLL('/opt/homebrew/lib/libopus.dylib')
opus.opus_get_version_string.restype=C.c_char_p
opus.opus_decoder_create.argtypes=[C.c_int,C.c_int,C.POINTER(C.c_int)]
opus.opus_decoder_create.restype=C.c_void_p
opus.opus_decoder_destroy.argtypes=[C.c_void_p]
opus.opus_decode_float.argtypes=[C.c_void_p,C.c_void_p,C.c_int,C.POINTER(C.c_float),C.c_int,C.c_int]
opus.opus_decode_float.restype=C.c_int
opus.opus_decoder_ctl.argtypes=[C.c_void_p,C.c_int]
opus.opus_decoder_ctl.restype=C.c_int
def packets(path):
    data=path.read_bytes();offset=0;result=[]
    while offset<len(data):
        header=struct.unpack_from('<5I',data,offset);offset+=20
        packet=data[offset:offset+header[0]];offset+=header[0]
        result.append((header,packet))
    assert offset==len(data)
    return result
def pcm(path):
    data=path.read_bytes();offset=0;result=[]
    while offset<len(data):
        n,=struct.unpack_from('<I',data,offset);offset+=4
        result.append(data[offset:offset+n*4]);offset+=n*4
    assert offset==len(data)
    return result
def new_decoder(channels):
    error=C.c_int();decoder=opus.opus_decoder_create(48000,channels,C.byref(error));assert decoder and error.value==0
    return decoder
def decode(decoder,packet,samples,channels,fec=False,interval=None):
    output=(C.c_float*(samples*channels))();storage=C.create_string_buffer(packet)
    assert opus.opus_decode_float(decoder,storage,len(packet),output,samples,int(fec))==samples
    assert all(math.isfinite(x) for x in output)
    if interval is not None:
        actual=C.c_uint32();assert opus.opus_decoder_ctl(decoder,4031,C.byref(actual))==0
        assert actual.value==interval,(hex(actual.value),hex(interval))
def native(summary,records):
    regular=new_decoder(summary['channels']);recovery=new_decoder(summary['channels']);fec_recovered=0
    try:
        for index,(header,packet) in enumerate(records):
            _,interval,samples,flags,channels=header
            decode(regular,packet,samples,channels,interval=interval)
            if summary['carrier'] and index==5: continue
            if summary['carrier'] and index==6:
                decode(recovery,packet,samples,channels,fec=True)
                fec_recovered+=int(bool(flags&2))
            decode(recovery,packet,samples,channels,interval=interval)
    finally:
        opus.opus_decoder_destroy(regular);opus.opus_decoder_destroy(recovery)
    return fec_recovered
b={x['name']:x for x in map(json.loads,(base/'streams.jsonl').read_text().splitlines())}
c={x['name']:x for x in map(json.loads,(candidate/'streams.jsonl').read_text().splitlines())}
assert b.keys()==c.keys()
report={'libopus':opus.opus_get_version_string().decode(),'streams':[]}
for name,current in c.items():
    baseline=b[name]
    a=packets(base/(name+'.packets'));z=packets(candidate/(name+'.packets'))
    ap=pcm(base/(name+'.pcm'));zp=pcm(candidate/(name+'.pcm'))
    assert len(a)==baseline['accepted'] and len(z)==current['accepted']
    assert len(z)>=len(a),(name,baseline,current)
    assert a==z[:len(a)],(name,'successful packet/range/flag prefix changed')
    assert ap==zp[:len(ap)],(name,'successful decoded PCM prefix changed')
    if baseline['failed_at'] is None: assert len(a)==len(z)
    fec=native(current,z)
    row={'name':name,'baseline':baseline,'candidate':current,'exact_prefix_packets':len(a),'baseline_full_success':baseline['failed_at'] is None,'rescued':len(z)>len(a),'native_packets':len(z),'native_fec_recoveries':fec,'packets_sha256':hashlib.sha256((candidate/(name+'.packets')).read_bytes()).hexdigest()}
    report['streams'].append(row)
report.update({'profiles':len(c),'baseline_full_success_streams':sum(x['baseline_full_success'] for x in report['streams']),'rescued_streams':sum(x['rescued'] for x in report['streams']),'exact_prefix_packets':sum(x['exact_prefix_packets'] for x in report['streams']),'native_packets':sum(x['native_packets'] for x in report['streams']),'native_fec_recoveries':sum(x['native_fec_recoveries'] for x in report['streams']),'candidate_refusals':[{k:x['candidate'][k] for k in ['name','accepted','failed_at','error']} for x in report['streams'] if x['candidate']['failed_at'] is not None]})
(root/'fallback-comparison.json').write_text(json.dumps(report,indent=2)+'\n')
print(json.dumps({k:v for k,v in report.items() if k!='streams'},indent=2))
