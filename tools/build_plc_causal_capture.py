#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Instrument a frozen production candidate, preserving its online conceal body."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('source',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
    repo=Path(__file__).resolve().parents[1];root=a.output.resolve()
    if root.exists():p.error('fresh output required')
    root.mkdir(parents=True);shutil.copytree(a.source/'src',root/'src');(root/'examples').mkdir()
    path=root/'src/codec.rs';text=path.read_text();start=text.index('    pub fn conceal(&mut self, output: &mut [f32]) -> Result<FrameInfo, Error> {');end=text.index('    /// Reset overlap',start);body=text[start:end]
    capture=body.replace('pub fn conceal(&mut self, output: &mut [f32])','pub fn conceal_capture(&mut self, output: &mut [f32], captured: &mut [f32;FRAME_SAMPLES], captured_input: &mut [f32;80], overlap: &mut [f32;ALGORITHMIC_DELAY])')
    anchor='        self.synthesize(&coefficients, output)?;';assert capture.count(anchor)==1
    capture=capture.replace(anchor,'        *captured=coefficients;*captured_input=input;*overlap=self.overlap;\n'+anchor)
    path.write_text(text[:end]+capture+text[end:])
    path=root/'src/lib.rs';path.write_text(path.read_text().replace('#![deny(missing_docs)]','#![allow(missing_docs)]'))
    for name in ['cpu_probe','roundtrip','plc_compare']:shutil.copy2(repo/'examples'/f'{name}.rs',root/'examples'/f'{name}.rs')
    shutil.copy2(repo/'tools/plc_ablation_compare.rs',root/'examples/plc_ablation_compare.rs')
    (root/'Cargo.toml').write_text('[package]\nname="snacka-causal-acceptance"\nversion="0.0.0"\nedition="2024"\n[lib]\nname="snacka"\n[features]\nopus=[]\nreference-tests=[]\n[profile.release]\noverflow-checks=true\n')
    subprocess.run(['cargo','build','--offline','--release','--examples'],cwd=root,check=True)
    receipt=dict(production_receipt=json.loads((a.source/'production-receipt.json').read_text()),
        conceal_body_sha256=hashlib.sha256(body.encode()).hexdigest(),binaries={p.name:hashlib.sha256(p.read_bytes()).hexdigest() for p in (root/'target/release/examples').iterdir() if p.is_file() and '.' not in p.name})
    (root/'build-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n');print(json.dumps(receipt,indent=2))


if __name__=='__main__':main()
