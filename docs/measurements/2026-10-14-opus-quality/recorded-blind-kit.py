import hashlib,json,pathlib,secrets,subprocess,sys,tempfile,zipfile
import numpy as np
from scipy.io.wavfile import write as write_wav
sys.path.insert(0,'/workspace/snacka/tools/reference')
from evaluate_opus_quality import Reference,packet_archive
from evaluate_speech import floats,noisy
root=pathlib.Path('/workspace/snacka-blind-listening');root.mkdir(exist_ok=True)
(root/'audio').mkdir(exist_ok=True)
corpus=pathlib.Path('/workspace/snacka-quality-corpus')
labels=[];key=[]
with tempfile.TemporaryDirectory() as directory:
 d=pathlib.Path(directory);source=d/'input.f32';out=d/'owned.f32';archive=d/'packets.bin'
 cases=[(clip,condition,bitrate) for clip in ['cen1-fcaw-b.wav','cen1-fjlp-b.wav','cen1-fvap-b.wav','cen1-marh-b.wav'] for condition in ['clean','noise10'] for bitrate in [12000,24000]]
 secrets.SystemRandom().shuffle(cases)
 for index,(clip,condition,bitrate) in enumerate(cases,1):
  subprocess.run(['ffmpeg','-v','error','-y','-i',str(corpus/clip),'-ar','48000','-ac','1','-f','f32le',str(source)],check=True)
  clean=source.read_bytes();data=clean if condition=='clean' else noisy(floats(clean),clip,10)[0]
  original=np.frombuffer(data,dtype='<f4').astype(np.float64);source.write_bytes(data)
  subprocess.run(['/workspace/snacka-quality-verified',str(source),str(out),str(archive),'silk','WB','20','1',str(bitrate),'-'],stdout=subprocess.DEVNULL,check=True)
  owned=np.frombuffer(out.read_bytes(),dtype='<f4')
  ref=Reference(pathlib.Path('/workspace/snacka-reference-build/libopus.so'),960,'WB',bitrate,'silk')
  try:reference,_=ref.roundtrip(original,packet_archive(archive,bitrate//400),set())
  finally:ref.close()
  role_a=secrets.choice(['snacka','libopus']);pair={'snacka':owned,'libopus':reference}
  token=f'{index:02d}'
  for label,values in [('A',pair[role_a]),('B',pair['libopus' if role_a=='snacka' else 'snacka']),('original',np.frombuffer(clean,dtype='<f4'))]:
   assert np.isfinite(values).all()
   write_wav(root/'audio'/f'{token}-{label}.wav',48000,np.asarray(values,dtype=np.float32))
  labels.append({'id':token,'A':f'audio/{token}-A.wav','B':f'audio/{token}-B.wav','original':f'audio/{token}-original.wav'})
  key.append({'id':token,'A':role_a,'B':'libopus' if role_a=='snacka' else 'snacka','clip':clip,'condition':condition,'bitrate':bitrate,'frame_ms':20,'bandwidth':'WB','channels':1})
html='''<!doctype html><html lang="sv"><meta charset="utf-8"><meta name="viewport" content="width=device-width"><title>Blind jämförelse av tal</title><style>body{font:18px system-ui;max-width:680px;margin:40px auto;padding:0 20px;background:#f4f4f1;color:#202022}audio{width:100%}section{margin:24px 0;padding:18px;background:white;border-radius:12px}button{padding:12px 20px;margin:6px;border:1px solid #888;border-radius:8px;background:white;font:inherit}label{display:block;margin:14px 0}textarea{width:95%;height:60px;font:inherit}#status{font-weight:600}</style><h1>Blind jämförelse av tal</h1><p>Lyssna på A och B med samma volym. Bedöm vilket som låter tydligast och mest naturligt, även när bakgrunden innehåller brus. Originalet finns som stöd. Välj lika om du inte hör någon skillnad.</p><p>Ta korta pauser och exportera dina svar när du är klar. Ingen data skickas någonstans.</p><div id="view"></div><button id="previous">Föregående</button><button id="next">Nästa</button><button id="export">Exportera svar</button><p id="status"></p><script>const cases=CASE_DATA;let index=0;const answers={};function show(){const c=cases[index];document.getElementById('view').innerHTML=`<h2>Par ${index+1} av ${cases.length}</h2><section><p>Original (rent tal)</p><audio controls src="${c.original}"></audio><p>A</p><audio controls src="${c.A}"></audio><p>B</p><audio controls src="${c.B}"></audio></section><section><p>Vilket föredrar du?</p>${['A','B','Lika','Kan inte bedöma'].map(x=>`<label><input type="radio" name="choice" value="${x}"> ${x}</label>`).join('')}<label>Kommentar (valfri)<textarea id="note"></textarea></label></section>`;const a=answers[c.id]||{};for(const r of document.querySelectorAll('input[name="choice"]')){r.checked=r.value===a.preference;r.onchange=()=>{answers[c.id]={preference:r.value,note:document.getElementById('note').value};update()}}document.getElementById('note').value=a.note||'';document.getElementById('note').oninput=event=>{answers[c.id]={...(answers[c.id]||{}),note:event.target.value};update()};document.getElementById('previous').disabled=index===0;document.getElementById('next').disabled=index===cases.length-1;update()}function update(){document.getElementById('status').textContent=Object.values(answers).filter(x=>x.preference).length+' / '+cases.length+' bedömda par'}document.getElementById('previous').onclick=()=>{index--;show()};document.getElementById('next').onclick=()=>{index++;show()};document.getElementById('export').onclick=()=>{const quote=x=>'"'+String(x||'').replaceAll('"','""')+'"';let csv='case,preference,note\\n'+cases.map(c=>[c.id,(answers[c.id]||{}).preference,(answers[c.id]||{}).note].map(quote).join(',')).join('\\n')+'\\n';const blob=new Blob([csv],{type:'text/csv;charset=utf-8'});const a=document.createElement('a');a.href=URL.createObjectURL(blob);a.download='blind-listening-answers.csv';a.click();setTimeout(()=>URL.revokeObjectURL(a.href),1000)};show()</script></html>'''
(root/'index.html').write_text(html.replace('CASE_DATA',json.dumps(labels,ensure_ascii=False)))
(root/'LICENSE-AN4.txt').write_bytes((corpus/'license.txt').read_bytes())
(root/'instructions.txt').write_text('Packa upp hela paketet och öppna index.html i en modern webbläsare.\nLyssna med samma enhet och volym för A och B. Avsluta med Exportera svar.\nDina svar stannar i sidan tills du exporterar dem. Codec-nyckeln finns inte i paketet.\n\nMaterial: CMU AN4, copyright 1991–2005 Carnegie Mellon University. Se LICENSE-AN4.txt.\nOriginalkorpus: https://github.com/cmusphinx/an4/tree/b567d132f43304e70a2b2eca6077f22255311812\nFyra tidigare separata utvärderingsröster, första cen1-klippet per röst.\n16 slumpordnade par: 12/24 kb/s, WB, mono, 20 ms, rent tal eller Gaussiskt brus 10 dB.\nProven jämför Snacka och libopus 1.6.1. 48 kHz float-WAV utan extra komprimering,\nnormalisering eller individuell nivåjustering. Inspelningarna kommer från 16 kHz.\nKlippen är separat tidsjusterade: Snackas nominella fördröjning 120 prover,\nlibopus VOIP-lookahead 312. Provet visar ljudkvalitet, inte samtalsfördröjning.\nDetta lilla engelska prov är utvecklingsevidens, inte full kvalitetsacceptans.\n')
private=pathlib.Path('/workspace/snacka-blind-listening-key.json');private.write_text(json.dumps(key,indent=2)+'\n')
archive_path=pathlib.Path('/workspace/snacka-blind-listening.zip')
with zipfile.ZipFile(archive_path,'w',compression=zipfile.ZIP_DEFLATED) as archive:
 for p in sorted(root.rglob('*')):
  if p.is_file():archive.write(p,p.relative_to(root))
manifest={'cases':16,'public_files_sha256':{str(p.relative_to(root)):hashlib.sha256(p.read_bytes()).hexdigest() for p in root.rglob('*') if p.is_file()},'zip_sha256':hashlib.sha256(archive_path.read_bytes()).hexdigest(),'private_key_sha256':hashlib.sha256(private.read_bytes()).hexdigest(),'method':'random pair order and per-pair A/B allocation; key excluded from exported package; unnormalized 48 kHz float WAV; listening results not yet supplied'}
pathlib.Path('/workspace/snacka-blind-listening-manifest.json').write_text(json.dumps(manifest,indent=2)+'\n')
print('Created',archive_path,'bytes',archive_path.stat().st_size,'pairs',len(labels),flush=True)
