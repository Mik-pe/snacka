# SPDX-License-Identifier: MIT
"""Reproduce Snacka's own stable SILK-to-48kHz rational interpolation model.

Development only: NumPy, a C compiler, an installed libopus 1.6.1 static archive
and its matching headers are required. No reference implementation algorithms
or coefficient tables are read or copied. The bridge makes external calls to
observe normalized impulse responses at thirteen amplitudes. A shared AR(3)
tail is fitted by least squares, then each 14-tap polyphase numerator is projected
to exact unit DC gain and RFC 6716 Table 54 delay with minimum frequency error.
The resulting coefficients are original mathematical model data under MIT.

Example (output artifacts must be outside the repository):
  python3 tools/reference/design_opus_resampler.py /path/libopus.a /path/opus-1.6.1 \
      --output-dir /tmp/snacka-resampler-design
Add --write to update the inline model and the compact reference response fixture.
There is no NumPy or foreign-code dependency in the production Rust library.
"""

import argparse
import ast
import hashlib
import json
from pathlib import Path
import re
import struct
import subprocess

import numpy as np


CASES = ((8000, 0, 4.304), (12000, 4, 8.304), (16000, 7, 11.296))
ORDER, TAPS = 3, 14
# Frequency is measured in cycles per input sample; includes passband edge,
# native Nyquist, and output images. The fixture records complex response.
FREQUENCIES = (0.0, 0.05, 0.1, 0.2, 0.3, 0.4, 0.45, 0.47, 0.5, 0.65, 0.75, 1.25)
BRIDGE = r"""
#include <stdio.h>
#include <string.h>
#include "SigProc_FIX.h"
extern const char *opus_get_version_string(void);
int main(int argc, char **argv) {
    if (argc != 2 || strcmp(opus_get_version_string(), "libopus 1.6.1")) return 1;
    FILE *file = fopen(argv[1], "wb");
    if (!file) return 2;
    for (int rate = 8000; rate <= 16000; rate += 4000) {
        for (int amplitude = 20000; amplitude <= 32000; amplitude += 1000) {
            silk_resampler_state_struct state;
            if (silk_resampler_init(&state, rate, 48000, 0)) return 3;
            short input[320] = {0}, output[960];
            input[0] = amplitude;
            fwrite(&rate, sizeof(rate), 1, file);
            fwrite(&amplitude, sizeof(amplitude), 1, file);
            for (int block = 0; block < 8; ++block) {
                if (silk_resampler(&state, output, input, rate / 50)) return 4;
                fwrite(output, sizeof(*output), 960, file);
                memset(input, 0, sizeof(input));
            }
        }
    }
    return fclose(file) != 0;
}
"""


def observations(data):
    cursor, groups = 0, {rate: [] for rate, _, _ in CASES}
    for rate, _, _ in CASES:
        for amplitude in range(20000, 32001, 1000):
            if struct.unpack_from("<ii", data, cursor) != (rate, amplitude):
                raise RuntimeError("invalid oracle impulse record")
            cursor += 8
            samples = np.frombuffer(data[cursor:cursor + 15360], dtype="<i2")
            if len(samples) != 7680:
                raise RuntimeError("truncated oracle impulse record")
            cursor += 15360
            groups[rate].append(samples.astype(np.float64) / amplitude)
    if cursor != len(data):
        raise RuntimeError("unexpected oracle impulse suffix")
    return {rate: np.mean(signals, axis=0) for rate, signals in groups.items()}


def realize(denominator, numerators, length):
    result = np.zeros((len(numerators), length))
    for index in range(length):
        if index < TAPS:
            result[:, index] = numerators[:, index]
        for lag in range(1, min(index + 1, len(denominator))):
            result[:, index] -= denominator[lag] * result[:, index - lag]
    return result


