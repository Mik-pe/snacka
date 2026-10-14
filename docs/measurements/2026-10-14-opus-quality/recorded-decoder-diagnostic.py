import ctypes as ct,csv,hashlib,json,os,pathlib,struct,subprocess,sys,tempfile
import numpy as np
sys.path.insert(0,'/workspace/snacka/tools/reference')
from evaluate_opus_quality import Reference,packet_archive,metrics
corpus=pathlib.Path('/workspace/snacka-quality-corpus')
rows=[]
with tempfile.TemporaryDirectory() as directory:
 d=pathlib.Path(directory);source=d/'input.f32';output=d/'owned.f32';archive=d/'packets.bin'
 for clip in ['cen1-fcaw-b.wav','cen1-fjlp-b.wav','cen1-fvap-b.wav','cen1-marh-b.wav']:
  subprocess.run(['ffmpeg','-v','error','-y','-i',str(corpus/clip),'-ar','48000','-ac','1','-f','f32le',str(source)],check=True)
  original=np.frombuffer(source.read_bytes(),dtype='<f4').astype(np.float64)
  for bitrate in [12000,24000,32000]:
   ref=Reference(pathlib.Path('/workspace/snacka-reference-build/libopus.so'),960,'WB',bitrate,'silk')
   try:
    subprocess.run(['/workspace/snacka-quality-verified',str(source),str(output),str(archive),'silk','WB','20','1',str(bitrate),'-'],stdout=subprocess.DEVNULL,check=True)
    owned=np.frombuffer(output.read_bytes(),dtype='<f4').astype(np.float64)
    packets=packet_archive(archive,bitrate//400)
    decoded_ref,peer=ref.roundtrip(original,packets,set())
    for label,values in [('owned_encoder_owned_decoder',owned),('owned_encoder_reference_decoder',peer),('reference_encoder_reference_decoder',decoded_ref)]:
     rows.append({'clip':clip,'bitrate':bitrate,'codec_path':label,**metrics(original,original,values,f'{clip}:{bitrate}:diagnostic')})
   finally:ref.close()
   ref=Reference(pathlib.Path('/workspace/snacka-reference-build/libopus.so'),960,'WB',bitrate,'silk')
   try:
    raw=bytearray(b'SNKQ0001')
    for i in range((len(original)+959)//960+1):
     frame=np.zeros(960,dtype=np.float32);part=original[i*960:(i+1)*960];frame[:len(part)]=part
     storage=(ct.c_ubyte*(bitrate//400))()
     length=ref.check(ref.lib.opus_encode_float(ref.encoder,frame.ctypes.data_as(ref.float_pointer),960,storage,len(storage)))
     interval=ct.c_uint32();ref.check(ref.lib.opus_encoder_ctl(ref.encoder,4031,ct.byref(interval)))
     raw.extend(struct.pack('<II',length,interval.value));raw.extend(bytes(storage)[:length])
    archive.write_bytes(raw)
    subprocess.run(['/workspace/snacka-quality-decode',str(archive),str(output),'1',str(ref.delay),str(len(original))],check=True)
    decoded=np.frombuffer(output.read_bytes(),dtype='<f4').astype(np.float64)
    rows.append({'clip':clip,'bitrate':bitrate,'codec_path':'reference_encoder_owned_decoder',**metrics(original,original,decoded,f'{clip}:{bitrate}:diagnostic')})
   finally:ref.close()
  print('Diagnostic',clip,flush=True)
with open('/workspace/snacka-quality-diagnostic.csv','w',newline='') as stream:
 w=csv.DictWriter(stream,fieldnames=list(rows[0]),lineterminator='\n');w.writeheader();w.writerows(rows)
