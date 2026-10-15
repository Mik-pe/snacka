from pathlib import Path
import hashlib,json,re,subprocess

W=Path('/tmp/snacka-opus-mapped-controls-worktree-20261015')
E=Path(__file__).resolve().parent
base='963b903173ff3a55cb60cfa22b189c5d76e78d48'
head=subprocess.check_output(['git','rev-parse','HEAD'],cwd=W,text=True).strip()
assert not subprocess.check_output(['git','status','--porcelain'],cwd=W,text=True)
sha=lambda b:hashlib.sha256(b).hexdigest()
def block(text,start):
 a=text.index('{',start);depth=1;end=a+1
 while depth:
  if text[end]=='{':depth+=1
  elif text[end]=='}':depth-=1
  end+=1
 return text[a:end]
def methods(text,name):
 body=block(text,text.index('impl '+name+' {'))
 return {m.group(1):block(body,m.start()) for m in re.finditer(r'(?m)^    (?:pub(?:\([^)]*\))? )?(?:const )?fn (\w+)\b',body)}
owners=(('src/opus/encoder.rs','EncoderConfig',[]),('src/opus/encoder.rs','Encoder',[]),('src/opus/multistream.rs','MultistreamEncoder',['reconfigure']),('src/opus/multistream.rs','MultistreamDecoder',['last_packet_samples']),('src/opus/projection.rs','ProjectionEncoder',['reconfigure']),('src/opus/projection.rs','ProjectionDecoder',['last_packet_samples']))
receipt={'base':base,'source_commit':head,'owners':{},'scope':'Exact existing method body/field/Cargo equality; new control/getter methods only. No timing or general native PCM parity evidence.'}
for relative,name,added in owners:
 old=subprocess.check_output(['git','show',base+':'+relative],cwd=W,text=True).split('#[cfg(test)]')[0]
 new=(W/relative).read_text().split('#[cfg(test)]')[0]
 before,after=methods(old,name),methods(new,name)
 assert sorted(set(after)-set(before))==added,(name,set(after)-set(before))
 assert all(after[k]==v for k,v in before.items()),name
 old_fields=block(old,old.index('pub struct '+name+' {'));new_fields=block(new,new.index('pub struct '+name+' {'))
 assert old_fields==new_fields,name
 receipt['owners'][name]={'path':relative,'existing_method_bodies':{k:sha(v.encode()) for k,v in before.items()},'added_methods':added,'fields_unchanged':True,'fields_sha256':sha(old_fields.encode())}
changes=subprocess.check_output(['git','diff','--name-only',base,head,'--','src','Cargo.toml','Cargo.lock'],cwd=W,text=True).splitlines()
assert changes==['src/opus/encoder.rs','src/opus/multistream.rs','src/opus/projection.rs'],changes
receipt.update(changed_production_paths=changes,existing_methods_unchanged=sum(len(v['existing_method_bodies']) for v in receipt['owners'].values()),fields_unchanged=len(owners),cargo_unchanged=True)
(E/'processing-source-audit.json').write_text(json.dumps(receipt,indent=2)+'\n')
print(json.dumps({k:v for k,v in receipt.items() if k!='owners'},indent=2))
