#!/usr/bin/env bash
# A bootstrap stages verified feature archives and invokes train_neural_plc.py.
# The GPU is released even when staging, training, or retrieval fails.
set -euo pipefail
session_name=${1:?usage: run_colab_plc_training.sh session bootstrap.py output_dir}
bootstrap=${2:?bootstrap.py required}
output_dir=${3:?output directory required}
test -f "$bootstrap"
mkdir -p "$output_dir"
cleanup() {
  colab stop -s "$session_name" || true
}
trap cleanup EXIT
trap 'exit 130' INT TERM
colab new -s "$session_name" --gpu L4
python3 - "$session_name" "$bootstrap" "$output_dir" <<'PY'
import json, pathlib, subprocess, sys, tempfile, time
session_name, bootstrap, output_dir = sys.argv[1:]
with tempfile.TemporaryDirectory(prefix="snacka-colab-ready-") as temporary:
    probe = pathlib.Path(temporary)/"ready.py"
    probe.write_text('import torch\nassert torch.cuda.is_available()\nprint("GPU ready:",torch.cuda.get_device_name(0))\n')
    for attempt in range(4):
        time.sleep(5 + attempt*5)
        try:
            result = subprocess.run(["colab","exec","-s",session_name,"-f",str(probe),"--timeout","30"],
                                    capture_output=True,text=True,timeout=45)
        except subprocess.TimeoutExpired:
            print("Kernel readiness timed out; retrying",flush=True)
            continue
        if result.returncode == 0:
            print(result.stdout,flush=True)
            break
        print("Kernel not ready; retrying",flush=True)
    else:
        raise RuntimeError("GPU kernel did not become ready")
    # The bootstrap launches a detached worker and returns immediately. A
    # transient WebSocket disconnect cannot discard the training process.
    subprocess.run(["colab","exec","-s",session_name,"-f",bootstrap,"--timeout","60"],check=True,timeout=90)
    status_path = pathlib.Path(temporary)/"status.json"
    deadline = time.monotonic()+3600
    last = None
    while time.monotonic()<deadline:
        try:
            fetched = subprocess.run(["colab","download","-s",session_name,
                         "/content/snacka-plc-training/status.json",str(status_path)],
                         capture_output=True,text=True,timeout=30)
        except subprocess.TimeoutExpired:
            time.sleep(5)
            continue
        if fetched.returncode == 0:
            status=json.loads(status_path.read_text())
            if status!=last:
                print("Training status:",status,flush=True);last=status
            if status["state"]=="complete":break
            if status["state"]=="failed":raise RuntimeError("Remote trainer failed; inspect train.log")
        time.sleep(5)
    else:
        raise TimeoutError("Remote trainer exceeded one hour")
PY
for name in neural_plc_weights.rs parity.json training-report.json checkpoint.pt; do
  colab download -s "$session_name" "/content/snacka-plc-training/result/$name" "$output_dir/$name"
done
