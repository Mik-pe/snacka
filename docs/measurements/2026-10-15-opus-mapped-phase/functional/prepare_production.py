import hashlib,json,os,pathlib,subprocess,tarfile,io,datetime
E=pathlib.Path('/tmp/snacka-opus-mapped-phase-evidence-20261015')
W=pathlib.Path('/tmp/snacka-opus-mapped-phase-20261015')
B=pathlib.Path('/tmp/snacka-opus-decoder-last-samples-evidence-20261015/production')
P=E/'production'; P.mkdir(exist_ok=True)
S=P/'source'; S.mkdir(exist_ok=True)
sha=lambda p:hashlib.sha256(pathlib.Path(p).read_bytes()).hexdigest()
m=json.loads((B/'manifest.json').read_text())
commit=subprocess.check_output(['git','rev-parse','HEAD'],cwd=W,text=True).strip()
assert commit=='567d1b6de5c41593ce1a6437d633a1f01947f60f'
assert not subprocess.check_output(['git','status','--porcelain'],cwd=W,text=True)
base='ec43d0c352a36a9a70ebe1fbd76d57efbf70a3e7'
assert subprocess.check_output(['git','diff',m['source_commit'],base,'--','Cargo.toml','Cargo.lock','src','tools/reference/benchmark_opus_decode.rs'],cwd=W)==b''
for rel,expected in m['source_files'].items():
 assert sha(pathlib.Path(m['source_directory'])/rel)==expected,rel
 assert hashlib.sha256(subprocess.check_output(['git','show',base+':'+rel],cwd=W)).hexdigest()==expected,rel
for k in ('library','binary','bridge'):
 assert sha(m[k]['path'])==m[k]['sha256'],k
archive=subprocess.check_output(['git','archive',commit],cwd=W)
with tarfile.open(fileobj=io.BytesIO(archive)) as t:t.extractall(S,filter='data')
source={p:sha(S/p) for p in m['source_files']}
changed=[p for p,h in source.items() if h!=m['source_files'][p]]
assert changed==['src/opus/multistream.rs','src/opus/projection.rs'],changed
assert sha(S/'tools/reference/benchmark_opus_decode.rs')==m['bridge']['sha256']
T=pathlib.Path('/Users/mikpe/.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin')
for tool,t in m['tools'].items(): assert sha(T/tool)==t['sha256'],tool
env=os.environ.copy(); removed=[]
for k in list(env):
 if k.startswith('CARGO_PROFILE_') or k in ('RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS','RUSTC','RUSTDOC','RUSTUP_TOOLCHAIN','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','CARGO_INCREMENTAL','CARGO_BUILD_INCREMENTAL','CARGO_TARGET_DIR','LD_LIBRARY_PATH','DYLD_LIBRARY_PATH','DYLD_INSERT_LIBRARIES','MallocNanoZone'):
  removed.append(k);env.pop(k)
explicit={'RUSTC':str(T/'rustc'),'RUSTDOC':str(T/'rustdoc'),'RUSTC_WRAPPER':'','RUSTC_WORKSPACE_WRAPPER':'','CARGO_INCREMENTAL':'0','CARGO_TARGET_DIR':str(P/'target'),'CARGO_PROFILE_RELEASE_CODEGEN_UNITS':'16'}
env.update(explicit)
libcmd=[str(T/'cargo'),'build','--locked','--release','--lib','--features','opus','-vv']
bridgecmd=[str(T/'rustc'),'--edition=2024','-O','-C','overflow-checks=yes',str(S/'tools/reference/benchmark_opus_decode.rs'),'--extern','snacka='+str(P/'target/release/libsnacka.rlib'),'-L','dependency='+str(P/'target/release/deps'),'-L','native=/opt/homebrew/lib','-o',str(P/'benchmark-opus-decode')]
pre={'status':'pinned-before-build','source_commit':commit,'base_commit':base,'baseline_manifest':{'path':str(B/'manifest.json'),'sha256':sha(B/'manifest.json')},'baseline_source_equivalence':'89e2b209 Cargo.toml/Cargo.lock/src/common bridge are byte-identical to ec43d0c','source_directory':str(S),'source_files':source,'production_changed_paths':changed,'bridge':m['bridge'],'baseline_library':m['library'],'baseline_binary':m['binary'],'native':m['native'],'tools':m['tools'],'explicit_environment':explicit,'removed_environment_keys':sorted(removed),'environment_note':'Other host environment inherited. No complete environment snapshot claimed. Values not exposed.','library_command':libcmd,'bridge_command':bridgecmd,'profile':'Rust1.99 opt3 CGU16 overflow-on debug0/defaultCPU/no extra ISA/nonincremental/wrappers-off; Cargo lto=false allows local ThinLTO, not -C lto=off; no intercrate LTO.','clock_runs':False}
(P/'manifest-before-build.json').write_text(json.dumps(pre,indent=2)+'\n')
for name,cmd in [('library-build',libcmd),('bridge-build',bridgecmd)]:
 with (P/(name+'.log')).open('w') as log:
  r=subprocess.run(cmd,cwd=S,env=env,stdout=log,stderr=subprocess.STDOUT)
 assert r.returncode==0,(name,r.returncode)
post=dict(pre,status='built-not-run',library={'path':str(P/'target/release/libsnacka.rlib'),'sha256':sha(P/'target/release/libsnacka.rlib')},binary={'path':str(P/'benchmark-opus-decode'),'sha256':sha(P/'benchmark-opus-decode')},logs={n:sha(P/n) for n in ('library-build.log','bridge-build.log')})
(P/'manifest.json').write_text(json.dumps(post,indent=2)+'\n')
print(json.dumps({'status':post['status'],'source_commit':commit,'library':post['library'],'binary':post['binary'],'manifest_sha256':sha(P/'manifest.json')}))