def design(responses):
    phases = []
    for rate, delay, _ in CASES:
        ratio = 48000 // rate
        for phase in range(ratio):
            phases.append(responses[rate][phase::ratio][delay:delay + 100])
    phases = np.array(phases)
    rows, wanted = [], []
    for impulse in phases:
        for index in range(TAPS, 70):
            rows.append([-impulse[index - lag] for lag in range(1, ORDER + 1)])
            wanted.append(impulse[index])
    denominator = np.r_[1, np.linalg.lstsq(np.array(rows), wanted, rcond=None)[0]]
    if np.max(np.abs(np.roots(denominator))) >= 0.848:
        raise RuntimeError("fitted model is outside the stable design bound")
    numerators = np.zeros((len(phases), TAPS))
    for phase, impulse in enumerate(phases):
        for index in range(TAPS):
            numerators[phase, index] = sum(denominator[lag] * impulse[index - lag]
                                          for lag in range(min(index + 1, ORDER + 1)))
    omega = np.linspace(0, np.pi, 4096)
    response = np.exp(-1j * omega[:, None] * np.arange(ORDER + 1)) @ denominator
    basis = np.exp(-1j * omega[:, None] * np.arange(TAPS)) / response[:, None]
    gram = (basis.conj().T @ basis).real / len(omega)
    constraints = np.stack([np.ones(TAPS), np.arange(TAPS)])
    solved = np.linalg.solve(gram, constraints.T)
    projection = solved @ np.linalg.inv(constraints @ solved)
    mass = denominator.sum()
    moment = np.dot(denominator, np.arange(ORDER + 1))
    index = 0
    for rate, delay, target in CASES:
        ratio = 48000 // rate
        for phase in range(ratio):
            desired = np.array([mass, (target - delay - phase / ratio + moment / mass) * mass])
            numerators[index] += projection @ (desired - constraints @ numerators[index])
            index += 1
    predicted = realize(denominator, numerators, 256)
    metrics = {"poles": np.roots(denominator).tolist(),
               "impulse_root_mean_square_error": float(np.sqrt(np.mean((predicted[:, :100] - phases) ** 2))),
               "maximum_phase_complex_response_error": float(np.max(np.abs(
                   np.fft.fft(phases, n=2048, axis=1) - np.fft.fft(predicted, n=2048, axis=1))))}
    families, index = [], 0
    for rate, _, _ in CASES:
        ratio = 48000 // rate
        families.append(numerators[index:index + ratio].tolist() + [[0.0] * TAPS for _ in range(6 - ratio)])
        index += ratio
    return denominator[1:].tolist(), families, metrics


def fixture(responses):
    data = bytearray(b"SNKRSF01" + struct.pack("<I", len(FREQUENCIES)))
    for rate, _, _ in CASES:
        impulse, ratio = responses[rate], 48000 // rate
        index = np.arange(len(impulse))
        for frequency in FREQUENCIES:
            response = np.dot(impulse, np.exp(-2j * np.pi * frequency / ratio * index)) / ratio
            data += struct.pack("<ddd", frequency, response.real, response.imag)
    return data


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("archive", type=Path)
    parser.add_argument("headers", type=Path)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--write", action="store_true", help="update owned model constants and fixture")
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    output = args.output_dir.resolve()
    if output == root or root in output.parents:
        parser.error("output directory must be outside the repository")
    output.mkdir(parents=True, exist_ok=True)
    source, executable, impulses = output / "oracle.c", output / "oracle", output / "impulses.bin"
    source.write_text(BRIDGE)
    subprocess.run(["cc", "-O2", "-I" + str(args.headers / "silk"),
                    "-I" + str(args.headers / "celt"), "-I" + str(args.headers / "include"),
                    str(source), str(args.archive), "-lm", "-o", str(executable)], check=True)
    subprocess.run([str(executable), str(impulses)], check=True)
    data = impulses.read_bytes()
    responses = observations(data)
    feedback, families, metrics = design(responses)
    model = "const FEEDBACK: [f64; 3] = " + repr(feedback) + ";\n\n"
    model += "const NUMERATORS: [[[f64; 14]; 6]; 3] = " + repr(families) + ";\n"
    destination = output / "model.rs"
    destination.write_text(model)
    subprocess.run(["rustfmt", "--edition", "2024", str(destination)], check=True)
    fixture_data = fixture(responses)
    (output / "resampler-response.bin").write_bytes(fixture_data)
    path = root / "src/opus/silk/resample.rs"
    original = path.read_text()
    block = original.split("// BEGIN GENERATED MODEL\n", 1)[1].split("// END GENERATED MODEL", 1)[0]
    numbers = [declaration.split("=", 1)[1].rsplit(";", 1)[0]
               for declaration in block.split("const ") if "=" in declaration]
    numbers = [re.sub(r"\[0\.0;\s*14\]", repr([0.0] * TAPS), value)
               for value in numbers]
    existing = [np.array(ast.literal_eval(value.strip())) for value in numbers]
    differences = [float(np.max(np.abs(before - after))) for before, after in zip(existing, (feedback, families))]
    if len(existing) != 2:
        raise RuntimeError("owned model markers or constants missing")
    metrics.update({"archive_sha256": hashlib.sha256(args.archive.read_bytes()).hexdigest(),
                    "observations_sha256": hashlib.sha256(data).hexdigest(),
                    "maximum_owned_coefficient_difference": max(differences),
                    "fixture_bytes": len(fixture_data)})
    (output / "report.json").write_text(json.dumps(metrics, indent=2) + "\n")
    print(json.dumps(metrics, sort_keys=True))
    if args.write:
        before, rest = original.split("// BEGIN GENERATED MODEL\n", 1)
        _, after = rest.split("// END GENERATED MODEL", 1)
        path.write_text(before + "// BEGIN GENERATED MODEL\n" + destination.read_text() + "// END GENERATED MODEL" + after)
        (root / "src/opus/testdata/resampler-response.bin").write_bytes(fixture_data)
        subprocess.run(["rustfmt", "--edition", "2024", str(path)], check=True)
    elif max(differences) > 1e-10:
        raise RuntimeError("generated coefficients differ from owned model; inspect report before --write")


if __name__ == "__main__":
    main()
