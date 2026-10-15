# macOS optional-feature reference

This is a separately built Apple M5/macOS 27 libopus 1.6.1 test oracle. It adds
no production dependency or model data to Snacka. The exact compiler,
configuration, header/library hashes and notice inventory are in
[macos-build.json](macos-build.json); public-API execution and wire observations
are in [macos-packets.json](macos-packets.json).

The [official release tarball](https://downloads.xiph.org/releases/opus/opus-1.6.1.tar.gz)
has SHA-256 `6ffcb593207be92584df15b32466ed64bbec99109f007c82205f0194572411a1`.
Extract it outside the repository, create separate `build` and `reference`
directories, and run from `build` (replace the prefix with an absolute path):

```sh
env CC=clang CFLAGS=-O3 ../opus-1.6.1/configure \
  --prefix=/absolute/path/reference --enable-shared --disable-static \
  --disable-doc --disable-extra-programs \
  --enable-dred --enable-osce --enable-qext
make -j4
make install
```

From the Snacka repository, exercise the public API and owned framing inspector:

```sh
python3 tools/reference/probe_opus_extensions.py \
  /absolute/path/reference/lib/libopus.0.dylib \
  --output /absolute/path/outside-repo/probe
```

The probe uses [public Opus 1.6 APIs](https://opus-codec.org/docs/opus_api-1.6.pdf).
Its standalone safe Rust inspector uses Snacka's padding parser derived from
[extension framing draft -06](https://www.ietf.org/archive/id/draft-ietf-mlcodec-opus-extension-06.html).
Packet records and temporary executables stay outside the repository; the
committed evidence contains hashes and short payload prefixes, without PCM or
model weights.

Observed DRED packets use ID 126 and prefix `44 0c` (ASCII `D`, version 12).
The probe actually parses/processes redundancy, recovers an omitted 20-ms packet
and resumes normal decoding. QEXT uses ID 124 at both 48 and 96 kHz; decoding
with extensions enabled differs from the ignore-extensions control. The same
SILK-WB packets decoded with BWE disabled/enabled also differ. Separately,
malformed and unknown padding leaves base decoded PCM identical, with the owned
parser distinguishing the malformed cases.

These observations pin this released tarball and build. They do not establish
equivalence to a different draft/model artifact, owned DRED/QEXT/OSCE support,
perceptual quality, conformance-vector acceptance or performance parity.
