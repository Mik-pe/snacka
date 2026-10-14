import csv
import hashlib
import io
import json
import os
from pathlib import Path
import platform
import statistics
import subprocess
import sys

roots = {'baseline': Path('/workspace/snacka-next-baseline'), 'current': Path('/workspace/snacka')}
destination = Path('/workspace/snacka-next-encoder-results')
destination.mkdir(exist_ok=True)
sys.path.insert(0, str(roots['current'] / 'tools/reference'))
from benchmark_opus_matrix import sources

def snapshot(root):
    result = sources(root)
    for path in sorted((root / 'benches').rglob('*.rs')):
        result[str(path.relative_to(root))] = hashlib.sha256(path.read_bytes()).hexdigest()
    return result

initial = {role: snapshot(root) for role, root in roots.items()}
binaries = {role: root / 'target/release/deps/opus-238e56d6c6e4d89d' for role, root in roots.items()}
environment = {**os.environ, 'LD_LIBRARY_PATH': '/workspace/snacka-reference-build',
               'SNACKA_BENCH_CPU_LABEL': 'AMD_EPYC_9V74'}
all_rows = []
for repetition in range(3):
    for role in (('baseline', 'current') if repetition % 2 == 0 else ('current', 'baseline')):
        result = subprocess.run([str(binaries[role]), '--encoder-all'], env=environment,
                                text=True, capture_output=True, check=True)
        (destination / f'{role}-{repetition}.csv').write_text(result.stdout)
        (destination / f'{role}-{repetition}.log').write_text(result.stderr)
        rows = list(csv.DictReader(io.StringIO(result.stdout)))
        owned = [row for row in rows if row['codec'] == 'snacka']
        assert len(owned) == 88
        all_rows.extend({'role': role, 'repetition': repetition, **row} for row in owned)
        print(role, repetition, 'completed', len(owned), 'encoder cases', flush=True)
assert all(snapshot(root) == initial[role] for role, root in roots.items())
key_fields = ('wire_mode', 'bandwidth', 'frame_ms', 'channels', 'packet_bytes', 'signal')
grouped = {}
for row in all_rows:
    key = tuple(row[field] for field in key_fields)
    grouped.setdefault(key, {})[(row['role'], row['repetition'])] = row
summary_rows = []
for key, group in grouped.items():
    changes = []
    for repetition in range(3):
        a = float(group[('baseline', repetition)]['paired_encode_ratio_p50'])
        b = float(group[('current', repetition)]['paired_encode_ratio_p50'])
        changes.append(100 * (b / a - 1))
    row = dict(zip(key_fields, key))
    row['p50_change_percent'] = statistics.median(changes)
    row['change_min_percent'] = min(changes)
    row['change_max_percent'] = max(changes)
    for role in roots:
        for percentile in (50, 95, 99):
            row[f'{role}_p{percentile}_ratio'] = statistics.median(
                float(group[(role, repetition)][f'paired_encode_ratio_p{percentile}']) for repetition in range(3))
    summary_rows.append(row)
with (destination / 'summary.csv').open('w', newline='') as output:
    writer = csv.DictWriter(output, fieldnames=list(summary_rows[0]))
    writer.writeheader()
    writer.writerows(summary_rows)
metadata = {
    'baseline_commit': 'bca7700', 'platform': platform.platform(),
    'machine': platform.machine(), 'affinity': sorted(os.sched_getaffinity(0)),
    'cpu': 'AMD EPYC 9V74', 'reference_version': 'libopus 1.6.1',
    'rustc': subprocess.check_output(['/workspace/setup/cargo/bin/rustc', '--version'], text=True).strip(),
    'compiler_flags': 'release/bench profile, opt-level=3, 16 codegen units, overflow checks, no LTO or native CPU flags; reference library search path only',
    'repetitions': 3, 'cases': 88, 'batches_per_case': 1000,
    'source_unchanged': True, 'sources': initial,
    'binary_sha256': {role: hashlib.sha256(path.read_bytes()).hexdigest() for role, path in binaries.items()},
    'reference_sha256': hashlib.sha256(Path('/workspace/snacka-reference-build/libopus.so').read_bytes()).hexdigest(),
    'artifacts_sha256': {path.name: hashlib.sha256(path.read_bytes()).hexdigest() for path in destination.iterdir() if path.is_file()},
}
(destination / 'source.json').write_text(json.dumps(metadata, indent=2) + '\n')
for mode in ('silk', 'hybrid', 'celt'):
    rows = [row for row in summary_rows if row['wire_mode'] == mode]
    print(mode, 'cases', len(rows), 'median p50 change', round(statistics.median(row['p50_change_percent'] for row in rows), 3),
          'median current/libopus', round(statistics.median(row['current_p50_ratio'] for row in rows), 3), flush=True)
