#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Build a noncausal oracle in a fresh, dependency-free diagnostic source copy.

Production sources and public APIs are never edited. The private hook is
derived from the exact current conceal body; anchor drift fails closed.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('output',type=Path)
    p.add_argument('--cargo',default='cargo')
    a=p.parse_args(); a.output=a.output.resolve(); repo=Path(__file__).resolve().parents[1]
    if a.output.exists():p.error('use a fresh diagnostic directory')
    a.output.mkdir(parents=True)
    shutil.copytree(repo/'src',a.output/'src')
    (a.output/'tests/testdata').mkdir(parents=True)
    shutil.copy2(repo/'tests/testdata/neural-plc-parity.bin',a.output/'tests/testdata/neural-plc-parity.bin')
    (a.output/'examples').mkdir()
    shutil.copy2(repo/'tools/plc_oracle_math.rs',a.output/'src/oracle.rs')
    shutil.copy2(repo/'tools/plc_oracle_compare.rs',a.output/'examples/oracle.rs')
    source=(a.output/'src/codec.rs').read_text()
    begin=source.index('    pub fn conceal(&mut self, output: &mut [f32]) -> Result<FrameInfo, Error> {')
    end=source.index('    /// Reset overlap',begin)
    original=source[begin:end]
    hook=original.replace('pub fn conceal(&mut self, output: &mut [f32]) -> Result<FrameInfo, Error>',
        'pub fn conceal_oracle(&mut self, output: &mut [f32], target: &[f32], kind: crate::oracle::Kind) -> Result<(FrameInfo, crate::oracle::Record), Error>')
    old='''        if self.neural_plc {
            let input = self
                .plc_history
                .input(samples, self.concealment.features(samples));
            crate::neural_plc::refine(&mut coefficients[..samples], &input);
        }
'''
    assert hook.count(old)==1
    hook=hook.replace(old,'''        let input = self.plc_history.input(samples, self.concealment.features(samples));
        let oracle_record = crate::oracle::refine(&mut coefficients[..samples], target, &input, kind);
''')
    assert hook.count('        Ok(FrameInfo {')==1
    hook=hook.replace('        Ok(FrameInfo {','        Ok((FrameInfo {')
    assert hook.count('        })\n    }')==1
    hook=hook.replace('        })\n    }','        }, oracle_record))\n    }')
    (a.output/'src/codec.rs').write_text(source[:end]+hook+source[end:])
    lib=a.output/'src/lib.rs'
    lib.write_text(lib.read_text().replace('#![deny(missing_docs)]','#![allow(missing_docs)]')+'\npub mod oracle;\n')
    (a.output/'Cargo.toml').write_text('[package]\nname="snacka-oracle-diagnostic"\nversion="0.0.0"\nedition="2024"\n[lib]\nname="snacka"\n[features]\nopus=[]\nreference-tests=[]\n[profile.release]\noverflow-checks=true\n')
    names=['src/codec.rs','src/neural_plc.rs','src/neural_plc_weights.rs','src/transform.rs','tools/plc_oracle_math.rs','tools/plc_oracle_compare.rs','tools/build_plc_oracle.py']
    receipt={'purpose':'noncausal diagnostic; not shipped inference','source_sha256':{n:hashlib.sha256((repo/n).read_bytes()).hexdigest() for n in names},
             'original_conceal_sha256':hashlib.sha256(original.encode()).hexdigest(),
             'hook_sha256':hashlib.sha256(hook.encode()).hexdigest()}
    target=str(a.output/'target')
    subprocess.run([a.cargo,'test','--release','--target-dir',target,'--lib','oracle::tests::'],cwd=a.output,check=True)
    subprocess.run([a.cargo,'build','--release','--target-dir',target,'--example','oracle'],cwd=a.output,check=True)
    binary=a.output/'target/release/examples/oracle'
    receipt['binary_sha256']=hashlib.sha256(binary.read_bytes()).hexdigest()
    (a.output/'build-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
    print(binary)


if __name__=='__main__':main()
