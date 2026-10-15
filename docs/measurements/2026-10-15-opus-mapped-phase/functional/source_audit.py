import pathlib,json,subprocess,hashlib,re
W=pathlib.Path('/tmp/snacka-opus-mapped-phase-20261015');E=pathlib.Path(__file__).resolve().parent
base='ec43d0c352a36a9a70ebe1fbd76d57efbf70a3e7';head='567d1b6de5c41593ce1a6437d633a1f01947f60f'
h=lambda b:hashlib.sha256(b).hexdigest()
assert subprocess.check_output(['git','rev-parse','HEAD'],cwd=W,text=True).strip()==head
assert not subprocess.check_output(['git','status','--porcelain'],cwd=W,text=True)
assert subprocess.check_output(['git','diff','89e2b209184b43de3e69c37ad33f6b00976254f9',base,'--','Cargo.toml','Cargo.lock','src','tools/reference/benchmark_opus_decode.rs'],cwd=W)==b''
r={'base_commit':base,'source_commit':head,'baseline_89e2_production_equivalent':True,'existing_method_bodies':{},'fields':{},'new_methods':{},'scope':'Text-level exact method-body and field equality, not native waveform/ASM/timing evidence.'}
for rel,struct in [('src/opus/multistream.rs','MultistreamDecoder'),('src/opus/projection.rs','ProjectionDecoder')]:
 old=subprocess.check_output(['git','show',base+':'+rel],cwd=W,text=True).split('#[cfg(test)]')[0];new=(W/rel).read_text().split('#[cfg(test)]')[0]
 def block(text,start):
  a=text.index('{',start);d=1;b=a+1
  while d:
   if text[b]=='{':d+=1
   elif text[b]=='}':d-=1
   b+=1
  return text[a:b]
 start=old.index('impl '+struct+' {');startnew=new.index('impl '+struct+' {')
 def methods(text,start):
  body=block(text,start)
  return {m.group(1):block(body,m.start()) for m in re.finditer(r'(?m)^    (?:pub(?:\([^)]*\))? )?(?:const )?fn (\w+)\b',body)}
 a=methods(old,start);b=methods(new,startnew)
 for name,body in a.items():assert b[name]==body,(rel,name)
 r['existing_method_bodies'][rel]={name:h(body.encode()) for name,body in a.items()}
 added=sorted(set(b)-set(a));assert added==['phase_inversion','set_phase_inversion'],added
 r['new_methods'][rel]=added
 f1=block(old,old.index('pub struct '+struct+' {'));f2=block(new,new.index('pub struct '+struct+' {'));assert f1==f2
 r['fields'][rel]={'sha256':h(f1.encode()),'unchanged':True}
changes=subprocess.check_output(['git','diff','--name-only',base,head,'--','src','Cargo.toml','Cargo.lock'],cwd=W,text=True).splitlines();assert changes==['src/opus/multistream.rs','src/opus/projection.rs']
r['changed_production_paths']=changes
(E/'processing-source-audit.json').write_text(json.dumps(r,indent=2)+'\n');print(json.dumps({'existing_methods_unchanged':sum(map(len,r['existing_method_bodies'].values())),'fields_unchanged':len(r['fields']),'new_methods':r['new_methods']}))
