#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Build a matched expanded phase oracle and causal f0 trajectory in a fresh source copy."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('output', type=Path)
    a = p.parse_args()
    repo = Path(__file__).resolve().parents[1]
    root = a.output.resolve()
    if root.exists():
        p.error('fresh diagnostic directory required')
    root.mkdir(parents=True)
    shutil.copytree(repo / 'src', root / 'src')
    (root / 'examples').mkdir()
    shutil.copy2(repo / 'tools/plc_phase.rs', root / 'src/representation.rs')
    shutil.copy2(repo / 'tools/plc_representation_compare.rs', root / 'examples/representation.rs')
    (root / 'tests/testdata').mkdir(parents=True)
    shutil.copy2(repo / 'tests/testdata/neural-plc-parity.bin', root / 'tests/testdata/neural-plc-parity.bin')
    source = (root / 'src/codec.rs').read_text()
    begin = source.index('    pub fn conceal(&mut self, output: &mut [f32]) -> Result<FrameInfo, Error> {')
    end = source.index('    /// Reset overlap', begin)
    body = source[begin:end]
    hook = body.replace('pub fn conceal(&mut self, output: &mut [f32]) -> Result<FrameInfo, Error>',
        'pub fn conceal_representation(&mut self, output: &mut [f32], target: &[f32], kind: crate::representation::Kind, supported: usize, seed: u32) -> Result<(FrameInfo, crate::representation::Record), Error>')
    start = hook.index('        self.block.fill(0.0);')
    finish = hook.index('        self.synthesize(&coefficients, output)?;')
    hook = hook[:start] + '''        let input = self.plc_history.input(samples, self.concealment.features(samples));
        let mut candidates = Vec::new();
        for &delta in crate::representation::PHASES {
            self.block.fill(0.0);
            for (ahead, value) in self.block[start..start + samples + ALGORITHMIC_DELAY].iter_mut().enumerate() {
                *value = self.concealment.representation_sample(ahead, delta);
            }
            let mut coefficients = vec![0.0; samples];
            self.plan.forward(&self.block[..2 * samples], &mut coefficients, &mut self.workspace).map_err(|_| Error::Packet)?;
            candidates.push(coefficients);
        }
        self.block.fill(0.0);
        for (ahead,value) in self.block[start..start+samples+ALGORITHMIC_DELAY].iter_mut().enumerate() {
            let delta=crate::representation::trajectory(&input,ahead);
            *value=self.concealment.representation_sample(ahead,delta);
        }
        let mut trajectory=vec![0.0;samples];
        self.plan.forward(&self.block[..2*samples],&mut trajectory,&mut self.workspace).map_err(|_|Error::Packet)?;
        candidates.push(trajectory);
        let (selected, record) = crate::representation::select(&candidates, target, &input, kind, supported, seed);
        let mut coefficients = [0.0; FRAME_SAMPLES];
        coefficients[..samples].copy_from_slice(&selected);
''' + hook[finish:]
    hook = hook.replace('        Ok(FrameInfo {', '        Ok((FrameInfo {').replace('        })\n    }', '        }, record))\n    }')
    (root / 'src/codec.rs').write_text(source[:end] + hook + source[end:])
    path = root / 'src/concealment.rs'
    text = path.read_text()
    start = text.index('    pub(crate) fn sample(&self, ahead: usize) -> f32 {')
    end = text.index('    pub(crate) fn advance', start)
    shifted = text[start:end].replace('fn sample(&self, ahead: usize)', 'fn representation_sample(&self, ahead: usize, delta: f32)')
    shifted = shifted.replace('        let elapsed = self.elapsed + ahead;', '''        if delta == 0.0 { return self.sample(ahead); }
        let elapsed = self.elapsed + ahead;''')
    shifted = shifted.replace('elapsed as f32 % self.period', '(elapsed as f32 + delta).rem_euclid(self.period)')
    path.write_text(text[:end] + shifted + text[end:])
    lib = root / 'src/lib.rs'
    lib.write_text(lib.read_text().replace('#![deny(missing_docs)]', '#![allow(missing_docs)]') + '\npub mod representation;\n')
    (root / 'Cargo.toml').write_text('[package]\nname="snacka-representation-diagnostic"\nversion="0.0.0"\nedition="2024"\n[lib]\nname="snacka"\n[features]\nopus=[]\nreference-tests=[]\n[profile.release]\noverflow-checks=true\n')
    subprocess.run(['cargo', 'test', '--offline', '--release', '--target-dir', str(root/'target'), '--lib', 'representation::tests::'], cwd=root, check=True)
    subprocess.run(['cargo', 'build', '--offline', '--release', '--target-dir', str(root/'target'), '--example', 'representation'], cwd=root, check=True)
    binary = root / 'target/release/examples/representation'
    names = list(sorted((repo/'src').rglob('*.rs'))) + [repo/'tools'/n for n in ('build_plc_phase.py', 'plc_phase.rs', 'plc_representation_compare.rs')]
    receipt = dict(source_sha256={str(f.relative_to(repo)):hashlib.sha256(f.read_bytes()).hexdigest() for f in names},
                   original_conceal_sha256=hashlib.sha256(body.encode()).hexdigest(),
                   hook_sha256=hashlib.sha256(hook.encode()).hexdigest(),
                   binary_sha256=hashlib.sha256(binary.read_bytes()).hexdigest())
    (root/'build-receipt.json').write_text(json.dumps(receipt, indent=2)+'\n')
    print(binary)


if __name__ == '__main__':
    main()
