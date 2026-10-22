#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Build the already registered deterministic trajectory without oracle search.

Only received history and native PLC state enter this allocation-free path.
This is a nonselecting diagnostic, not a new production candidate.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('output',type=Path);p.add_argument('--protocol',type=Path,required=True);a=p.parse_args()
    repo=Path(__file__).resolve().parents[1];root=a.output.resolve()
    if root.exists():p.error('fresh source required')
    root.mkdir(parents=True);shutil.copytree(repo/'src',root/'src');(root/'examples').mkdir();(root/'tests').mkdir()
    # Reuse the exact frozen formula, not an independently reimplemented one.
    phase=(repo/'tools/plc_phase.rs').read_text();start=phase.index('pub fn trajectory(');end=phase.index('\nfn high_spectral',start)
    formula=phase[start:end]
    (root/'src/trajectory.rs').write_text('// SPDX-License-Identifier: MIT\n'+formula)
    path=root/'src/lib.rs';path.write_text(path.read_text().replace('#![deny(missing_docs)]','#![allow(missing_docs)]')+'\nmod trajectory;\n')
    source=(repo/'src/codec.rs').read_text();start=source.index('    pub fn conceal(&mut self, output: &mut [f32]) -> Result<FrameInfo, Error> {');end=source.index('    /// Reset overlap',start);body=source[start:end]
    body=body.replace('        self.block.fill(0.0);','''        let input=self.plc_history.input(samples,self.concealment.features(samples));
        let eligible=self.neural_plc && input[78]>=1.0 && input[76]<0.985 && input[73]<input[77];
        self.block.fill(0.0);''')
    body=body.replace('self.concealment.sample(ahead)','self.concealment.phase_sample(ahead,if eligible {crate::trajectory::trajectory(&input,ahead)}else{0.0})')
    begin=body.index('        if self.neural_plc {');finish=body.index('        self.synthesize',begin);body=body[:begin]+body[finish:]
    capture=body.replace('pub fn conceal(&mut self, output: &mut [f32])','pub fn conceal_capture(&mut self, output: &mut [f32], captured: &mut [f32;FRAME_SAMPLES], captured_input: &mut [f32;80], overlap: &mut [f32;ALGORITHMIC_DELAY])')
    capture=capture.replace('        self.synthesize(&coefficients, output)?;','        *captured=coefficients;*captured_input=input;*overlap=self.overlap;\n        self.synthesize(&coefficients, output)?;')
    (root/'src/codec.rs').write_text(source[:start]+body+capture+source[end:])
    path=root/'src/concealment.rs';text=path.read_text();start=text.index('    pub(crate) fn sample(&self, ahead: usize) -> f32 {');end=text.index('    pub(crate) fn advance',start)
    shifted=text[start:end].replace('fn sample(&self, ahead: usize)','fn phase_sample(&self, ahead: usize, delta: f32)')
    shifted=shifted.replace('        let elapsed = self.elapsed + ahead;','        if delta == 0.0 {return self.sample(ahead);}\n        let elapsed = self.elapsed + ahead;').replace('elapsed as f32 % self.period','(elapsed as f32+delta).rem_euclid(self.period)');path.write_text(text[:end]+shifted+text[end:])
    shutil.copy2(repo/'tools/plc_ablation_compare.rs',root/'examples/plc_ablation_compare.rs');shutil.copy2(repo/'tests/allocations.rs',root/'tests/allocations.rs')
    (root/'Cargo.toml').write_text('[package]\nname="snacka-trajectory-diagnostic"\nversion="0.0.0"\nedition="2024"\n[lib]\nname="snacka"\n[features]\nopus=[]\nreference-tests=[]\n[profile.release]\noverflow-checks=true\n')
    subprocess.run(['cargo','build','--offline','--release','--target-dir',str(root/'target'),'--examples'],cwd=root,check=True)
    receipt=dict(protocol_sha256=hashlib.sha256(a.protocol.read_bytes()).hexdigest(),trajectory_formula_sha256=hashlib.sha256(formula.encode()).hexdigest(),
        conceal_body_sha256=hashlib.sha256(body.encode()).hexdigest(),binary_sha256=hashlib.sha256((root/'target/release/examples/plc_ablation_compare').read_bytes()).hexdigest(),
        source_sha256={str(f.relative_to(repo)):hashlib.sha256(f.read_bytes()).hexdigest() for f in (repo/'src').rglob('*.rs')},purpose='nonselecting registered trajectory at a bitrate which can emit pitch')
    (root/'build-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n');print(json.dumps(receipt,indent=2))


if __name__=='__main__':main()
