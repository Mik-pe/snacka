#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Stage the selected 17-head candidate without changing the installed source.

The online conceal body is the validation student's body. Instrumentation is
added only in a separate capture method. Acceptance must bind the resulting
source and executable hashes before opening the independent cohort.
"""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess


def main():
    p=argparse.ArgumentParser(description=__doc__)
    p.add_argument('output',type=Path);p.add_argument('--student',type=Path,required=True)
    p.add_argument('--model',type=Path,required=True);p.add_argument('--selection',type=Path,required=True)
    p.add_argument('--source-commit',required=True);a=p.parse_args()
    repo=Path(__file__).resolve().parents[1];root=a.output.resolve()
    decision=json.loads(a.selection.read_text())
    if decision['selected']!='distill_phase':p.error('only the frozen phase-selected candidate is supported')
    model=json.loads((a.model/'training-report.json').read_text())
    student=json.loads((a.student/'build-receipt.json').read_text())
    if student['disable_phase'] or student['weights_sha256']!=model['weights_sha256']:p.error('student model differs')
    if root.exists():p.error('fresh output required')
    root.mkdir(parents=True)
    for name in ['src','tests','examples','benches']:shutil.copytree(repo/name,root/name)
    for name in ['Cargo.toml','Cargo.lock']:shutil.copy2(repo/name,root/name)
    for name in ['neural_plc.rs','concealment.rs','neural_plc_weights.rs']:
        shutil.copy2(a.student/'src'/name,root/'src'/name)
    # Copy the exact validated online body, without the experimental capture API.
    original=(repo/'src/codec.rs').read_text();text=(a.student/'src/codec.rs').read_text()
    signature='    pub fn conceal(&mut self, output: &mut [f32]) -> Result<FrameInfo, Error> {'
    begin=text.index(signature);end=text.index('    pub fn conceal_capture(',begin)
    body=text[begin:end]
    if hashlib.sha256(body.encode()).hexdigest()!=student['student_conceal_sha256']:raise ValueError('validated online body differs')
    start=original.index(signature);finish=original.index('    /// Reset overlap',start)
    (root/'src/codec.rs').write_text(original[:start]+body+original[finish:])
    # The old parity fixture describes a different 16-output raw head.
    path=root/'src/neural_plc.rs';text=path.read_text()
    begin=text.index('    #[test]\n    fn quantized_rust_matches_frozen_l4_pytorch_inference()')
    end=text.index('    #[test]',begin+12)
    text=text[:begin]+'''    #[test]
    fn quantized_rust_matches_selected_pytorch_controls() {
        let records=include_bytes!("../tests/testdata/neural-plc-phase-parity.bin");
        let(records,trailing)=records.as_chunks::<388>();assert!(trailing.is_empty());assert_eq!(records.len(),32);
        for record in records {
            let input=std::array::from_fn(|i|f32::from_le_bytes(record[i*4..i*4+4].try_into().unwrap()));
            let(gains,phase)=controls(&input);
            for (i,value) in gains.into_iter().chain([phase]).enumerate() {
                let offset=(80+i)*4;let expected=f32::from_le_bytes(record[offset..offset+4].try_into().unwrap());
                assert!((value-expected).abs()<5e-5,"{i}: {value} != {expected}");
            }
            assert!(gains.iter().all(|x|(0.7875..=1.2125).contains(x)));assert!((-8.0..=8.0).contains(&phase));
        }
    }

'''+text[end:];path.write_text(text)
    shutil.copy2(a.model/'parity.bin',root/'tests/testdata/neural-plc-phase-parity.bin')
    subprocess.run(['cargo','fmt'],cwd=root,check=True)
    # Freeze real production bytes before making an instrumented derivative.
    files=['src/codec.rs','src/concealment.rs','src/neural_plc.rs','src/neural_plc_weights.rs','tests/testdata/neural-plc-phase-parity.bin']
    receipt=dict(source_commit=a.source_commit,selection_sha256=hashlib.sha256(a.selection.read_bytes()).hexdigest(),
        model_weights_sha256=model['weights_sha256'],validated_online_body_sha256=student['student_conceal_sha256'],
        production_files={name:hashlib.sha256((root/name).read_bytes()).hexdigest() for name in files})
    (root/'production-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
    print(json.dumps(receipt,indent=2))


if __name__=='__main__':main()
