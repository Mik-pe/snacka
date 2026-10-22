#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Isolated allocation-free17-head native student, with matched phase-off ablation."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('output',type=Path);p.add_argument('--model',type=Path,required=True)
    p.add_argument('--disable-phase',action='store_true');p.add_argument('--source-commit',required=True);a=p.parse_args()
    repo=Path(__file__).resolve().parents[1];root=a.output.resolve();model=a.model.resolve();report=json.loads((model/'training-report.json').read_text())
    weights=model/'neural_plc_weights.rs'
    if hashlib.sha256(weights.read_bytes()).hexdigest()!=report['weights_sha256']:p.error('model hash differs')
    if report['spec']!=dict(gain_min=.7875,gain_max=1.2125,phase_max=8.,outputs=17):p.error('unsupported head definition')
    if root.exists():p.error('fresh isolated source directory required')
    root.mkdir(parents=True);shutil.copytree(repo/'src',root/'src');(root/'examples').mkdir();(root/'tests').mkdir()
    shutil.copy2(weights,root/'src/neural_plc_weights.rs')
    path=root/'src/neural_plc.rs';text=path.read_text();begin=text.index('pub(crate) fn gains(');end=text.index('pub(crate) fn refine(',begin)
    text=text[:begin]+'''pub(crate) fn controls(input: &[f32;80]) -> ([f32;16],f32) {
    let first=layer(input,&weights::W1,&weights::S1,&weights::B1,true);
    let second=layer(&first,&weights::W2,&weights::S2,&weights::B2,true);
    let output=layer(&second,&weights::W3,&weights::S3,&weights::B3,false);
    (std::array::from_fn(|i|1.0+0.2125*output[i].tanh()),8.0*output[16].tanh())
}
pub(crate) fn gains(input: &[f32;80]) -> [f32;16] {controls(input).0}

'''+text[end:];path.write_text(text)
    path=root/'src/concealment.rs';text=path.read_text();begin=text.index('    pub(crate) fn sample(&self, ahead: usize) -> f32 {');end=text.index('    pub(crate) fn advance',begin)
    shifted=text[begin:end].replace('fn sample(&self, ahead: usize)','fn phase_sample(&self, ahead: usize, delta: f32)')
    shifted=shifted.replace('        let elapsed = self.elapsed + ahead;','        if delta == 0.0 {return self.sample(ahead);}\n        let elapsed = self.elapsed + ahead;')
    shifted=shifted.replace('elapsed as f32 % self.period','(elapsed as f32+delta).rem_euclid(self.period)');path.write_text(text[:end]+shifted+text[end:])
    path=root/'src/codec.rs';text=path.read_text();begin=text.index('    pub fn conceal(&mut self, output: &mut [f32]) -> Result<FrameInfo, Error> {');end=text.index('    /// Reset overlap',begin);original=text[begin:end]
    body=original.replace('        self.block.fill(0.0);','''        let input=self.plc_history.input(samples,self.concealment.features(samples));
        let eligible=self.neural_plc && input[78]>=1.0 && input[76]<0.985 && input[73]<input[77];
        let (gains,delta)=if eligible {crate::neural_plc::controls(&input)}else{([1.0;16],0.0)};
        self.block.fill(0.0);''')
    body=body.replace('self.concealment.sample(ahead)','self.concealment.phase_sample(ahead, '+('0.0' if a.disable_phase else 'delta')+')')
    start=body.index('        if self.neural_plc {');finish=body.index('        self.synthesize',start)
    body=body[:start]+'''        if eligible {
            let edges=crate::neural_plc::edges(samples);
            for band in 0..16 {
                for value in &mut coefficients[edges[band]..edges[band+1]] {*value*=gains[band];}
            }
        }
'''+body[finish:]
    capture=body.replace('pub fn conceal(&mut self, output: &mut [f32])','pub fn conceal_capture(&mut self, output: &mut [f32], captured: &mut [f32;FRAME_SAMPLES], captured_input: &mut [f32;80], overlap: &mut [f32;ALGORITHMIC_DELAY])')
    capture=capture.replace('        self.synthesize(&coefficients, output)?;','        *captured=coefficients;*captured_input=input;*overlap=self.overlap;\n        self.synthesize(&coefficients, output)?;')
    path.write_text(text[:begin]+body+capture+text[end:])
    path=root/'src/lib.rs';path.write_text(path.read_text().replace('#![deny(missing_docs)]','#![allow(missing_docs)]')+'\npub fn diagnostic_controls(x:&[f32;80])->[f32;17]{let(g,p)=neural_plc::controls(x);std::array::from_fn(|i|if i<16 {g[i]}else{p})}\n')
    shutil.copy2(repo/'tools/plc_ablation_compare.rs',root/'examples/plc_ablation_compare.rs')
    shutil.copy2(repo/'tests/allocations.rs',root/'tests/allocations.rs')
    shutil.copy2(repo/'examples/plc_compare.rs',root/'examples/plc_compare.rs')
    shutil.copy2(repo/'tools/plc_inference_probe.rs',root/'examples/inference_probe.rs')
    (root/'examples/parity.rs').write_text('''use std::{env,fs};fn main(){
 let bytes=fs::read(env::args().nth(1).unwrap()).unwrap();assert_eq!(bytes.len(),32*388);
 let mut worst=0.0f32;for row in bytes.chunks_exact(388){let v:Vec<f32>=row.chunks_exact(4).map(|b|f32::from_le_bytes(b.try_into().unwrap())).collect();let x:[f32;80]=v[..80].try_into().unwrap();
 for(a,b)in snacka::diagnostic_controls(&x).iter().zip(&v[80..]){assert!(a.is_finite()&&b.is_finite());worst=worst.max((a-b).abs());}}
 assert!(worst<5e-5,"17head parity {worst}");println!("parity_max_abs={worst}");}
''')
    (root/'Cargo.toml').write_text('[package]\nname="snacka-phase-student"\nversion="0.0.0"\nedition="2024"\n[lib]\nname="snacka"\n[features]\nopus=[]\nreference-tests=[]\n[profile.release]\noverflow-checks=true\n')
    subprocess.run(['cargo','build','--offline','--release','--target-dir',str(root/'target'),'--examples'],cwd=root,check=True)
    subprocess.run([str(root/'target/release/examples/parity'),str(model/'parity.bin')],check=True)
    receipt=dict(source_commit=a.source_commit,disable_phase=a.disable_phase,model_report_sha256=hashlib.sha256((model/'training-report.json').read_bytes()).hexdigest(),weights_sha256=report['weights_sha256'],
        source_sha256={str(f.relative_to(repo)):hashlib.sha256(f.read_bytes()).hexdigest() for f in sorted((repo/'src').rglob('*.rs'))},
        original_conceal_sha256=hashlib.sha256(original.encode()).hexdigest(),student_conceal_sha256=hashlib.sha256(body.encode()).hexdigest(),
        binary_sha256=hashlib.sha256((root/'target/release/examples/plc_ablation_compare').read_bytes()).hexdigest())
    (root/'build-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')

if __name__=='__main__':main()
