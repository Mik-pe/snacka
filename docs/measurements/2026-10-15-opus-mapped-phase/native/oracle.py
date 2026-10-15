"""Public libopus phase-control oracle only. Existing CELT fixture; no encoder, timers or foreign bodies."""
import ctypes as C, pathlib, json, hashlib, struct, math
R=pathlib.Path(__file__).resolve().parent
pinned=json.loads((R/'manifest-before-run.json').read_text())
h=lambda p:hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
assert h(__file__)==pinned['oracle_sha256']
assert h(pinned['native']['path'])==pinned['native']['sha256']
assert h(R/'stereo-phase.opus')==pinned['fixture']['sha256']
lib=C.CDLL(pinned['native']['path'])
P=C.c_void_p;I=C.c_int;IP=C.POINTER(I);FP=C.POINTER(C.c_float)
lib.opus_get_version_string.restype=C.c_char_p
for prefix in ('opus','opus_multistream','opus_projection'):
 ctl=getattr(lib,prefix+'_decoder_ctl');ctl.argtypes=[P,I];ctl.restype=I
 destroy=getattr(lib,prefix+'_decoder_destroy');destroy.argtypes=[P];destroy.restype=None
 decode=getattr(lib,prefix+'_decode_float');decode.argtypes=[P,P,I,FP,I,I];decode.restype=I
lib.opus_decoder_create.argtypes=[I,I,IP];lib.opus_decoder_create.restype=P
lib.opus_multistream_decoder_create.argtypes=[I,I,I,I,P,IP];lib.opus_multistream_decoder_create.restype=P
lib.opus_projection_decoder_create.argtypes=[I,I,I,I,P,I,IP];lib.opus_projection_decoder_create.restype=P
SET,GET,RESET,CHILD,FINAL=4046,4047,4028,5122,4031
rows=[];errors=[]
def emit(row):
 rows.append(row);sink.write(json.dumps(row,separators=(',',':'))+'\n');sink.flush()
def create(family,rate,streams,coupled,mapping):
 error=I();channels=len(mapping)
 if family=='ordinary':state=lib.opus_decoder_create(rate,channels,C.byref(error))
 elif family=='mapped':
  b=(C.c_ubyte*channels)(*mapping)
  state=lib.opus_multistream_decoder_create(rate,channels,streams,coupled,b,C.byref(error))
 else:
  # C=K=4, column-major Q15: row0 averages the coupled pair;
  # row1 differences it, other rows retain coded mono channels.
  values=[16384,16384,0,0,16384,-16384,0,0,0,0,32767,0,0,0,0,32767]
  if (streams,coupled)!=(3,1):
   values=[32767 if r==c else 0 for c in range(4) for r in range(4)]
  b=struct.pack('<16h',*values);buf=C.create_string_buffer(b)
  state=lib.opus_projection_decoder_create(rate,4,streams,coupled,buf,len(b),C.byref(error));channels=4
 if not state or error.value:raise RuntimeError({'create_code':error.value,'family':family,'rate':rate,'streams':streams,'coupled':coupled})
 return state,channels
prefix={'ordinary':'opus','mapped':'opus_multistream','projection':'opus_projection'}
def ctl(family,state,request,*args):return getattr(lib,prefix[family]+'_decoder_ctl')(state,request,*args)
def phase(family,state):
 value=I(-999);code=ctl(family,state,GET,C.byref(value));return {'ctl_code':code,'disabled':value.value,'enabled':not bool(value.value) if code==0 else None}
def child(family,state,index):
 p=P();code=ctl(family,state,CHILD,I(index),C.byref(p))
 return code,p

def snapshot(family,state,streams):
 result={'aggregate':phase(family,state),'children':[]}
 if family!='ordinary':
  for i in range(streams):
   code,p=child(family,state,i)
   result['children'].append({'stream':i,'state_ctl_code':code,'phase':phase('ordinary',p) if code==0 and p else None})
 return result
