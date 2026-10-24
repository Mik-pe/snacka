#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Stage tagged native candidates on current PR31; raw-energy activation is explicit."""
import argparse,hashlib,json,shutil,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def sha(p):return hashlib.sha256(p.read_bytes()).hexdigest()
def copy(src,dst):shutil.copytree(src,dst,copy_function=shutil.copyfile)
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('output',type=Path);p.add_argument('--model',type=Path);p.add_argument('--phase',choices=['off','on'],default='on');a=p.parse_args()
 out=a.output.resolve()
 if out.exists():p.error('fresh output required')
 out.mkdir(parents=True);copy(ROOT/'src',out/'src');copy(ROOT/'tests',out/'tests');(out/'examples').mkdir()
 kind='installed-PR31';energy=False
 if a.model:
  report=json.loads((a.model/'training-report.json').read_text());kind=report['model_kind'];assert sha(a.model/'neural_plc_weights.rs')==report['weights_sha256']
  assert kind in ('gain-ratio-phase-v1','target-log-power-phase-v1');energy=kind=='target-log-power-phase-v1'
  weights=(a.model/'neural_plc_weights.rs').read_text();assert f'const MODEL_KIND: &str = "{kind}";' in weights
  assert f'const FORMAT_TAG: u32 = {2 if energy else 1};' in weights
  (out/'src/neural_plc_weights.rs').write_text(weights)
  path=out/'src/neural_plc.rs';s=path.read_text();s=s.replace('    count: usize,','    count: usize,\n    last_log_power: [f32;16],').replace('            count: 0,','            count: 0,\n            last_log_power: [0.0;16],')
  s=s.replace('            frame[band] = (energy / values.len() as f32 + 1e-12).ln() * 0.5;','            let log_power=(energy / values.len() as f32 + 1e-12).ln();\n            self.last_log_power[band]=log_power;\n            frame[band]=log_power*0.5;')
  s=s.replace('    pub(crate) fn reset(&mut self) {','    pub(crate) fn last_log_power(&self)->[f32;16] {self.last_log_power}\n    pub(crate) fn reset(&mut self) {')
  start=s.index('pub(crate) fn controls(');end=s.index('#[cfg(test)]',start)
  # Preserve diagnostic ratio helpers; runtime explicitly selects the tagged representation.
  old=s[start:end];raw=old.replace('fn controls(', 'fn raw_controls(');raw=raw[:raw.index('    (\n')]+'''    (std::array::from_fn(|i| output[i]), 8.0 * output[16].tanh())
}
'''
  s=s[:start]+f'const _: () = assert!(weights::FORMAT_TAG == {2 if energy else 1});\n'+raw+old+s[end:]
  # Replace the obsolete installed-model fixture with a raw-head fixture (same17shape, explicit semantics).
  start=s.index('    #[test]\n    fn quantized_rust_matches_selected_pytorch_controls()');end=s.index('    #[test]',start+12)
  s=s[:start]+'''    #[test]
    fn raw_head_matches_quantized_pytorch() {
        let bytes=include_bytes!("../tests/testdata/energy-raw-parity.bin");
        let (records,tail)=bytes.as_chunks::<388>();assert!(tail.is_empty());assert_eq!(records.len(),32);
        for record in records {
            let x=std::array::from_fn(|i|f32::from_le_bytes(record[4*i..4*i+4].try_into().unwrap()));
            let (raw,phase)=raw_controls(&x);
            for (i,value) in raw.into_iter().chain([phase]).enumerate() {
                let k=4*(80+i);let expected=f32::from_le_bytes(record[k..k+4].try_into().unwrap());
                assert!((value-expected).abs()<=5e-6,"{}: {} != {}",i,value,expected);
            }
        }
    }

'''+s[end:]
  path.write_text(s);shutil.copyfile(a.model/'raw-parity.bin',out/'tests/testdata/energy-raw-parity.bin')
  control=(ROOT/'src/neural_plc.rs').read_text();control=control[:control.index('fn layer<')];(out/'src/plc_history_control.rs').write_text(control.replace('neural_plc_weights as weights','neural_plc_weights as _weights'));shutil.copyfile(ROOT/'tools/plc_target_history_tests.rs',out/'src/plc_target_history_tests.rs')
  lib=out/'src/lib.rs';lib.write_text(lib.read_text()+'\n#[cfg(test)]\n#[allow(dead_code,unused_imports)]\nmod plc_history_control;\n#[cfg(test)]\nmod plc_target_history_tests;\n')
  shutil.copyfile(ROOT/'tools/plc_target_energy.rs',out/'src/target_energy.rs')
  path=out/'src/lib.rs';path.write_text(path.read_text()+'\n#[allow(dead_code)]\nmod target_energy;\n')
  path=out/'src/codec.rs';s=path.read_text();start=s.index('    pub fn conceal(&mut self, output: &mut [f32])');end=s.index('    /// Reset overlap',start);body=s[start:end]
  body=body.replace('self.neural_plc && input[78]', 'self.neural_plc && crate::neural_plc_weights::ENABLED && input[78]')
  body=body.replace('crate::neural_plc::controls(&input)','crate::neural_plc::raw_controls(&input)')
  # raw placeholders are never converted on disabled/ineligible paths.
  anchor='        self.block.fill(0.0);'
  adjustment='''        let eligible=eligible && gains.iter().all(|v|v.is_finite()) && delta.is_finite();
        let delta=if eligible {delta} else {0.0};
'''
  if a.phase=='off':adjustment+='        let delta=0.0;\n'
  body=body.replace(anchor,adjustment+anchor,1)
  anchor='        if eligible {\n            let edges'
  conversion='crate::target_energy::gains(&coefficients[..samples],&self.plc_history.last_log_power(),&gains)' if energy else 'gains.map(|v|1.0+0.2125*v.tanh())'
  body=body.replace(anchor,f'        let gains=if eligible {{{conversion}}} else {{[1.0;16]}};\n'+anchor)
  s=s[:start]+body+s[end:];path.write_text(s)
 # Capture copy adds diagnostics without changing online production conceal body.
 path=out/'src/codec.rs';text=path.read_text();start=text.index('    pub fn conceal(&mut self, output: &mut [f32])');end=text.index('    /// Reset overlap',start);body=text[start:end]
 capture=body.replace('pub fn conceal(&mut self, output: &mut [f32])','pub fn conceal_capture(&mut self, output: &mut [f32], captured: &mut [f32;FRAME_SAMPLES], captured_input: &mut [f32;80], overlap: &mut [f32;ALGORITHMIC_DELAY])')
 anchor='        self.synthesize(&coefficients, output)?;';assert capture.count(anchor)==1
 capture=capture.replace(anchor,'        *captured=coefficients;*captured_input=input;*overlap=self.overlap;\n'+anchor);path.write_text(text[:end]+capture+text[end:])
 path=out/'src/lib.rs';path.write_text(path.read_text().replace('#![deny(missing_docs)]','#![allow(missing_docs)]'))
 for name in ('cpu_probe','roundtrip','plc_footprint'):shutil.copyfile(ROOT/'examples'/f'{name}.rs',out/'examples'/f'{name}.rs')
 shutil.copyfile(ROOT/'tools/plc_ablation_compare.rs',out/'examples/plc_ablation_compare.rs')
 shutil.copyfile(ROOT/'tools/plc_energy_timing.rs',out/'examples/plc_energy_timing.rs')
 (out/'Cargo.toml').write_text('[package]\nname="snacka-energy-study"\nversion="0.0.0"\nedition="2024"\n[lib]\nname="snacka"\n[features]\nopus=[]\nreference-tests=[]\n[profile.release]\noverflow-checks=true\n')
 subprocess.run(['cargo','fmt'],cwd=out,check=True)
 subprocess.run(['cargo','test','--offline','--release','--target-dir',str(out/'target'),'--lib','--test','allocations'],cwd=out,check=True)
 subprocess.run(['cargo','build','--offline','--release','--target-dir',str(out/'target'),'--examples'],cwd=out,check=True)
 receipt=dict(model_kind=kind,phase=a.phase,model_report_sha256=sha(a.model/'training-report.json') if a.model else None,conceal_body_sha256=hashlib.sha256(body.encode()).hexdigest(),sources={str(f.relative_to(out)):sha(f) for f in sorted((out/'src').rglob('*.rs'))},binaries={f.name:sha(f) for f in (out/'target/release/examples').iterdir() if f.is_file() and '.' not in f.name})
 (out/'production-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
if __name__=='__main__':main()
