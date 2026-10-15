#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
from pathlib import Path
import argparse, hashlib, io, json, os, shutil, subprocess, tarfile

parser=argparse.ArgumentParser(description="Rebuild the recorded macOS encoder before/after libraries and standalone probes.")
parser.add_argument('--checkout',default=os.environ.get('SNACKA_BENCH_CHECKOUT',str(Path(__file__).resolve().parents[3])))
parser.add_argument('--artifacts',default=os.environ.get('SNACKA_BENCH_ARTIFACTS'))
parser.add_argument('--toolchain-bin',default=os.environ.get('SNACKA_RUST_TOOLCHAIN_BIN',str(Path.home()/'.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin')))
parser.add_argument('--reference-library',default=os.environ.get('SNACKA_REFERENCE_LIBRARY','/opt/homebrew/lib/libopus.dylib'))
args=parser.parse_args()
if not args.artifacts:
    parser.error('set --artifacts or SNACKA_BENCH_ARTIFACTS to a fresh directory outside the checkout')
repo=Path(args.checkout).resolve()
root=Path(args.artifacts).resolve()
if root==repo or repo in root.parents or (root.exists() and any(root.iterdir())):
    parser.error('artifact directory must be fresh and outside the checkout')
root.mkdir(parents=True,exist_ok=True)
toolchain=Path(args.toolchain_bin).resolve()
reference=Path(args.reference_library).resolve()
assert reference.is_file() and Path(args.reference_library).name=='libopus.dylib'
base='5fc1b43d535d7d8277be456a6ea8fb15c473f916'
after='552d0c4380ba77b00b2e4b548dce34fc14f34815'
commits={'before':base,'after':after}
proof_source=root/'exact-probe.rs'
shutil.copyfile(Path(__file__).with_name('exact-probe.rs'),proof_source)
env = dict(os.environ)
for key in list(env):
    if key.startswith('CARGO_PROFILE_RELEASE_') or key.endswith('_RUSTFLAGS') or key in ('RUSTFLAGS', 'RUSTC_BOOTSTRAP'):
        env.pop(key)
env.update({'RUSTC': str(toolchain/'rustc'), 'RUSTDOC': str(toolchain/'rustdoc'),
            'RUSTC_WRAPPER': '', 'RUSTC_WORKSPACE_WRAPPER': '',
            'CARGO_INCREMENTAL': '0', 'CARGO_BUILD_INCREMENTAL': 'false',
            'CARGO_PROFILE_RELEASE_CODEGEN_UNITS': '16', 'CARGO_PROFILE_RELEASE_LTO': 'false',
            'RUSTFLAGS': '', 'CARGO_ENCODED_RUSTFLAGS': ''})
keys = ['RUSTC','RUSTDOC','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','RUSTC_BOOTSTRAP',
        'CARGO_INCREMENTAL','CARGO_BUILD_INCREMENTAL','CARGO_PROFILE_RELEASE_CODEGEN_UNITS',
        'CARGO_PROFILE_RELEASE_LTO','CARGO_PROFILE_RELEASE_DEBUG','CARGO_PROFILE_RELEASE_OPT_LEVEL',
        'CARGO_PROFILE_RELEASE_OVERFLOW_CHECKS','CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS',
        'CARGO_PROFILE_RELEASE_PANIC','CARGO_PROFILE_RELEASE_STRIP','RUSTFLAGS','CARGO_ENCODED_RUSTFLAGS',
        'CARGO_BUILD_RUSTFLAGS','CARGO_TARGET_AARCH64_APPLE_DARWIN_RUSTFLAGS','CARGO_BUILD_TARGET',
        'MACOSX_DEPLOYMENT_TARGET','SDKROOT','CC','CXX','CFLAGS','CXXFLAGS','LDFLAGS','SNACKA_BENCH_QOS']
def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()
manifest = {'base_commit': base,
            'difference': 'after is commit552d0c4; only src/opus/silk/quantize.rs differs from base5fc1b43',
            'environment': {key: env.get(key) for key in keys},
            'profile': {'opt_level':3, 'debug':0, 'codegen_units':16,'lto':False,
                        'incremental':False,'overflow_checks':True,'target_cpu':'target default'},
            'cargo_version':subprocess.check_output([str(toolchain/'cargo'),'-Vv'],text=True,env=env),
            'rustc_version':subprocess.check_output([str(toolchain/'rustc'),'-Vv'],text=True,env=env),
            'variants':{}}
