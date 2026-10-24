#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Rebuild a study application from its bound source commit, not evolving main.

Usage matches build_plc_energy_candidate.py. Only the verified test correction
is overlaid on the frozen pretraining tree. No current production source enters
the experimental model build, including later gain-loader format guards.
"""
import hashlib,io,json,shutil,subprocess,sys,tarfile,tempfile
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
def main():
 binding=json.loads((ROOT/'docs/neural-plc-target-energy-data-binding.json').read_text())
 archive=subprocess.check_output(['git','archive',binding['source_commit'],'src','tests','tools','examples'],cwd=ROOT)
 with tempfile.TemporaryDirectory(prefix='snacka-energy-frozen-source-') as temporary:
  root=Path(temporary)
  with tarfile.open(fileobj=io.BytesIO(archive)) as t:t.extractall(root,filter='data')
  for name,digest in binding['source_sha256'].items():
   path=root/name
   assert path.is_file() and hashlib.sha256(path.read_bytes()).hexdigest()==digest,name
  shutil.copyfile(ROOT/'tools/build_plc_energy_checked.py',root/'tools/build_plc_energy_checked.py')
  subprocess.run([sys.executable,str(root/'tools/build_plc_energy_checked.py'),*sys.argv[1:]],check=True)
if __name__=='__main__':main()
