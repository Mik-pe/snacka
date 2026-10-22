#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Audit registered Rust trajectory activation on existing causal teacher inputs.

No targets, test records, training or candidate ranking are used. Requires
NumPy and rustc. This is feature coverage, not perceptual acceptance.
"""
import argparse
import hashlib
import json
from pathlib import Path
import subprocess
import tempfile
import numpy as np


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('data',type=Path);p.add_argument('formula',type=Path);p.add_argument('output',type=Path);a=p.parse_args()
    if a.output.exists():p.error('fresh coverage output required')
    results={}
    with tempfile.TemporaryDirectory() as tmp:
        root=Path(tmp);source=root/'probe.rs';binary=root/'probe'
        source.write_text('#[path='+json.dumps(str(a.formula.resolve()))+'] mod trajectory;\n'+'''fn main(){
 let args:Vec<_>=std::env::args().collect();let bytes=std::fs::read(&args[1]).unwrap();assert!(bytes.len().is_multiple_of(320));
 let mut out=Vec::new();for row in bytes.chunks_exact(320){let x:[f32;80]=std::array::from_fn(|i|f32::from_le_bytes(row[i*4..i*4+4].try_into().unwrap()));let n=(x[72]*960.0)as usize;out.extend_from_slice(&trajectory::trajectory(&x,n/2).to_le_bytes());}
 std::fs::write(&args[2],out).unwrap();}
''')
        subprocess.run(['rustc','--edition=2024','-O',str(source),'-o',str(binary)],check=True)
        for split in ('train','validation'):
            results[split]={}
            for language,code in [('en',0),('sv',1)]:
                with np.load(a.data/f'{split}-{code}.npz',allow_pickle=False) as d:x=d['x'].copy()
                input_file=root/'input.f32';x.astype('<f4').tofile(input_file);output_file=root/'phase.f32'
                subprocess.run([str(binary),str(input_file),str(output_file)],check=True)
                phase=np.fromfile(output_file,dtype='<f4')
                if len(phase)!=len(x) or not np.isfinite(phase).all():raise ValueError('native activation probe differs')
                voiced=(x[:,[18,42,66]]>=1).all(1);freq=x[:,[17,41,65]]*500;valid=voiced&((freq>=50)&(freq<=500)).all(1)
                period=x[:,75]*960;consistent=valid&(period>0)&(np.abs(48000/np.maximum(freq[:,2],1)-period)<=period*.2)
                results[split][language]=dict(eligible_input_rows=len(x),three_received_voiced_rows=int(voiced.sum()),
                    valid_received_pitch_rows=int(valid.sum()),period_consistent_rows=int(consistent.sum()),nonzero_native_midframe_offsets=int((phase!=0).sum()),
                    mean_abs_offset_samples=float(np.abs(phase).mean()),input_sha256=hashlib.sha256(input_file.read_bytes()).hexdigest())
    report=dict(purpose='coverage of existing augmented train/validation inputs; no test/targets/selection',formula_sha256=hashlib.sha256(a.formula.read_bytes()).hexdigest(),results=results)
    a.output.write_text(json.dumps(report,indent=2)+'\n');print(json.dumps(report,indent=2))


if __name__=='__main__':main()
