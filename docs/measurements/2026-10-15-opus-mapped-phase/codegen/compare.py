from pathlib import Path
import re,json,hashlib
D=Path(__file__).resolve().parent
h=lambda s:hashlib.sha256(s.encode()).hexdigest()
def canonical(s): return re.sub(r'\.llvm\.\d+','.llvm.HASH',s)
def blocks(role):
 current=None;result={}
 for line in (D/(role+'-otool.txt')).read_text().splitlines():
  if re.match(r'^\S+:$',line):current=canonical(line[:-1]);result[current]=[]
  elif current and re.match(r'^[0-9a-f]{16}\t',line):result[current].append(line)
 return result
def normalize(lines):
 if not lines:return []
 start=int(lines[0].split('\t')[0],16);out=[]
 for line in lines:
  text=line.split('\t',1)[1];text=re.sub(r'\.llvm\.\d+','.llvm.HASH',text)
  if re.match(r'^(?:bl?|b\.\w+|cbn?z|tbn?z)\s',text):
   address=int(line.split('\t')[0],16)
   text=re.sub(r'(?<!#)0x([0-9a-f]+)$',lambda m:'RELOC_SELF' if int(m[1],16)==address else 'REL'+str(int(m[1],16)-start),text)
  out.append(text)
 return out
before=blocks('before');after=blocks('after');rows=[]
for symbol,body in before.items():
 if not ('4opus11multistreamNtB5_18MultistreamDecoder' in symbol or '4opus10projectionNtB5_17ProjectionDecoder' in symbol):continue
 if symbol not in after:rows.append({'symbol':symbol,'missing_after':True});continue
 a=normalize(body);b=normalize(after[symbol]);rows.append({'symbol':symbol,'before_instructions':len(a),'after_instructions':len(b),'instruction_text_equal':a==b,'before_instruction_sha256':h('\n'.join(a)),'after_instruction_sha256':h('\n'.join(b)),'first_differences':[{'index':i,'before':x,'after':y} for i,(x,y) in enumerate(zip(a,b)) if x!=y][:3]})
new=[s for s in after if s not in before and ('4opus11multistreamNtB5_18MultistreamDecoder' in s or '4opus10projectionNtB5_17ProjectionDecoder' in s)]
r={'scope':'Existing mapped/projection production object instruction text, perfunction address stripped, last branch destinations made function-relative; unlinked self-target relocation slots markedRELOC_SELF; LLVM local suffix normalized in keys/operands. Branch bit-index immediates and other instructions retained. This does not independently establish linked relocation target equality. Not CPU evidence.','existing_functions':rows,'new_symbols':new,'all_existing_equal':all(x.get('instruction_text_equal',False) for x in rows)}
(D/'comparison.json').write_text(json.dumps(r,indent=2)+'\n');print(json.dumps({'functions':len(rows),'all_equal':r['all_existing_equal'],'differences':[r for r in rows if not r.get('instruction_text_equal')],'new_symbols':new},indent=2))
