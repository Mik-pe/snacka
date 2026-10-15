#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Record installed public libopus APIs and accepted optional feature controls.

This is a development-only reference inventory, not codec or quality acceptance.
Exported symbols and accepted controls do not establish an exercised extension.
No foreign implementation source is read and no reference dependency is added.
"""

import argparse
import ctypes as ct
import hashlib
import json
from pathlib import Path
import platform

from check_opus_rtp import library


ERRORS = {0: "ok", -1: "bad_argument", -2: "buffer_too_small", -3: "internal",
          -4: "invalid_packet", -5: "unimplemented", -6: "invalid_state",
          -7: "allocation_failure"}
RATES = (8_000, 12_000, 16_000, 24_000, 48_000, 96_000)
SYMBOLS = (
    "opus_encode", "opus_encode_float", "opus_encode24",
    "opus_decode", "opus_decode_float", "opus_decode24",
    "opus_multistream_encode", "opus_multistream_encode_float", "opus_multistream_encode24",
    "opus_multistream_decode", "opus_multistream_decode_float", "opus_multistream_decode24",
    "opus_projection_encode", "opus_projection_encode_float", "opus_projection_encode24",
    "opus_projection_decode", "opus_projection_decode_float", "opus_projection_decode24",
    "opus_custom_encode", "opus_custom_encode_float", "opus_custom_encode24",
    "opus_custom_decode", "opus_custom_decode_float", "opus_custom_decode24",
    "opus_dred_decoder_create", "opus_dred_parse", "opus_dred_process",
    "opus_decoder_dred_decode", "opus_decoder_dred_decode_float", "opus_decoder_dred_decode24",
)


def status(code):
    return {"code": code, "status": ERRORS.get(code, "unknown")}


def creation(lib, kind, rate):
    error = ct.c_int()
    if kind == "encoder":
        state = lib.opus_encoder_create(rate, 1, 2049, ct.byref(error))
    else:
        state = lib.opus_decoder_create(rate, 1, ct.byref(error))
    try:
        return {**status(error.value), "created": bool(state)}
    finally:
        if state:
            getattr(lib, f"opus_{kind}_destroy")(state)


def control(lib, kind, setter, getter, value):
    error = ct.c_int()
    if kind == "encoder":
        state = lib.opus_encoder_create(48_000, 1, 2049, ct.byref(error))
    else:
        state = lib.opus_decoder_create(48_000, 1, ct.byref(error))
    if not state or error.value:
        raise RuntimeError(f"reference {kind} creation failed: {error.value}")
    try:
        ctl = getattr(lib, f"opus_{kind}_ctl")
        code = ctl(state, setter, ct.c_int(value))
        result = {"requested_value": value, "set": status(code)}
        if code == 0 and getter is not None:
            observed = ct.c_int()
            code = ctl(state, getter, ct.byref(observed))
            result["get"] = {**status(code), "value": observed.value if code == 0 else None}
        return result
    finally:
        getattr(lib, f"opus_{kind}_destroy")(state)


def inspect(path):
    lib = library(path)
    result = {
        "reference": lib.opus_get_version_string().decode(),
        "reference_library": str(path),
        "reference_sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
        "platform": platform.platform(),
        "scope": "public symbol presence, constructor and control acceptance only; no extension processing or quality claim",
        "api_reference": "https://opus-codec.org/docs/opus_api-1.6.pdf",
        "features_reference": "https://opus-codec.org/demo/opus-1.6/",
        "symbols": {name: hasattr(lib, name) for name in SYMBOLS},
        "sample_rates": {kind: {str(rate): creation(lib, kind, rate) for rate in RATES}
                         for kind in ("encoder", "decoder")},
        "controls": {},
    }
    for name, kind, setter, getter, value in (
        ("encoder_complexity", "encoder", 4010, 4011, 10),
        ("constrained_vbr", "encoder", 4020, 4021, 1),
        ("expected_loss_percent", "encoder", 4014, 4015, 30),
        ("dred_duration", "encoder", 4050, 4051, 10),
        ("qext", "encoder", 4056, 4057, 1),
        ("decoder_complexity", "decoder", 4010, 4011, 10),
        ("osce_bandwidth_extension", "decoder", 4054, 4055, 1),
        ("ignore_extensions", "decoder", 4058, 4059, 1),
    ):
        result["controls"][name] = control(lib, kind, setter, getter, value)
    return result


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    serialized = json.dumps(inspect(args.library.resolve()), indent=2) + "\n"
    if args.output:
        args.output.write_text(serialized)
    else:
        print(serialized, end="")


if __name__ == "__main__":
    main()
