import pathlib,re,hashlib,json
D=pathlib.Path(__file__).resolve().parent
h=lambda b:hashlib.sha256(b).hexdigest()
summary={'scope':'Inventory of existing release rlib AArch64 disassembly, not cost evidence. Instruction counts are static; no ABI layout guarantee is exported by the Rust type. Source DSP bodies unchanged; generated bodies are not claimed byte-identical.','roles':{}}
for role in ('before','after'):
 lines=(D/(role+'-otool.txt')).read_text().splitlines();blocks={};current=None
 for l in lines:
  if re.match(r'^\S+:$',l):current=l[:-1];blocks[current]=[]
  elif current and re.match(r'^[0-9a-f]{16}\t',l):blocks[current].append(l)
 selected={}
 for symbol,body in blocks.items():
  if '4opus7decoderNtB5_7Decoder' not in symbol:continue
  names=['decode_parsed','recover_parsed','recover_unrecorded','conceal_unrecorded','conceal','recover','write_output','render_lowband','decode_frame','conceal_frame','reset','new']
  name=next((n for n in names if re.search(r'\d'+re.escape(n)+r'(?:\.llvm\.\d+)?$',symbol)),None)
  if not name:continue
  normalized='\n'.join(l.split('\t',1)[1] for l in body)+'\n'
  selected[name]={'symbol':symbol,'instructions':len(body),'text_bytes':4*len(body),'instruction_text_sha256':h(normalized.encode()),'first_instructions':[l.split('\t',1)[1] for l in body[:3]]}
  (D/(role+'-'+name+'.asm')).write_text(symbol+':\n'+'\n'.join(body)+'\n')
 summary['roles'][role]=selected
summary['public_entrypoint_effects']={'decode_parsed':'One successful outer status store; same starting96-byte stack frame.','conceal':'New48-byte outer wrapper calls preserved unrecorded body with96-byte stack frame.','recover_parsed':'Recovery helper is inlined here by LLVM; same starting320-byte stack frame, internal conceal helper calls remain.','recover':'Public packet parse/dispatch wrapper is codegen-changed.','last_packet_samples':'Getter is inline/metadata-only and has no standalone library symbol in this build.'}
(D/'summary.json').write_text(json.dumps(summary,indent=2)+'\n')
print(json.dumps({r:{n:{k:v for k,v in d.items() if k in ('instructions','text_bytes')} for n,d in x.items()} for r,x in summary['roles'].items()},indent=2))