fixture=(R/'stereo-phase.opus').read_bytes()
assert fixture[0]&3==0 and len(fixture)-1<252
packet=lambda n:(fixture[:1]+bytes([len(fixture)-1])+fixture[1:])*(n-1)+fixture
with (R/'events.jsonl').open('x') as sink:
 emit({'kind':'version','version':lib.opus_get_version_string().decode(),'scope':'native public controls and existing-fixture applicability; no timing/quality claim'})
 for rate in (8000,12000,16000,24000,48000):
  for family in ('mapped','projection'):
   layouts=[('mono',3,0,[0,1,2]),('mixed',3,1,[0,1,2,255,0]),('more-coupled',3,2,[0,1,2,3,4])] if family=='mapped' else [('mono',4,0,[0,1,2,3]),('mixed',3,1,[0,1,2,3]),('more-coupled',2,2,[0,1,2,3])]
   for label,streams,coupled,mapping in layouts:
    state=None;context={'kind':'controls','family':family,'layout':label,'rate':rate,'streams':streams,'coupled':coupled,'mapping':mapping}
    try:
     state,channels=create(family,rate,streams,coupled,mapping)
     emit(context|{'event':'fresh','status':snapshot(family,state,streams)})
     for enabled in (False,True):
      code=ctl(family,state,SET,I(not enabled));emit(context|{'event':'set','requested_enabled':enabled,'set_code':code,'status':snapshot(family,state,streams)})
      code=ctl(family,state,RESET);emit(context|{'event':'reset','retained_requested_enabled':enabled,'reset_code':code,'status':snapshot(family,state,streams)})
     # Native-only diagnostic: determine aggregate selection without inventing it.
     for index,enabled in ((streams-1,False),(0,False),(0,True)):
      c,p=child(family,state,index);code=ctl('ordinary',p,SET,I(not enabled)) if c==0 and p else c
      emit(context|{'event':'set-one-child','stream':index,'requested_enabled':enabled,'state_code':c,'set_code':code,'status':snapshot(family,state,streams)})
    except Exception as e:emit(context|{'event':'exception','error':repr(e)});errors.append(repr(e))
    finally:
     if state:getattr(lib,prefix[family]+'_decoder_destroy')(state)
 profiles={}
 for rate in (8000,12000,16000,24000,48000):
  for family,label,streams,coupled,mapping in [('ordinary','mono',1,0,[0]),('mapped','mono',1,0,[0]),('mapped','mixed',3,1,[0,1,2,255,0]),('projection','mixed',3,1,[0,1,2,3])]:
   for policy in ('default',False,True):
    state=None;context={'kind':'pcm','family':family,'layout':label,'rate':rate,'streams':streams,'coupled':coupled,'policy_enabled':policy}
    try:
     state,channels=create(family,rate,streams,coupled,mapping)
     if policy!='default':
      code=ctl(family,state,SET,I(not policy));emit(context|{'event':'set','set_code':code,'status':snapshot(family,state,streams)})
     samples=rate//400; data=packet(streams);buf=C.create_string_buffer(data)
     profile=[]
     for index in range(10):
      reset_code=ctl(family,state,RESET) if index==9 else None
      values=(C.c_float*(samples*channels+7))(*([12345.25]*(samples*channels+7)))
      is_plc=index==6;is_fec=index==7
      code=getattr(lib,prefix[family]+'_decode_float')(state,None if is_plc else buf,0 if is_plc else len(data),values,samples,int(is_fec))
      count=max(0,min(code,samples))*channels
      pcm=list(values[:count]);raw=struct.pack('<'+'f'*len(pcm),*pcm)
      profile.append(pcm)
      filename=f'{family}-{label}-{rate}-{policy}-{index}.f32';(R/filename).write_bytes(raw)
      interval=C.c_uint32();interval_code=ctl(family,state,FINAL,C.byref(interval))
      emit(context|{'event':'decode','index':index,'operation':'plc' if is_plc else 'fec-request-CELT-fallback' if is_fec else 'reset-decode' if index==9 else 'decode','reset_code':reset_code,'return_code':code,'expected_samples':samples,'channels':channels,'finite':all(map(math.isfinite,pcm)),'suffix_unchanged':all(v==12345.25 for v in values[count:]),'pcm_file':filename,'pcm_sha256':hashlib.sha256(raw).hexdigest(),'pcm_rms':math.sqrt(sum(v*v for v in pcm)/len(pcm)) if pcm else None,'range_code':interval_code,'final_range':interval.value,'status':snapshot(family,state,streams)})
     profiles[(family,label,rate,policy)]=profile
    except Exception as e:emit(context|{'event':'exception','error':repr(e)});errors.append(repr(e))
    finally:
     if state:getattr(lib,prefix[family]+'_decoder_destroy')(state)
 for rate in (8000,12000,16000,24000,48000):
  for family,label in (('ordinary','mono'),('mapped','mono'),('mapped','mixed'),('projection','mixed')):
   if not all((family,label,rate,p) in profiles for p in ('default',False,True)):continue
   for index in range(10):
    arrays={p:profiles[family,label,rate,p][index] for p in ('default',False,True)}
    emit({'kind':'pcm-policy-difference','family':family,'layout':label,'rate':rate,'index':index,'maximum_enabled_vs_disabled_difference':max((abs(x-y) for x,y in zip(arrays[True],arrays[False])),default=0),'default_equals_enabled':arrays['default']==arrays[True],'default_equals_disabled':arrays['default']==arrays[False]})
assert h(pinned['native']['path'])==pinned['native']['sha256']
assert h(R/'stereo-phase.opus')==pinned['fixture']['sha256']
(R/'receipt.json').write_text(json.dumps({'rows':len(rows),'exceptions':errors,'events_sha256':h(R/'events.jsonl'),'native_unchanged':True,'fixture_unchanged':True,'scope':'No timers/encoder/corpus generation; synthesized framing wraps an unchanged existing packet only.','files':{p.name:{'sha256':h(p),'bytes':p.stat().st_size} for p in sorted(R.glob('*.f32'))}},indent=2)+'\n')
print(json.dumps({'rows':len(rows),'exceptions':len(errors),'events_sha256':h(R/'events.jsonl')}))
