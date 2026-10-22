#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Build instrumented PLC experiments in a fresh dependency-free source copy."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('output', type=Path)
    p.add_argument('--weights', type=Path)
    p.add_argument('--gain-min', type=float, help='must match the adjacent frozen training report')
    p.add_argument('--gain-max', type=float, help='must match the adjacent frozen training report')
    p.add_argument('--cargo', default='cargo')
    a = p.parse_args()
    if a.weights:
        report=a.weights.parent/'training-report.json'
        if not report.is_file():
            p.error('weights require their adjacent training-report.json; arrays alone do not define the applied gain head')
        spec=json.loads(report.read_text())['spec']
        for argument,key in (('gain_min','gain_min'),('gain_max','gain_max')):
            declared=getattr(a,argument)
            if declared is not None and declared!=spec[key]:
                p.error(f'{argument} differs from the frozen model report')
            setattr(a,argument,spec[key])
        if hashlib.sha256(a.weights.read_bytes()).hexdigest()!=json.loads(report.read_text())['weights_sha256']:
            p.error('weight bytes differ from their frozen model report')
    repo = Path(__file__).resolve().parents[1]
    root = a.output.resolve()
    if root.exists():
        p.error('fresh directory required')
    root.mkdir(parents=True)
    shutil.copytree(repo/'src', root/'src')
    (root/'examples').mkdir()
    source = (root/'src/codec.rs').read_text()
    begin = source.index('    pub fn conceal(&mut self, output: &mut [f32]) -> Result<FrameInfo, Error> {')
    end = source.index('    /// Reset overlap', begin)
    body = source[begin:end]
    hook = body.replace('pub fn conceal(&mut self, output: &mut [f32])',
        'pub fn conceal_capture(&mut self, output: &mut [f32], captured: &mut [f32; FRAME_SAMPLES], input: &mut [f32; 80], overlap: &mut [f32; ALGORITHMIC_DELAY])')
    anchor = '        if self.neural_plc {\n'
    assert hook.count(anchor) == 1
    hook = hook.replace(anchor, '''        *input = self.plc_history.input(samples, self.concealment.features(samples));
        *overlap = self.overlap;
''' + anchor)
    anchor = '        self.synthesize(&coefficients, output)?;'
    assert hook.count(anchor) == 1
    hook = hook.replace(anchor, '        *captured = coefficients;\n' + anchor)
    (root/'src/codec.rs').write_text(source[:end]+hook+source[end:])
    lib = root/'src/lib.rs'
    lib.write_text(lib.read_text().replace('#![deny(missing_docs)]', '#![allow(missing_docs)]')+
                   '\npub fn diagnostic_gains(x: &[f32;80]) -> [f32;16] { neural_plc::gains(x) }\n')
    (root/'examples/plc_parity.rs').write_text('''use std::{env,fs};
fn main() {
 let bytes=fs::read(env::args().nth(1).unwrap()).unwrap();
 assert_eq!(bytes.len(),32*384);
 let mut largest=0.0f32;
 for row in bytes.chunks_exact(384) {
  let values:Vec<f32>=row.chunks_exact(4).map(|b| f32::from_le_bytes(b.try_into().unwrap())).collect();
  let input:[f32;80]=values[..80].try_into().unwrap();
  for (actual,&expected) in snacka::diagnostic_gains(&input).iter().zip(&values[80..]) {
   largest=largest.max((actual-expected).abs());
  }
 }
 assert!(largest<5e-5,"GPU/Rust difference {largest}");
 println!("parity_max_abs={largest}");
}
''')
    for name in ('plc_sequence', 'plc_ablation_compare'):
        shutil.copy2(repo/f'tools/{name}.rs', root/f'examples/{name}.rs')
    if a.weights:
        shutil.copy2(a.weights, root/'src/neural_plc_weights.rs')
        path = root/'src/neural_plc.rs'
        text = path.read_text()
        anchor = 'output.map(|x| 1.0 + weights::STRENGTH * 0.5 * x.tanh())'
        assert text.count(anchor) == 1
        center, radius = (a.gain_min+a.gain_max)/2, (a.gain_max-a.gain_min)/2
        path.write_text(text.replace(anchor, f'output.map(|x| {center} + {radius} * x.tanh())'))
    (root/'Cargo.toml').write_text('[package]\nname="snacka-plc-ablation"\nversion="0.0.0"\nedition="2024"\n[lib]\nname="snacka"\n[features]\nopus=[]\nreference-tests=[]\n[profile.release]\noverflow-checks=true\n')
    subprocess.run([a.cargo, 'build', '--offline', '--release', '--target-dir', str(root/'target'), '--examples'], cwd=root, check=True)
    receipt = {'source_commit': subprocess.check_output(['git','rev-parse','HEAD'],cwd=repo,text=True).strip(),
               'source_sha256': {str(f.relative_to(repo)):hashlib.sha256(f.read_bytes()).hexdigest()
                                 for f in sorted((repo/'src').rglob('*.rs'))},
               'original_conceal_sha256':hashlib.sha256(body.encode()).hexdigest(),
               'capture_hook_sha256':hashlib.sha256(hook.encode()).hexdigest(),
               'gain_bounds':[a.gain_min,a.gain_max] if a.weights else 'installed main',
               'weights_sha256':hashlib.sha256((root/'src/neural_plc_weights.rs').read_bytes()).hexdigest(),
               'binaries':{n:hashlib.sha256((root/f'target/release/examples/{n}').read_bytes()).hexdigest()
                           for n in ('plc_sequence','plc_ablation_compare')}}
    (root/'build-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
    print(json.dumps(receipt['binaries']))


if __name__ == '__main__':
    main()
