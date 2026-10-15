# SPDX-License-Identifier: MIT
"""Freeze the recorded decoder sources, build production bridges, and generate inputs.

Requires SNACKA_BENCH_CHECKOUT, SNACKA_BENCH_ARTIFACTS (new directory), and
SNACKA_RUST_TOOLCHAIN_BIN. SNACKA_REFERENCE_LIBRARY defaults to Homebrew libopus.
This builds and generates packets; it does not run timings.
"""
from pathlib import Path
import argparse
import hashlib
import io
import json
import os
import subprocess
import sys
import tarfile

argparse.ArgumentParser(description=__doc__).parse_args()
repo = Path(os.environ['SNACKA_BENCH_CHECKOUT']).resolve()
root = Path(os.environ['SNACKA_BENCH_ARTIFACTS']).resolve()
toolchain = Path(os.environ['SNACKA_RUST_TOOLCHAIN_BIN']).resolve()
reference = Path(os.environ.get('SNACKA_REFERENCE_LIBRARY', '/opt/homebrew/lib/libopus.dylib')).resolve()
base = 'cca87dd757d96623b91bff81d34a021e5a3e2792'
candidate = 'c9707899a71469343efa4476617a91ab06ba0e2d'
root.mkdir(exist_ok=False)
def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
bridge = root / 'benchmark_opus_decode.rs'
bridge.write_bytes(subprocess.check_output(['git', 'show', f'{base}:tools/reference/benchmark_opus_decode.rs'], cwd=repo))
environment = dict(os.environ)
for key in list(environment):
    if key.startswith('CARGO_PROFILE_RELEASE_') or key.endswith('_RUSTFLAGS') or key in ('RUSTFLAGS', 'RUSTC_BOOTSTRAP'):
        environment.pop(key)
environment.update({'RUSTC': str(toolchain/'rustc'), 'RUSTDOC': str(toolchain/'rustdoc'),
                    'RUSTC_WRAPPER': '', 'RUSTC_WORKSPACE_WRAPPER': '',
                    'CARGO_INCREMENTAL': '0', 'CARGO_BUILD_INCREMENTAL': 'false',
                    'CARGO_PROFILE_RELEASE_CODEGEN_UNITS': '16', 'CARGO_PROFILE_RELEASE_LTO': 'false',
                    'RUSTFLAGS': '', 'CARGO_ENCODED_RUSTFLAGS': ''})
keys = ['RUSTC', 'RUSTDOC', 'RUSTC_WRAPPER', 'RUSTC_WORKSPACE_WRAPPER', 'RUSTC_BOOTSTRAP',
        'CARGO_INCREMENTAL', 'CARGO_BUILD_INCREMENTAL', 'CARGO_PROFILE_RELEASE_CODEGEN_UNITS',
        'CARGO_PROFILE_RELEASE_LTO', 'CARGO_PROFILE_RELEASE_DEBUG', 'CARGO_PROFILE_RELEASE_OPT_LEVEL',
        'CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS', 'CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS',
        'CARGO_PROFILE_RELEASE_PANIC', 'CARGO_PROFILE_RELEASE_STRIP', 'RUSTFLAGS', 'CARGO_ENCODED_RUSTFLAGS',
        'CARGO_BUILD_RUSTFLAGS', 'CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS', 'CARGO_BUILD_TARGET',
        'MACOSX_DEPLOYMENT_TARGET', 'SDKROOT', 'CC', 'CXX', 'CFLAGS', 'CXXFLAGS', 'LDFLAGS', 'SNACKA_BENCH_QOS']
manifest = {'base_commit': base, 'candidate_commit': candidate,
            'difference': 'Only src/opus/silk/resample.rs differs between source snapshots.',
            'environment': {key: environment.get(key) for key in keys},
            'effective_profile': {'opt_level': 3, 'debug': 0, 'codegen_units': 16, 'lto': False,
                                  'incremental': False, 'overflow_checks': True, 'target_cpu': 'Rust target default'},
            'bridge_source': str(bridge), 'bridge_sha256': sha(bridge),
            'reference_path': str(reference), 'reference_real_path': str(reference), 'reference_sha256': sha(reference),
            'cargo_version': subprocess.check_output([str(toolchain/'cargo'), '-Vv'], text=True, env=environment),
            'rustc_version': subprocess.check_output([str(toolchain/'rustc'), '-Vv'], text=True, env=environment),
            'tool_sha256': {str(toolchain/name): sha(toolchain/name) for name in ['cargo', 'rustc', 'rustdoc']},
            'variants': {}}
assert manifest['rustc_version'].startswith('rustc 1.99.0 '), 'Use the recorded Rust release.'
for role, commit in [('before', base), ('after', candidate)]:
    source = root / role
    source.mkdir()
    archive = subprocess.check_output(['git', 'archive', commit], cwd=repo)
    with tarfile.open(fileobj=io.BytesIO(archive)) as files:
        files.extractall(source, filter='data')
    paths = [source/'Cargo.toml', source/'Cargo.lock', *sorted((source/'src').rglob('*'))]
    hashes = {str(path.relative_to(source)): sha(path) for path in paths if path.is_file()}
    target = root / f'target-{role}'
    command = [str(toolchain/'cargo'), 'build', '--locked', '--release', '--features', 'opus', '--lib', '--target-dir', str(target)]
    subprocess.run(command, cwd=source, env=environment, check=True)
    binary = root / f'benchmark-{role}'
    bridge_command = [str(toolchain/'rustc'), '--edition=2024', '-O', '-C', 'codegen-units=16',
                      '-C', 'lto=off', '-C', 'debuginfo=0', '-C', 'overflow-checks=yes', str(bridge),
                      '--extern', f'snacka={target}/release/libsnacka.rlib',
                      '-L', f'dependency={target}/release/deps', '-L', f'native={reference.parent}', '-o', str(binary)]
    subprocess.run(bridge_command, cwd=source, env=environment, check=True)
    manifest['variants'][role] = {'source': str(source), 'sources': hashes, 'target': str(target),
                                 'library_command': command, 'library_sha256': sha(target/'release/libsnacka.rlib'),
                                 'bridge_command': bridge_command, 'binary': str(binary), 'binary_sha256': sha(binary)}
assert manifest['variants']['before']['sources'].keys() == manifest['variants']['after']['sources'].keys()
changed = [name for name, expected in manifest['variants']['before']['sources'].items()
           if manifest['variants']['after']['sources'][name] != expected]
assert changed == ['src/opus/silk/resample.rs'], changed
manifest['changed_files'] = changed
sys.path.insert(0, str(root/'before/tools/reference'))
from benchmark_opus_matrix import generate, geometries
from check_opus_rtp import library
lib = library(reference)
assert lib.opus_get_version_string().decode() == 'libopus 1.6.1'
streams = root/'streams'
streams.mkdir()
expected = json.loads((Path(__file__).parent/'equivalence.json').read_text())['stream_sha256']
for mode, band, duration in geometries():
    for channels in (1, 2):
        path = streams/f'{mode}-{band}-{duration}ms-{channels}ch.bit'
        generate(lib, mode, band, duration, channels, 24, path)
        assert sha(path) == expected[path.name], ('Reference build generated different packets', path.name)
manifest['stream_sha256'] = {path.name: sha(path) for path in sorted(streams.glob('*.bit'))}
assert len(manifest['stream_sha256']) == 142
manifest['runner_sha256'] = sha(Path(__file__))
(root/'source.json').write_text(json.dumps(manifest, indent=2)+'\n')
print('Built two frozen sources and verified all 142 input hashes; no timing run.', flush=True)
