from pathlib import Path
import ctypes as c,json,math,struct,sys,hashlib
A=Path('/tmp/snacka-opus-silk-sparse-fallback-artifacts-20261015')
sys.path.insert(0,'/tmp/snacka-opus-silk-sparse-fallback-20261015/tools/reference')
from check_opus_rtp import library
L=library(Path('/opt/homebrew/lib/libopus.dylib'))
def floats(raw):return struct.unpack('<%df'%(len(raw)//4),raw)
def decode(raw,samples,expected=None):
 e=c.c_int();d=L.opus_decoder_create(48000,1,c.byref(e));assert d and not e.value
 output=[];tocs=set();sizes=set();ranges=[];i=0
 while raw:
  n=struct.unpack('<I',raw[:4])[0];p=raw[4:4+n];raw=raw[4+n:];assert len(p)==n
  b=(c.c_ubyte*n).from_buffer_copy(p);pcm=(c.c_float*samples)();got=L.opus_decode_float(d,b,n,pcm,samples,0);assert got==samples
  assert all(math.isfinite(v) for v in pcm)
  r=c.c_uint();assert L.opus_decoder_ctl(d,4031,c.byref(r))==0
  if expected is not None:assert r.value==expected[i],(i,r.value,expected[i])
  output.extend(pcm);tocs.add(p[0]);sizes.add(n);ranges.append(r.value);i+=1
 L.opus_decoder_destroy(d)
 return output,{'packets':i,'actual_toc_bytes':sorted(tocs),'packet_lengths':sorted(sizes),'finite_pcm':True,'exact_encoder_final_ranges':expected is not None,'range_sha256':hashlib.sha256(struct.pack('<%dI'%len(ranges),*ranges)).hexdigest()}
def metrics(x,y):
 start=1920;end=min(len(x),len(y))-1920-1024
 def mse(lag,step):return sum((x[i]-y[i+lag])**2 for i in range(start,end,step))/len(range(start,end,step))
 coarse=min(range(0,1025,8),key=lambda d:mse(d,8));lag=min(range(max(0,coarse-8),min(1024,coarse+8)+1),key=lambda d:mse(d,1))
 energy=sum(x[i]**2 for i in range(start,end))/(end-start)
 return {'best_delay_48k_samples':lag,'aligned_unscaled_snr_db':10*math.log10(energy/mse(lag,1)),'input_rms':math.sqrt(sum(v*v for v in x)/len(x)),'output_rms':math.sqrt(sum(v*v for v in y)/len(y)),'output_peak':max(map(abs,y)),'trim_samples':1920,'delay_search_max_samples':1024}
result={'source_commit':'8f723ba1f35c053e1cc840f38a4b20446a127dde','libopus':L.opus_get_version_string().decode(),'scope':'Original two development refusals; native wire/finite/range proof and unscaled reconstruction diagnostics, not fresh perceptual acceptance','profiles':{}}
for label,samples,rate,bw in [('nb',480,12000,1101),('wb',960,6000,1103)]:
 d=A/'refusal-sparse';raw=(d/f'{label}.packets').read_bytes();old=(A/'refusal-baseline'/f'{label}.packets').read_bytes();x=floats((d/f'{label}.input.f32').read_bytes());ranges=struct.unpack('<%dI'%((d/f'{label}.ranges').stat().st_size//4),(d/f'{label}.ranges').read_bytes())
 y,wire=decode(raw,samples,ranges);own=floats((d/f'{label}.owned.f32').read_bytes());assert len(own)==len(y)
 (d/f'{label}.native.f32').write_bytes(struct.pack('<%df'%len(y),*y))
 e=c.c_int();enc=L.opus_encoder_create(48000,1,2052,c.byref(e));assert enc and not e.value
 for request,value in {4002:rate,4006:0,4008:bw,4010:10,4012:0,4024:3001}.items():assert L.opus_encoder_ctl(enc,request,c.c_int(value))==0
 records=b''
 for first in range(0,len(x),samples):
  pcm=(c.c_float*samples)(*x[first:first+samples]);buf=(c.c_ubyte*15)();n=L.opus_encode_float(enc,pcm,samples,buf,15);assert n==15
  records+=struct.pack('<I',n)+bytes(buf[:n])
 L.opus_encoder_destroy(enc);ref,refwire=decode(records,samples)
 (d/f'{label}.native-reference.packets').write_bytes(records)
 result['profiles'][label]={'candidate':{**wire,'baseline_success_prefix_packets':len(old)//19,'baseline_success_prefix_exact':raw.startswith(old),'owned_native_max_abs_pcm_error':max(abs(a-b) for a,b in zip(own,y)),'packet_sha256':hashlib.sha256(raw).hexdigest(),**metrics(x,y)},'native_reference':{**refwire,**metrics(x,ref)}}
(A/'native-refusals.json').write_text(json.dumps(result,indent=2)+'\n');print(json.dumps(result,indent=2))
