from pathlib import Path
import ctypes as c
import hashlib, io, json, math, os, subprocess, tarfile

root = Path(__file__).resolve().parent
repo = Path('/tmp/snacka-opus-decoder-gain-20261015')
base = 'd2e420edd388bfd7e8925401da012ed238fe0bf8'
toolchain = Path('/Users/mikpe/.rustup/toolchains/1.99.0-aarch64-apple-darwin/bin')
reference = Path('/opt/homebrew/lib/libopus.dylib').resolve()
def sha(path): return hashlib.sha256(path.read_bytes()).hexdigest()
lib = c.CDLL(str(reference))
lib.opus_encoder_create.argtypes = [c.c_int, c.c_int, c.c_int, c.POINTER(c.c_int)]
lib.opus_encoder_create.restype = c.c_void_p
lib.opus_encoder_ctl.argtypes = [c.c_void_p, c.c_int]
lib.opus_encoder_ctl.restype = c.c_int
lib.opus_encoder_destroy.argtypes = [c.c_void_p]
lib.opus_encode_float.argtypes = [c.c_void_p, c.POINTER(c.c_float), c.c_int, c.POINTER(c.c_ubyte), c.c_int]
lib.opus_encode_float.restype = c.c_int
lib.opus_get_version_string.restype = c.c_char_p
inputs = []
for mode, app, band, bitrate, fec in [('silk',2048,1103,24000,0),('hybrid',2048,1105,24000,0),('celt',2051,1105,64000,0),('silk-fec',2048,1103,24000,1)]:
    for channels in [1,2]:
        error = c.c_int()
        state = lib.opus_encoder_create(48000,channels,app,c.byref(error))
        assert state and error.value == 0
        for ctl,value in [(4002,bitrate*channels),(4006,0),(4022,channels),(4004,band),(4008,band),(4024,3001),(4012,fec),(4014,30 if fec else 0)]:
            assert lib.opus_encoder_ctl(state,ctl,c.c_int(value)) == 0
        packets = bytearray()
        for frame in range(12):
            values = []
            for index in range(960*channels):
                t = (frame*960+index//channels)/48000
                pitch = 137+29*math.sin(2*math.pi*1.3*t)
                phase = 2*math.pi*pitch*t+(index%channels)*.4
                values.append(.16*math.sin(phase)+.055*math.sin(2*phase)+.024*math.sin(7*phase))
            pcm = (c.c_float*len(values))(*values)
            output = (c.c_ubyte*8192)()
            count = lib.opus_encode_float(state,pcm,960,output,8192)
            assert count > 0
            packets.extend(count.to_bytes(4,'little'))
            packets.extend(bytes(output[:count]))
        lib.opus_encoder_destroy(state)
        path = root/f'{mode}-{channels}.packets'
        path.write_bytes(packets)
        inputs.append({'path':str(path),'sha256':sha(path),'mode':mode,'coded_channels':channels,'frames':12,'frame_samples':960,'bitrate':bitrate*channels})
environment = dict(os.environ)
for key in list(environment):
    if key.startswith('CARGO_PROFILE_RELEASE_') or key.endswith('_RUSTFLAGS') or key in ('RUSTFLAGS','RUSTC_BOOTSTRAP'):
        environment.pop(key)
environment.update({'RUSTC':str(toolchain/'rustc'),'RUSTDOC':str(toolchain/'rustdoc'),'RUSTC_WRAPPER':'','RUSTC_WORKSPACE_WRAPPER':'','CARGO_INCREMENTAL':'0','CARGO_BUILD_INCREMENTAL':'false','RUSTFLAGS':'','CARGO_ENCODED_RUSTFLAGS':''})
archive = subprocess.check_output(['git','archive',base],cwd=repo)
manifest = {'base_commit':base,'scope':'Untimed generated public native packets. No clocks, warmups, calibration, quality corpus or timing. Default gain only; compare all PCM bits, ranges, counts and untouched suffix across source variants.','reference':{'path':str(reference),'sha256':sha(reference),'version':lib.opus_get_version_string().decode()},'inputs':inputs,'driver_sha256':sha(root/'gain_zero_proof.rs'),'runner_sha256':sha(Path(__file__)),'rustc':subprocess.check_output([str(toolchain/'rustc'),'-Vv'],text=True),'variants':{}}
for role in ['before','after']:
    source = root/role
    source.mkdir(exist_ok=False)
    with tarfile.open(fileobj=io.BytesIO(archive)) as files:
        files.extractall(source,filter='data')
    if role == 'after':
        (source/'src/opus/decoder.rs').write_bytes((repo/'src/opus/decoder.rs').read_bytes())
    sources = {str(path.relative_to(source)):sha(path) for path in [source/'Cargo.toml',source/'Cargo.lock',*sorted((source/'src').rglob('*'))] if path.is_file()}
    target = root/f'target-{role}'
    command = [str(toolchain/'cargo'),'build','--locked','--release','--features','opus','--lib','--target-dir',str(target)]
    subprocess.run(command,cwd=source,env=environment,check=True)
    binary = root/f'proof-{role}'
    bridge = [str(toolchain/'rustc'),'--edition=2024','-O','-C','overflow-checks=yes',str(root/'gain_zero_proof.rs'),'--extern',f'snacka={target}/release/libsnacka.rlib','-L',f'dependency={target}/release/deps','-o',str(binary)]
    subprocess.run(bridge,cwd=source,env=environment,check=True)
    output = subprocess.check_output([str(binary),*[entry['path'] for entry in inputs]],text=True,env=environment)
    (root/f'{role}.jsonl').write_text(output)
    manifest['variants'][role] = {'sources':sources,'library_command':command,'driver_command':bridge,'library_sha256':sha(target/'release/libsnacka.rlib'),'binary_sha256':sha(binary),'output_sha256':sha(root/f'{role}.jsonl'),'rows':len(output.splitlines())}
    print(role,len(output.splitlines()),flush=True)
before = (root/'before.jsonl').read_bytes()
after = (root/'after.jsonl').read_bytes()
assert before == after, 'PCM/range/count/recovery mismatch'
changed = [path for path,expected in manifest['variants']['before']['sources'].items() if manifest['variants']['after']['sources'][path] != expected]
assert changed == ['src/opus/decoder.rs'], changed
manifest['changed_production_files'] = changed
manifest['all_rows_bit_exact'] = True
manifest['environment'] = {key:environment.get(key) for key in ['RUSTC','RUSTDOC','RUSTC_WRAPPER','RUSTC_WORKSPACE_WRAPPER','CARGO_INCREMENTAL','CARGO_ENCODED_RUSTFLAGS','RUSTFLAGS']}
(root/'source-and-results.json').write_text(json.dumps(manifest,indent=2)+'\n')
print('all rows bit exact',flush=True)
