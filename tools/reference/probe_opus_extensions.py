#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Exercise optional libopus features through public APIs, outside production.

Uses a separately built reference and an independently implemented Rust padding
parser. Generated signals establish execution and wire observations, not speech
quality or performance parity. No model data is imported into the owned crate.
"""

import argparse
import ctypes as ct
import hashlib
import json
import math
from pathlib import Path
import struct
import subprocess

from check_opus_rtp import library


def checked(value):
    if value < 0:
        raise RuntimeError(f"public libopus API error {value}")
    return value


def create(lib, kind, rate, channels, application=2048):
    error = ct.c_int()
    function = getattr(lib, f"opus_{kind}_create")
    arguments = (rate, channels, application, ct.byref(error)) if kind == "encoder" else (rate, channels, ct.byref(error))
    pointer = function(*arguments)
    checked(error.value)
    if not pointer:
        raise RuntimeError(f"null {kind}")
    return pointer


def ctl(lib, kind, state, request, value):
    checked(getattr(lib, f"opus_{kind}_ctl")(state, request, ct.c_int(value)))


def encode(lib, state, rate, channels, index):
    size = rate // 50
    values = []
    for sample in range(size):
        time = (index * size + sample) / rate
        for channel in range(channels):
            fundamental = 137 + 29 * channel
            voice = sum(math.sin(2 * math.pi * fundamental * harmonic * time) / harmonic**2 for harmonic in range(1, 7))
            # QEXT's true 96-kHz path also receives ultrasonic content.
            high = 0.025 * math.sin(2 * math.pi * 29_000 * time) if rate == 96_000 else 0
            values.append(0.16 * (0.6 + 0.4 * math.sin(7 * time)**2) * voice + high)
    pcm = (ct.c_float * len(values))(*values)
    output = (ct.c_ubyte * 16_384)()
    length = checked(lib.opus_encode_float(state, pcm, size, output, len(output)))
    return bytes(output[:length])


def decode(lib, state, data, samples, channels):
    output = (ct.c_float * (samples * channels))()
    packet = (ct.c_ubyte * len(data)).from_buffer_copy(data) if data is not None else None
    count = checked(lib.opus_decode_float(state, packet, len(data) if data is not None else 0, output, samples, 0))
    if count != samples or not all(math.isfinite(value) for value in output):
        raise RuntimeError("invalid decoded geometry/PCM")
    return bytes(output)


def compare_pcm(left, right):
    a = struct.unpack(f"={len(left) // 4}f", left)
    b = struct.unpack(f"={len(right) // 4}f", right)
    return {"identical": left == right, "max_absolute_difference": max(abs(x - y) for x, y in zip(a, b)),
            "left_sha256": hashlib.sha256(left).hexdigest(), "right_sha256": hashlib.sha256(right).hexdigest()}


def inspect_packets(probe, destination, label, packets, allow_invalid=False):
    path = destination / f"{label}.packets"
    path.write_bytes(b"".join(struct.pack("<I", len(packet)) + packet for packet in packets))
    lines = subprocess.check_output([str(probe), str(path)], text=True).splitlines()
    parsed = [json.loads(line) for line in lines]
    if len(parsed) != len(packets) or (not allow_invalid and any(item["error"] for item in parsed)):
        raise RuntimeError(f"owned parser rejected generated {label} extensions")
    ids = sorted({extension["id"] for item in parsed for extension in item["extensions"]})
    observed = [{**item, "packet_sha256": hashlib.sha256(packets[item["packet"]]).hexdigest()}
                for item in parsed if item["extensions"]]
    result = {"packets": len(packets), "record_sha256": hashlib.sha256(path.read_bytes()).hexdigest(),
              "modes": sorted({item["mode"] for item in parsed}), "bandwidths": sorted({item["bandwidth"] for item in parsed}),
              "extension_ids": ids, "packets_with_extensions": len(observed), "first": observed[:2], "last": observed[-1:],
              "framing_errors": [{"packet": item["packet"], "error": item["error"]} for item in parsed if item["error"]]}
    if label == "dred":
        result["experimental_prefixes_hex"] = sorted({extension["prefix_hex"][:4] for item in observed
                                                      for extension in item["extensions"] if extension["id"] == 126})
    return result


def dred(lib, probe, destination):
    state = create(lib, "encoder", 48_000, 1)
    settings = {4002: 64_000, 4010: 10, 4014: 30, 4008: 1103, 4024: 3001, 4050: 50}
    try:
        for request, value in settings.items():
            ctl(lib, "encoder", state, request, value)
        packets = [encode(lib, state, 48_000, 1, index) for index in range(64)]
    finally:
        lib.opus_encoder_destroy(state)
    result = inspect_packets(probe, destination, "dred", packets)
    assert result["packets_with_extensions"] > 0
    result["configuration"] = {"sample_rate": 48_000, "channels": 1, "duration_ms": 20, "bitrate": 64_000,
                               "complexity": 10, "loss_percent": 30, "bandwidth": "wideband", "signal": "voice",
                               "dred_duration_10ms_frames": 50}
    error = ct.c_int()
    model = lib.opus_dred_decoder_create(ct.byref(error))
    checked(error.value)
    parsed = lib.opus_dred_alloc(ct.byref(error))
    checked(error.value)
    if not model or not parsed:
        raise RuntimeError("DRED state allocation failed")
    decoder = create(lib, "decoder", 48_000, 1)
    plc = create(lib, "decoder", 48_000, 1)
    try:
        # Prime matching history, omit packet 39, recover it from packet 40.
        for packet in packets[:39]:
            assert decode(lib, decoder, packet, 960, 1) == decode(lib, plc, packet, 960, 1)
        packet = (ct.c_ubyte * len(packets[40])).from_buffer_copy(packets[40])
        end = ct.c_int()
        offset = checked(lib.opus_dred_parse(model, parsed, packet, len(packet), 24_000, 48_000, ct.byref(end), 1))
        if offset <= 0:
            raise RuntimeError("generated packet contained no usable DRED")
        checked(lib.opus_dred_process(model, parsed, parsed))
        recovered = (ct.c_float * 960)()
        count = checked(lib.opus_decoder_dred_decode_float(decoder, parsed, 960, recovered, 960))
        assert count == 960 and all(math.isfinite(value) for value in recovered)
        comparison = compare_pcm(bytes(recovered), decode(lib, plc, None, 960, 1))
        assert not comparison["identical"]
        decode(lib, decoder, packets[40], 960, 1)
        result["recovery"] = {"omitted_packet": 39, "carrier_packet": 40, "parse_offset": offset,
                              "dred_end": end.value, "samples": count, "versus_plc": comparison,
                              "following_normal_decode_samples": 960}
    finally:
        lib.opus_decoder_destroy(decoder)
        lib.opus_decoder_destroy(plc)
        lib.opus_dred_free(parsed)
        lib.opus_dred_decoder_destroy(model)
    return result


def qext(lib, probe, destination, rate):
    encoder = create(lib, "encoder", rate, 2, 2051)
    extended = create(lib, "decoder", rate, 2)
    base = create(lib, "decoder", rate, 2)
    try:
        ctl(lib, "encoder", encoder, 4002, 1_024_000)
        ctl(lib, "encoder", encoder, 4006, 0)
        ctl(lib, "encoder", encoder, 4056, 1)
        ctl(lib, "decoder", base, 4058, 1)
        packets = [encode(lib, encoder, rate, 2, index) for index in range(8)]
        result = inspect_packets(probe, destination, f"qext-{rate}", packets)
        assert result["packets_with_extensions"] > 0
        a = b"".join(decode(lib, extended, packet, rate // 50, 2) for packet in packets)
        b = b"".join(decode(lib, base, packet, rate // 50, 2) for packet in packets)
        result["extended_vs_ignore_extensions"] = compare_pcm(a, b)
        assert a != b
        result["sample_rate"] = rate
        result["configuration"] = {"channels": 2, "duration_ms": 20, "bitrate": 1_024_000,
                                   "application": "restricted_lowdelay", "vbr": False, "qext": True}
        result["version_note"] = "raw prefix recorded; QEXT entropy payload has no assumed DRED-style model version"
        return result
    finally:
        lib.opus_encoder_destroy(encoder)
        lib.opus_decoder_destroy(extended)
        lib.opus_decoder_destroy(base)


def base_and_bwe(lib, probe, destination):
    encoder = create(lib, "encoder", 48_000, 1)
    try:
        for request, value in {4002: 16_000, 4008: 1103, 4024: 3001}.items():
            ctl(lib, "encoder", encoder, request, value)
        packets = [encode(lib, encoder, 48_000, 1, index) for index in range(32)]
    finally:
        lib.opus_encoder_destroy(encoder)
    original = inspect_packets(probe, destination, "base-wb", packets)
    assert original["modes"] == ["Silk"] and original["bandwidths"] == ["Wideband"]
    variants, variant_packets = [], []
    # Reframe the first mono 20-ms packet through the public padding API.
    for padding in (bytes([0x41, 255]), bytes([3, 255]), bytes([0xff, 1, 42]), bytes([7, 42])):
        buffer = (ct.c_ubyte * (len(packets[0]) + 16))()
        buffer[:len(packets[0])] = packets[0]
        checked(lib.opus_packet_pad(buffer, len(packets[0]), len(buffer)))
        framed = bytes(buffer)
        # All newly written padding is zero; keep enough actual extension bytes
        # at its end to retain intentionally truncated length encodings.
        framed = framed[:-len(padding)] + padding
        # The leading original-padding terminator would hide these extensions,
        # so turn every earlier padding byte into ID0,L1 skip padding.
        parsed = (ct.c_ubyte * len(framed)).from_buffer_copy(framed)
        sizes = (ct.c_int16 * 48)()
        frames = (ct.POINTER(ct.c_ubyte) * 48)()
        toc, offset = ct.c_ubyte(), ct.c_int()
        count = checked(lib.opus_packet_parse(parsed, len(parsed), ct.byref(toc), frames, sizes, ct.byref(offset)))
        start = offset.value + sum(sizes[:count])
        framed = framed[:start] + bytes([1]) * (len(framed) - start - len(padding)) + padding
        variant_packets.append(framed)
        left, right = create(lib, "decoder", 48_000, 1), create(lib, "decoder", 48_000, 1)
        try:
            comparison = compare_pcm(decode(lib, left, packets[0], 960, 1), decode(lib, right, framed, 960, 1))
            assert comparison["identical"]
            variants.append({"padding_hex": framed[start:].hex(), **comparison})
        finally:
            lib.opus_decoder_destroy(left)
            lib.opus_decoder_destroy(right)
    framing = inspect_packets(probe, destination, "base-padding", variant_packets, allow_invalid=True)
    assert len(framing["framing_errors"]) == 2 and framing["extension_ids"] == [3, 127]
    states = [create(lib, "decoder", 48_000, 1) for _ in range(2)]
    try:
        for index, state in enumerate(states):
            ctl(lib, "decoder", state, 4010, 10)
            ctl(lib, "decoder", state, 4054, index)
        decoded = [b"".join(decode(lib, state, packet, 960, 1) for packet in packets) for state in states]
        comparison = compare_pcm(*decoded)
        assert not comparison["identical"]
        return {"base_packets": original, "base_padding_variants": variants, "owned_padding_framing": framing, "osce_bwe_toggle": comparison,
                "configuration": {"sample_rate": 48_000, "channels": 1, "duration_ms": 20, "bitrate": 16_000,
                                  "bandwidth": "wideband", "signal": "voice", "decoder_complexity": 10},
                "osce_note": "same WB packets, decoder complexity10, BWE0 vs1; execution only, no perceptual acceptance"}
    finally:
        for state in states:
            lib.opus_decoder_destroy(state)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=True)
    lib = library(args.library.resolve())
    signatures = {
        "opus_dred_decoder_create": ([ct.POINTER(ct.c_int)], ct.c_void_p),
        "opus_dred_alloc": ([ct.POINTER(ct.c_int)], ct.c_void_p),
        "opus_dred_decoder_destroy": ([ct.c_void_p], None),
        "opus_dred_free": ([ct.c_void_p], None),
        "opus_dred_parse": ([ct.c_void_p, ct.c_void_p, ct.POINTER(ct.c_ubyte), ct.c_int32, ct.c_int32, ct.c_int32, ct.POINTER(ct.c_int), ct.c_int], ct.c_int),
        "opus_dred_process": ([ct.c_void_p, ct.c_void_p, ct.c_void_p], ct.c_int),
        "opus_decoder_dred_decode_float": ([ct.c_void_p, ct.c_void_p, ct.c_int32, ct.POINTER(ct.c_float), ct.c_int32], ct.c_int),
        "opus_packet_pad": ([ct.POINTER(ct.c_ubyte), ct.c_int32, ct.c_int32], ct.c_int),
        "opus_packet_parse": ([ct.POINTER(ct.c_ubyte), ct.c_int32, ct.POINTER(ct.c_ubyte), ct.POINTER(ct.POINTER(ct.c_ubyte)), ct.POINTER(ct.c_int16), ct.POINTER(ct.c_int)], ct.c_int),
    }
    for name, (arguments, result) in signatures.items():
        function = getattr(lib, name)
        function.argtypes, function.restype = arguments, result
    probe = args.output.resolve() / "extension_probe"
    subprocess.run(["rustc", "--edition=2024", "-O", str(Path(__file__).with_name("extension_probe.rs")), "-o", str(probe)], check=True)
    result = {"reference": lib.opus_get_version_string().decode(), "library_sha256": hashlib.sha256(args.library.read_bytes()).hexdigest(),
              "scope": "public API execution and actual generated wire payloads only; no owned extension codec, quality or performance parity",
              "dred": dred(lib, probe, args.output), "qext": [qext(lib, probe, args.output, rate) for rate in (48_000, 96_000)],
              "base_and_bwe": base_and_bwe(lib, probe, args.output)}
    (args.output / "observed.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps({"reference": result["reference"], "dred": result["dred"]["extension_ids"], "qext": [item["extension_ids"] for item in result["qext"]]}))


if __name__ == "__main__":
    main()