# A single shared standalone bridge adds the same public pthread QoS request
# used by the decoder benchmark. The benchmark logic and input generator are
# otherwise copied from the frozen source without edits.
bridge=root/'encoder-benchmark.rs'
bridge_module=root/'encoder-benchmark-module.rs'
bridge_module.write_bytes(subprocess.check_output(['git','show',f'{base}:benches/opus/encoder.rs'],cwd=repo))
bridge_text=subprocess.check_output(['git','show',f'{base}:benches/opus.rs'],cwd=repo,text=True)
bridge_text=bridge_text.replace('#[path = "opus/encoder.rs"]',f'#[path = "{bridge_module}"]')
qos='''fn main() {
    #[cfg(target_os = "macos")]
    if let Ok(qos) = std::env::var("SNACKA_BENCH_QOS") {
        assert_eq!(qos, "user-initiated", "unsupported benchmark QoS");
        unsafe extern "C" {
            fn pthread_set_qos_class_self_np(class: u32, priority: std::ffi::c_int) -> std::ffi::c_int;
        }
        // SAFETY: Apple's public pthread/qos.h accepts QOS_CLASS_USER_INITIATED
        // (0x19, sys/qos.h) and relative priority zero for the calling thread.
        assert_eq!(unsafe { pthread_set_qos_class_self_np(0x19, 0) }, 0);
    }
'''
assert bridge_text.count('fn main() {') == 1
bridge.write_text(bridge_text.replace('fn main() {',qos))
manifest['bridge_sources']={str(bridge):sha(bridge),str(bridge_module):sha(bridge_module)}
for role in ['before','after']:
    source = root/role
    source.mkdir(exist_ok=True)
    archive=subprocess.check_output(['git','archive',commits[role]],cwd=repo)
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        tar.extractall(source, filter='data')
    hashes = {str(path.relative_to(source)): sha(path) for path in sorted((source/'src').rglob('*.rs'))}
    for path in [source/'Cargo.toml',source/'Cargo.lock',source/'benches/opus.rs',source/'benches/opus/encoder.rs']:
        hashes[str(path.relative_to(source))] = sha(path)
    target = root/f'target-{role}'
    command=[str(toolchain/'cargo'),'build','--locked','--release','--features','opus','--lib','--target-dir',str(target)]
    subprocess.run(command,cwd=source,env=env,check=True)
    row={'source':str(source),'sources':hashes,'library_command':command,'target':str(target),
         'library_sha256':sha(target/'release/libsnacka.rlib')}
    # The reference is separately installed; these standalone benchmark tools
    # do not change the production dependency-free Rust codec.
    binary=root/f'benchmark-{role}'
    bridge_command=[str(toolchain/'rustc'),'--edition=2024','-O','-C','codegen-units=16',
                    '-C','lto=off','-C','debuginfo=0','-C','overflow-checks=yes',
                    '--cfg','feature="reference-tests"',str(bridge),
                    '--extern',f'snacka={target}/release/libsnacka.rlib',
                    '-L',f'dependency={target}/release/deps','-L',f'native={Path(args.reference_library).absolute().parent}','-o',str(binary)]
    subprocess.run(bridge_command,cwd=source,env=env,check=True)
    row.update({'benchmark':str(binary),'benchmark_sha256':sha(binary),'benchmark_command':bridge_command})
    proof_binary=root/f'probe-{role}'
    proof_command=[str(toolchain/'rustc'),'--edition=2024','-O','-C','codegen-units=16',
                   '-C','lto=off','-C','debuginfo=0','-C','overflow-checks=yes',str(proof_source),
                   '--extern',f'snacka={target}/release/libsnacka.rlib',
                   '-L',f'dependency={target}/release/deps','-o',str(proof_binary)]
    subprocess.run(proof_command,cwd=source,env=env,check=True)
    row.update({'commit':commits[role],'probe_command':proof_command,'probe_sha256':sha(proof_binary)})
    manifest['variants'][role]=row
    print(role,row['library_sha256'],row['benchmark_sha256'],flush=True)
changed=[path for path in manifest['variants']['before']['sources']
         if manifest['variants']['before']['sources'][path] != manifest['variants']['after']['sources'][path]]
assert changed == ['src/opus/silk/quantize.rs'], changed
manifest['changed_files']=changed
manifest['runner_sha256']=sha(Path(__file__))
manifest['reference_sha256']=sha(reference)
manifest['reference_path']=str(Path(args.reference_library).absolute())
manifest['reference_real_path']=str(reference)
manifest['tool_sha256']={str(toolchain/name):sha(toolchain/name) for name in ['cargo','rustc','rustdoc']}
manifest['proof_source']=str(proof_source)
manifest['proof_source_sha256']=sha(proof_source)
(root/'source.json').write_text(json.dumps(manifest,indent=2)+'\n')
