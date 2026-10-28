#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Source-sealed experimental residual head; never overwrite installed PR31."""
import argparse,hashlib,json,shutil,subprocess,io,tarfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def once(s,a,b):
 if s.count(a)!=1:raise ValueError('source anchor '+a[:60])
 return s.replace(a,b)
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('output',type=Path);p.add_argument('--model',type=Path);p.add_argument('--strength',type=float,default=1.);p.add_argument('--teacher',action='store_true');p.add_argument('--baseline',action='store_true');p.add_argument('--pure-baseline',action='store_true');p.add_argument('--batched-search',action='store_true');p.add_argument('--slice-search',action='store_true');p.add_argument('--consecutive-search',action='store_true');a=p.parse_args();out=a.output.resolve()
 if out.exists():p.error('fresh output required')
 out.mkdir(parents=True)
 archive=subprocess.check_output(['git','archive','d0852534f031a55b2c0efef3f6b21d1728cc2c44','src','tests','examples'],cwd=ROOT) if (ROOT/'.git').exists() else None
 if archive is not None:
  with tarfile.open(fileobj=io.BytesIO(archive)) as stream:stream.extractall(out,filter='data')
 else:
  assert (ROOT/'SOURCE_COMMIT').read_text().strip()=='d0852534f031a55b2c0efef3f6b21d1728cc2c44'
  for folder in ('src','tests','examples'):shutil.copytree(ROOT/folder,out/folder)
 shutil.rmtree(out/'examples');(out/'examples').mkdir()
 for name in ['cpu_probe','roundtrip','plc_footprint']:shutil.copyfile(ROOT/'examples'/f'{name}.rs',out/'examples'/f'{name}.rs')
 for name in ['plc_ablation_compare','plc_energy_timing']:shutil.copyfile(ROOT/'tools'/f'{name}.rs',out/'examples'/f'{name}.rs')
 shutil.copyfile(ROOT/'tools/plc_conditioned.rs',out/'src/plc_conditioned.rs')
 weights='pub const FORMAT_TAG:u32=3;\npub const ENABLED:bool=true;\npub const USE_CANDIDATE:bool=true;\npub const DAMP_ONLY:bool=false;\npub const STRENGTH:f32=1.;\npub const WH:[[i8;64];16]=[[0;64];16];\npub const WC:[[i8;16];16]=[[0;16];16];\npub const SH:[f32;16]=[1.;16];\npub const SC:[f32;16]=[1.;16];\npub const BIAS:[f32;16]=[0.;16];\n'
 if a.model:
  report=json.loads((a.model/'training-report.json').read_text());assert sha(a.model/'plc_conditioned_weights.rs')==report['weights_sha256'];weights=(a.model/'plc_conditioned_weights.rs').read_text();assert 'FORMAT_TAG: u32 = 3;' in weights
  weights=once(weights,'pub const STRENGTH: f32 = 1.0;',f'pub const STRENGTH: f32 = {a.strength};')
 (out/'src/plc_conditioned_weights.rs').write_text(weights)
 if a.model:
  fixture=a.model/f'parity-{a.strength}.bin'
  assert fixture.is_file(),fixture
  shutil.copyfile(fixture,out/'adapter-parity.bin')
 path=out/'src/neural_plc.rs';s=path.read_text();original_history_source=s;s=once(s,'fn layer<const INPUT: usize, const OUTPUT: usize>','pub(crate) fn layer<const INPUT: usize, const OUTPUT: usize>');s=once(s,'    count: usize,','    count: usize,\n    last_log_power:[f32;16],');s=once(s,'            count: 0,','            count: 0,\n            last_log_power:[0.;16],');s=once(s,'            frame[band] = (energy / values.len() as f32 + 1e-12).ln() * 0.5;','            let log_power=(energy/values.len() as f32+1e-12).ln();\n            self.last_log_power[band]=log_power;\n            frame[band]=log_power*0.5;');s=once(s,'    pub(crate) fn reset(&mut self) {','    pub(crate) fn last_log_power(&self)->[f32;16] {self.last_log_power}\n    pub(crate) fn reset(&mut self) {');path.write_text(s)
 path=out/'src/codec.rs';source=path.read_text();start=source.index('    pub fn conceal(&mut self, output: &mut [f32])');end=source.index('    /// Reset overlap',start);original=source[start:end];body=original
 body=once(body,'        let (gains, delta) = if eligible {','        let hidden=if eligible {crate::plc_conditioned::latent(&input)} else {[0.;64]};\n        let (gains, delta) = if eligible {');body=once(body,'            crate::neural_plc::controls(&input)','            crate::plc_conditioned::base_controls(&hidden)')
 body=once(body,'        if eligible {\n            let edges','        let gains=if eligible && crate::plc_conditioned_weights::ENABLED {\n            let condition=if crate::plc_conditioned_weights::USE_CANDIDATE {crate::plc_conditioned::condition(&coefficients[..samples],&self.plc_history.last_log_power())} else {[0.;16]};\n            crate::plc_conditioned::gains(&hidden,&condition,&gains)\n        } else {gains};\n        if eligible {\n            let edges')
 if a.baseline:body=original
 capture=body.replace('pub fn conceal(&mut self, output: &mut [f32])','pub fn conceal_capture(&mut self, output: &mut [f32], captured: &mut [f32;FRAME_SAMPLES], captured_input: &mut [f32;80], overlap: &mut [f32;ALGORITHMIC_DELAY])');capture=once(capture,'        self.synthesize(&coefficients, output)?;','        *captured=coefficients;*captured_input=input;*overlap=self.overlap;\n        self.synthesize(&coefficients, output)?;')
 # A separate unchanged online baseline remains available for diagnostic parity.
 baseline=original.replace('pub fn conceal(','pub fn conceal_baseline(')
 if a.teacher:
  teacher=(ROOT/'tools/plc_conditioned_teacher_hook.rs').read_text();body+=teacher
  shutil.copyfile(ROOT/'tools/plc_conditioned_teacher.rs',out/'examples/plc_conditioned_teacher.rs')
 path.write_text(source[:start]+body+capture+baseline+source[end:])
 path=out/'src/lib.rs';path.write_text(path.read_text().replace('#![deny(missing_docs)]','#![allow(missing_docs)]')+'\n#[allow(dead_code)]\nmod plc_conditioned;\n#[allow(dead_code)]\nmod plc_conditioned_weights;\n')
 if a.teacher:
  s=(ROOT/'tools/plc_phase.rs').read_text();s=s.replace('crate::neural_plc::refine(&mut out, input);','let g=crate::neural_plc::controls(input).0;out=apply(&out,g.map(f64::from));').replace('crate::neural_plc::gains(input)','crate::neural_plc::controls(input).0');(out/'src/representation.rs').write_text(s);path.write_text(path.read_text()+'\n#[allow(dead_code)]\nmod representation;\n')
 (out/'Cargo.toml').write_text('[package]\nname="snacka-conditioned-study"\nversion="0.0.0"\nedition="2024"\n[lib]\nname="snacka"\n[features]\nopus=[]\nreference-tests=[]\n[profile.release]\noverflow-checks=true\n')
 if a.pure_baseline:
  assert a.baseline and not a.teacher and a.model is None
  (out/'src/neural_plc.rs').write_text(original_history_source)
  lib=out/'src/lib.rs';text=lib.read_text();text=text.replace('\n#[allow(dead_code)]\nmod plc_conditioned;','').replace('\n#[allow(dead_code)]\nmod plc_conditioned_weights;','');lib.write_text(text)
 if a.consecutive_search:
  from plc_consecutive_patch import apply
  apply(out/'src/concealment.rs',ROOT)
 if a.batched_search:
  from plc_batch_patch import apply
  apply(out/'src/concealment.rs',ROOT)
 if a.slice_search:
  from plc_slice_patch import apply
  apply(out/'src/concealment.rs',ROOT)
 subprocess.run(['cargo','fmt'],cwd=out,check=True);subprocess.run(['cargo','build','--offline','--release','--target-dir',str(out/'target'),'--examples'],cwd=out,check=True)
 receipt=dict(schema=1,base_commit='d0852534f031a55b2c0efef3f6b21d1728cc2c44',source_sha256={str(p.relative_to(out)):sha(p) for p in (out/'src').rglob('*.rs')},original_conceal_sha256=hashlib.sha256(original.encode()).hexdigest(),online_conceal_sha256=hashlib.sha256(body.encode()).hexdigest(),binaries={p.name:sha(p) for p in (out/'target/release/examples').iterdir() if p.is_file() and '.' not in p.name})
 (out/'production-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
if __name__=='__main__':main()
