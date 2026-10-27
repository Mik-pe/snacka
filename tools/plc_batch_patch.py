# SPDX-License-Identifier: MIT
"""Apply only to the exact source-registered scalar pitch implementation."""
import hashlib,subprocess
BASE_SHA="bdcb137329dbdd97d0e29144a8659e906b44754fc310b4667ef14e236d1b52b6"
def apply(path,root):
    assert hashlib.sha256(path.read_bytes()).hexdigest()==BASE_SHA
    patch=(root/'tools/plc_correlation_batch.patch').read_bytes()
    subprocess.run(['patch','--batch','--forward',str(path)],input=patch,check=True,capture_output=True)
    path.write_text(path.read_text()+'\n'+(root/'tools/plc_correlation_batch.rs').read_text())
