# SPDX-License-Identifier: MIT
"""Verify owned SILK packets through an independently installed libopus 1.6.1.

The Rust probe constructs 1200 native-rate packets across every SILK bandwidth,
duration and channel geometry. The optional oracle checks packet sample counts,
final arithmetic intervals and signal distortion with the specified layer
delay plus the reference's native-rate pass-through resampler delay.
"""

import argparse
import ctypes
import math
from pathlib import Path
import struct
import subprocess
import sys
import tempfile

from generate_entropy_fixtures import configure
from generate_opus_silk_decode_fixtures import configure_packets


def verify(library, data):
    if data[:8] != b"SNKSEC01":
        raise RuntimeError("invalid owned SILK encoder probe")
    cases, = struct.unpack_from("<I", data, 8)
    cursor, current, decoder = 12, None, None
    errors, energies = [0.0] * 24, [0.0] * 24
    try:
        for case in range(cases):
            stream, band, duration, channels, length, samples, expected_range = struct.unpack_from("<HBBBHHI", data, cursor)
            cursor += 13
            if stream != current:
                if decoder:
                    library.oracle_opus_decoder_destroy(decoder)
                rate = (8000, 12000, 16000)[band]
                error = ctypes.c_int()
                decoder = library.oracle_opus_decoder_create(rate, channels, ctypes.byref(error))
                if error.value or not decoder:
                    raise RuntimeError("oracle decoder creation failed")
                # Reference same-rate delay plus SILK's one-sample unmix delay.
                delay = (5, 10, 13)[band] * channels
                history, position = [0.0] * delay, 0
                current = stream
            packet = ctypes.create_string_buffer(data[cursor:cursor + length])
            cursor += length
            pcm = (ctypes.c_int16 * (samples * channels))()
            produced = library.oracle_opus_decode(decoder, packet, length, pcm, samples)
            interval = ctypes.c_uint32()
            status = library.oracle_opus_decoder_range(decoder, ctypes.byref(interval))
            if produced != samples or status or interval.value != expected_range:
                raise RuntimeError(f"case {case}: count/range mismatch {(produced, interval.value)} != {(samples, expected_range)}")
            source = struct.unpack_from("<" + "f" * (samples * channels), data, cursor)
            cursor += samples * channels * 4
            for original, actual in zip(source, pcm):
                expected = history[position]
                history[position] = original
                position = (position + 1) % delay
                difference = actual / 32768.0 - expected
                errors[stream] += difference * difference
                energies[stream] += expected * expected
        if cursor != len(data):
            raise RuntimeError("trailing probe data")
    finally:
        if decoder:
            library.oracle_opus_decoder_destroy(decoder)
    scores = [10 * math.log10(energy / error) for energy, error in zip(energies, errors)]
    if min(scores) <= 20:
        raise RuntimeError(f"synthetic waveform distortion exceeded gate: {scores}")
    print(f"libopus 1.6.1 decoded {cases} owned SILK packets, all ranges exact, minimum SNR {min(scores):.2f} dB")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path, help="separately installed libopus 1.6.1 static archive")
    parser.add_argument("--packets", type=int, default=50, help="packets per independent geometry stream")
    args = parser.parse_args()
    with tempfile.TemporaryDirectory(prefix="snacka-owned-silk-encoder-") as temporary:
        directory = Path(temporary)
        library_path, executable = directory / "oracle.so", directory / "probe"
        flag = "-dynamiclib" if sys.platform == "darwin" else "-shared"
        subprocess.run(["cc", flag, "-fPIC", str(Path(__file__).with_name("entropy_bridge.c")),
                        str(Path(__file__).with_name("silk_packet_bridge.c")), str(args.archive.resolve()),
                        "-lm", "-o", str(library_path)], check=True)
        library = ctypes.CDLL(str(library_path))
        configure(library)
        configure_packets(library)
        subprocess.run(["rustc", "-O", "--edition", "2024", "--cfg", 'feature="opus"',
                        "-A", "dead_code", "-A", "unused_imports",
                        str(Path(__file__).with_name("silk_encode_probe.rs")), "-o", str(executable)], check=True)
        result = subprocess.run([str(executable), str(args.packets)], capture_output=True)
        if result.returncode:
            raise RuntimeError(result.stderr.decode())
        verify(library, result.stdout)


if __name__ == "__main__":
    main()
