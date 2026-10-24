#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Native raw-energy labels; teacher phase never enters causal decoder state."""
import argparse, hashlib, json, shutil, subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def main():
 p=argparse.ArgumentParser(description=__doc__);p.add_argument('output',type=Path);a=p.parse_args()
 subprocess.run(['python3',str(ROOT/'tools/build_plc_target_energy.py'),str(a.output)],check=True)
 out=a.output/'cache';path=out/'src/codec.rs';text=path.read_text();start=text.index('    pub fn conceal(&mut self, output: &mut [f32])');end=text.index('    /// Reset overlap',start);body=text[start:end]
 hook=body.replace('pub fn conceal(&mut self, output: &mut [f32]) -> Result<FrameInfo, Error>','pub fn conceal_energy_teacher(&mut self, output: &mut [f32], target: &[f32], captured: &mut [f32;130]) -> Result<FrameInfo, Error>')
 begin=hook.index('        let eligible =');finish=hook.index('        self.synthesize(&coefficients, output)?;')
 hook=hook[:begin]+'''        captured[..80].copy_from_slice(&input);
        captured[80..96].copy_from_slice(&self.plc_history.last_log_power());
        captured[96..112].copy_from_slice(&crate::target_energy::log_power(target));
        let mut candidates=Vec::new();
        for &delta in crate::representation::PHASES {
            self.block.fill(0.0);
            for (ahead,value) in self.block[start..start+samples+ALGORITHMIC_DELAY].iter_mut().enumerate() {
                *value=self.concealment.phase_sample(ahead,delta);
            }
            let mut coefficients=vec![0.0;samples];
            self.plan.forward(&self.block[..2*samples],&mut coefficients,&mut self.workspace).map_err(|_|Error::Packet)?;
            candidates.push(coefficients);
        }
        // Reserved trajectory slot is unused by PhaseWide.
        candidates.push(candidates[0].clone());
        let (_,record)=crate::representation::select(&candidates,target,&input,crate::representation::Kind::PhaseWide,16,20261022);
        let selected=crate::representation::PHASES.iter().position(|&v|v==record.phase).ok_or(Error::Packet)?;
        captured[112..128].copy_from_slice(&crate::target_energy::log_power(&candidates[selected]));
        captured[128]=record.phase;
        captured[129]=if record.eligible {1.0}else{0.0};
        // Labels can see the target; the actual decoder always uses candidate0.
        let mut coefficients=[0.0;FRAME_SAMPLES];
        coefficients[..samples].copy_from_slice(&candidates[0]);
'''+hook[finish:]
 path.write_text(text[:end]+hook+text[end:])
 representation=(ROOT/'tools/plc_phase.rs').read_text().replace('crate::neural_plc::refine(&mut out, input);','let gg=crate::neural_plc::controls(input).0; out=apply(&out,gg.map(f64::from));').replace('crate::neural_plc::gains(input)','crate::neural_plc::controls(input).0')
 (out/'src/representation.rs').write_text(representation)
 lib=out/'src/lib.rs';lib.write_text(lib.read_text().replace('#![deny(missing_docs)]','#![allow(missing_docs)]')+'\n#[allow(dead_code)]\npub mod representation;\n')
 teacher=(ROOT/'tools/plc_phase_teacher.rs').read_text().replace('b"SPLT\\x01\\0\\0\\0"','b"SPLE\\x02\\0\\0\\0"')
 teacher=teacher.replace('let mut x=[0.;80];','let mut x=[0.;130];')
 teacher=teacher.replace('let (_,record)=decoder.conceal_teacher(&mut frame,&targets[index+step][..n],snacka::representation::Kind::PhaseWide,16,20261022,&mut x)?;','decoder.conceal_energy_teacher(&mut frame,&targets[index+step][..n],&mut x)?;')
 teacher=teacher.replace('                floats(&mut out,&record.gains.map(|v|v as f32))?;\n                floats(&mut out,&[record.phase,if record.eligible {1.}else{0.}])?;','')
 (out/'examples/energy_teacher.rs').write_text(teacher)
 subprocess.run(['cargo','fmt'],cwd=out,check=True)
 subprocess.run(['cargo','build','--offline','--release','--target-dir',str(out/'target'),'--example','energy_teacher'],cwd=out,check=True)
 receipt={str(f.relative_to(out)):hashlib.sha256(f.read_bytes()).hexdigest() for f in sorted((out/'src').rglob('*.rs'))}
 (a.output/'teacher-receipt.json').write_text(json.dumps(receipt,indent=2)+'\n')
if __name__=='__main__':main()
