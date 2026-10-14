# SPDX-License-Identifier: MIT
"""Verify causal owned SILK FEC through an independently installed libopus 1.6.1.

The Rust probe emits 96 packets across 24 rate/duration/channel geometries. Every
other packet is lost; its successor must decode real FEC and then its own regular
payload. Only public Opus declarations and ABI calls enter the optional oracle.
No reference implementation is linked into Snacka or required at runtime.
"""

import argparse
import ctypes
import json
import math
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

BRIDGE = r'''
#include <stdint.h>
extern const char *opus_get_version_string(void);
extern void *opus_decoder_create(int,int,int*);
extern void opus_decoder_destroy(void*);
extern int opus_decoder_ctl(void*,int,...);
extern int opus_decode_float(void*,const unsigned char*,int,float*,int,int);
void *fec_create(int rate,int channels,int *error) { return opus_decoder_create(rate,channels,error); }
void fec_destroy(void *state) { opus_decoder_destroy(state); }
int fec_decode(void *state,const unsigned char *packet,int length,float *output,int samples,int fec) {
 return opus_decode_float(state,packet,length,output,samples,fec);
}
int fec_range(void *state,uint32_t *range) { return opus_decoder_ctl(state,4031,range); }
'''


def configure(library):
    library.opus_get_version_string.restype = ctypes.c_char_p
    if library.opus_get_version_string().decode() != "libopus 1.6.1":
        raise RuntimeError("independent decoder must be libopus 1.6.1")
    pointer, integer = ctypes.c_void_p, ctypes.c_int
    library.fec_create.argtypes = [integer, integer, ctypes.POINTER(integer)]
    library.fec_create.restype = pointer
    library.fec_destroy.argtypes = [pointer]
    library.fec_decode.argtypes = [pointer, pointer, integer, pointer, integer, integer]
    library.fec_decode.restype = integer
    library.fec_range.argtypes = [pointer, ctypes.POINTER(ctypes.c_uint32)]
    library.fec_range.restype = integer


def verify(library, data):
    if data[:8] != b"SNKFEC01":
        raise RuntimeError("invalid owned FEC probe")
    cases, = struct.unpack_from("<I", data, 8)
    cursor, decoder, current = 12, None, None
    scores = {}
    following = 0
    try:
        for case in range(cases):
            stream, band, duration, channels, index, length, samples, expected_range = struct.unpack_from("<HBBBBHHI", data, cursor)
            cursor += 14
            if stream != current:
                if decoder:
                    library.fec_destroy(decoder)
                error = ctypes.c_int()
                decoder = library.fec_create((8000, 12000, 16000)[band], channels, ctypes.byref(error))
                if error.value or not decoder:
                    raise RuntimeError("independent decoder creation failed")
                # The public decoder's native-rate pass-through resampler adds
                # 4/9/12 samples; SILK's shared one-sample delay is in both outputs.
                history, position = [0.0] * ((4, 9, 12)[band] * channels), 0
                current = stream
                scores[stream] = {"band": band, "duration_ms": duration, "channels": channels,
                                  "fec_error": 0.0, "fec_energy": 0.0,
                                  "current_error": 0.0, "current_energy": 0.0,
                                  "source_error": 0.0, "source_energy": 0.0}
                previous_source = None
            packet = ctypes.create_string_buffer(data[cursor:cursor + length])
            cursor += length
            count = samples * channels
            source, owned_fec, owned_current = [struct.unpack_from("<" + "f" * count, data, cursor + block * count * 4) for block in range(3)]
            cursor += 3 * count * 4
            if index % 2 == 0:
                previous_source = source
                continue
            for sample in range(channels, count):
                expected = previous_source[sample - channels]
                scores[stream]["source_error"] += (owned_fec[sample] - expected) ** 2
                scores[stream]["source_energy"] += expected ** 2
            for fec, owned, label in [(1, owned_fec, "fec"), (0, owned_current, "current")]:
                output = (ctypes.c_float * count)()
                produced = library.fec_decode(decoder, packet, length, output, samples, fec)
                if produced != samples:
                    raise RuntimeError(f"case {case}/{label}: decoded {produced}, expected {samples}")
                if not fec:
                    interval = ctypes.c_uint32()
                    if library.fec_range(decoder, ctypes.byref(interval)) or interval.value != expected_range:
                        raise RuntimeError(f"case {case}: following range {interval.value} != {expected_range}")
                    following += 1
                for own, native in zip(owned, output):
                    if not math.isfinite(native):
                        raise RuntimeError(f"case {case}: non-finite independent audio")
                    expected = history[position]
                    history[position] = own
                    position = (position + 1) % len(history)
                    scores[stream][label + "_error"] += (native - expected) ** 2
                    scores[stream][label + "_energy"] += expected ** 2
        if cursor != len(data):
            raise RuntimeError("trailing owned FEC probe data")
    finally:
        if decoder:
            library.fec_destroy(decoder)
    for score in scores.values():
        for label in ["fec", "current"]:
            score[label + "_snr_db"] = 10 * math.log10(score[label + "_energy"] / max(score[label + "_error"], 1e-30))
        score["fec_source_snr_db"] = 10 * math.log10(score["source_energy"] / max(score["source_error"], 1e-30))
    result = {"packets": cases, "recovered_intervals": following,
              "following_ranges_exact": following, "geometries": list(scores.values())}
    print(json.dumps(result, indent=2))
    if following != 48 or min(score[label + "_snr_db"] for score in scores.values() for label in ["fec", "current"]) <= 35:
        raise RuntimeError("native FEC/regular PCM differs from owned decoding by more than the 35 dB SNR gate")
    if min(score["fec_source_snr_db"] for score in scores.values()) <= 15:
        raise RuntimeError("owned FEC does not meet the unchanged 15 dB source reconstruction gate")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="separately installed libopus 1.6.1 static archive")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="snacka-owned-silk-fec-") as temporary:
        directory = Path(temporary)
        bridge, library_path, probe = directory / "bridge.c", directory / "oracle.so", directory / "probe"
        bridge.write_text(BRIDGE)
        flag = "-dynamiclib" if sys.platform == "darwin" else "-shared"
        subprocess.run(["cc", flag, "-fPIC", str(bridge), str(args.archive.resolve()), "-lm", "-o", str(library_path)], check=True)
        library = ctypes.CDLL(str(library_path))
        configure(library)
        subprocess.run(["rustc", "-O", "--edition", "2024", "--cfg", 'feature="opus"',
                        "-A", "dead_code", "-A", "unused_imports",
                        str(Path(__file__).with_name("silk_fec_probe.rs")), "-o", str(probe)], check=True)
        result = subprocess.run([str(probe)], check=True, capture_output=True)
        verify(library, result.stdout)


if __name__ == "__main__":
    main()
