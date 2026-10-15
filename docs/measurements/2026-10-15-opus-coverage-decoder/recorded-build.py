# SPDX-License-Identifier: MIT
# Recorded macOS run; use an artifact directory outside the checkout.
from pathlib import Path
import shutil,subprocess,hashlib,json,os,platform
ROOT=Path(os.environ["SNACKA_BENCH_CHECKOUT"]).resolve()
BASE=Path(os.environ["SNACKA_BENCH_ARTIFACTS"]).resolve()
TC=Path(os.environ["SNACKA_RUST_TOOLCHAIN_BIN"]).resolve()
env={**os.environ,'PATH':str(TC)+os.pathsep+os.environ['PATH'],
 'RUSTC':str(TC/'rustc'),'RUSTC_WRAPPER':'','RUSTC_WORKSPACE_WRAPPER':'',
 'CARGO_INCREMENTAL':'0','RUSTFLAGS':'-C codegen-units=16'}
env.pop('CARGO_ENCODED_RUSTFLAGS',None)
manifest={'rustc':subprocess.check_output([str(TC/'rustc'),'-Vv'],text=True),
 'host':platform.platform(),'cpu':subprocess.check_output(['sysctl','-n','machdep.cpu.brand_string'],text=True).strip(),
 'flags':{'overflow_checks':True,'codegen_units':16,'lto':False,'native_cpu':False,'incremental':False,'rustc_wrappers':False},
 'snapshots':{}}
for name in ['before','after']:
 dest=BASE/'clean'/name
 shutil.copytree(BASE/'baseline',dest,dirs_exist_ok=True)
 if name=='after':
  for p in (ROOT/'src').rglob('*.rs'): shutil.copy2(p,dest/p.relative_to(ROOT))
 probe=ROOT/'tools/reference/benchmark_opus_decode.rs'
 shutil.copy2(probe,dest/'tools/reference/benchmark_opus_decode.rs')
 target=BASE/'clean'/f'{name}-target'
 cmd=[str(TC/'cargo'),'build','--locked','--release','--features','opus','--lib','--target-dir',str(target)]
 subprocess.run(cmd,cwd=dest,env=env,check=True)
 binary=BASE/f'clean-{name}-decode'
 bridge=[str(TC/'rustc'),'--edition=2024','-O','-C','overflow-checks=yes','-C','codegen-units=16',str(probe),'--extern',f'snacka={target}/release/libsnacka.rlib','-L',f'dependency={target}/release/deps','-L','native=/opt/homebrew/lib','-o',str(binary)]
 subprocess.run(bridge,env=env,check=True)
 files=[dest/'Cargo.toml',dest/'Cargo.lock',dest/'tools/reference/benchmark_opus_decode.rs',*sorted((dest/'src').rglob('*.rs'))]
 manifest['snapshots'][name]={'build':cmd,'bridge':bridge,
  'sources':{str(p.relative_to(dest)):hashlib.sha256(p.read_bytes()).hexdigest() for p in files},
  'binary_sha256':hashlib.sha256(binary.read_bytes()).hexdigest()}
 print(name+' ready',flush=True)
(BASE/'clean/source.json').write_text(json.dumps(manifest,indent=2)+'\n')
