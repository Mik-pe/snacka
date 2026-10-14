# SPDX-License-Identifier: MIT
"""Loopback RTP/Opus journey against a separately installed public libopus peer.

Only this external tester loads libopus. The Rust example has no FFI or codec
dependency. It exchanges generated voiced PCM through real UDP sockets, checks
standard RTP headers, duplicates/reordering, one lost packet and actual SILK FEC.
This does not exercise a microphone, speaker, mobile device, ICE, SRTP or WebRTC.
"""

import argparse
import array
import ctypes as ct
import json
import math
from pathlib import Path
import re
import socket
import struct
import subprocess
import sys
import time

FRAMES, SAMPLES, PT = 50, 960, 111


def library(path):
    lib = ct.CDLL(str(path))
    signatures = {
        "opus_encoder_create": ([ct.c_int, ct.c_int, ct.c_int, ct.POINTER(ct.c_int)], ct.c_void_p),
        "opus_decoder_create": ([ct.c_int, ct.c_int, ct.POINTER(ct.c_int)], ct.c_void_p),
        "opus_encoder_destroy": ([ct.c_void_p], None),
        "opus_decoder_destroy": ([ct.c_void_p], None),
        "opus_encoder_ctl": ([ct.c_void_p, ct.c_int], ct.c_int),
        "opus_decoder_ctl": ([ct.c_void_p, ct.c_int], ct.c_int),
        "opus_encode_float": ([ct.c_void_p, ct.POINTER(ct.c_float), ct.c_int, ct.POINTER(ct.c_ubyte), ct.c_int], ct.c_int),
        "opus_decode_float": ([ct.c_void_p, ct.POINTER(ct.c_ubyte), ct.c_int, ct.POINTER(ct.c_float), ct.c_int, ct.c_int], ct.c_int),
        "opus_get_version_string": ([], ct.c_char_p),
    }
    for name, (arguments, result) in signatures.items():
        function = getattr(lib, name)
        function.argtypes, function.restype = arguments, result
    return lib


def decoder(lib, channels):
    error = ct.c_int()
    pointer = lib.opus_decoder_create(48_000, channels, ct.byref(error))
    assert pointer and error.value == 0
    return pointer


def interval(lib, state, payload, channels, fec=0):
    output = (ct.c_float * (SAMPLES * channels))()
    encoded = (ct.c_ubyte * len(payload)).from_buffer_copy(payload)
    count = lib.opus_decode_float(state, encoded, len(payload), output, SAMPLES, fec)
    assert count == SAMPLES, count
    samples = list(output)
    assert all(math.isfinite(value) for value in samples)
    return samples


def final_range(lib, state):
    value = ct.c_uint32()
    assert lib.opus_decoder_ctl(state, 4031, ct.byref(value)) == 0
    return value.value


def start(example, command, peer, channels, output=None, frames=FRAMES, extra=()):
    arguments = [str(example), command, "--bind", "127.0.0.1:0", "--peer",
                 f"{peer[0]}:{peer[1]}", "--frames", str(frames), "--channels", str(channels), *extra]
    if output:
        arguments += ["--output", str(output)]
    process = subprocess.Popen(arguments, text=True, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    header = process.stdout.readline().strip()
    match = re.fullmatch(r"RTP local=127\.0\.0\.1:(\d+), peer=127\.0\.0\.1:(\d+)", header)
    if not match:
        stdout, stderr = process.communicate(timeout=5)
        raise RuntimeError(header + stdout + stderr)
    return process, ("127.0.0.1", int(match.group(1))), header


def finish(process, header):
    stdout, stderr = process.communicate(timeout=5)
    assert process.returncode == 0, (stdout, stderr)
    return header + "\n" + stdout.strip()


def terminate(process):
    if process.poll() is None:
        process.terminate()
        try:
            process.wait(timeout=2)
        except subprocess.TimeoutExpired:
            process.kill()
            process.wait()


def sender_to_reference(lib, example, destination, channels, fec=False):
    decoded = decoder(lib, channels)
    process, audio = None, []
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as peer:
            peer.bind(("127.0.0.1", 0))
            peer.settimeout(3)
            extra = ("--fec", "true", "--bitrate", str(32000 * channels)) if fec else ()
            process, address, header = start(example, "send", peer.getsockname(), channels, extra=extra)
            for index in range(FRAMES):
                datagram, source = peer.recvfrom(1500)
                assert source == address
                first, second, sequence, timestamp, ssrc = struct.unpack("!BBHII", datagram[:12])
                assert first == 0x80 and second & 127 == PT and ssrc == 0x53504B41
                assert bool(second & 128) == (index == 0)
                assert sequence == (65_530 + index) % 65_536
                assert timestamp == (0xFFFFFFFF - 3 * SAMPLES + index * SAMPLES) % 2**32
                assert 8 <= datagram[12] >> 3 <= 11, "owned voice WB packet must use SILK"
                if fec and index == 9:
                    audio += interval(lib, decoded, datagram[12:], channels, fec=1)
                if not fec or index != 8:
                    audio += interval(lib, decoded, datagram[12:], channels)
            status = finish(process, header)
            reported = int(re.search(r"final_range=(\d+)", status).group(1))
            assert reported == final_range(lib, decoded)
            fec_packets = int(re.search(r"fec_packets=(\d+)", status).group(1))
            if fec:
                assert fec_packets > 0, status
            name = "rust-fec-to-libopus.f32" if fec else "rust-to-libopus.f32"
            (destination / name).write_bytes(struct.pack(f"<{len(audio)}f", *audio))
            return {"direction": "Rust voice Opus/FEC to libopus" if fec else "Rust voice Opus to libopus", "packets": FRAMES,
                    "samples": len(audio), "channels": channels, "final_range": reported,
                    "owned_fec_packets": fec_packets, "simulated_reference_losses": int(fec),
                    **generated_quality(audio, channels), "status": status}
    finally:
        lib.opus_decoder_destroy(decoded)
        if process:
            terminate(process)


def voiced(index):
    result = []
    for sample in range(SAMPLES):
        seconds = (index * SAMPLES + sample) / 48_000
        envelope = math.sin(math.pi * (seconds % 0.7) / 0.7)**2
        value = sum(math.sin(2 * math.pi * 137 * harmonic * seconds) / harmonic**2 for harmonic in range(1, 7))
        result.append(0.2 * envelope * value)
    return result


def generated_quality(audio, channels, frames=FRAMES, quiet_first=0):
    """Synthetic voiced signal comparison, with explicitly searched delay.

    This is an engineering signal metric, not perceptual speech acceptance.
    Skip startup/end; a coarse delay search is refined at single-sample steps.
    """
    original = [value for index in range(frames)
                for value in ([0.0] * SAMPLES if index < quiet_first else voiced(index))]

    def snr(delay, stride):
        signal, error = 0.0, 0.0
        for index in range(max(3000, quiet_first * SAMPLES + 3000), len(original) - 1200 - delay, stride):
            for channel in range(channels):
                signal += original[index]**2
                error += (original[index] - audio[(index + delay) * channels + channel])**2
        return 10 * math.log10(signal / max(error, 1e-30))

    coarse = max(range(0, 1025, 4), key=lambda delay: snr(delay, 8))
    delay = max(range(max(0, coarse - 5), min(1024, coarse + 5) + 1),
                key=lambda value: snr(value, 1))
    return {"generated_signal_aligned_delay_samples": delay,
            "generated_signal_aligned_snr_db": snr(delay, 1),
            "quality_scope": "generated harmonic envelope, startup/end excluded, best delay0..1024; no perceptual acceptance"}


def reference_to_receiver(lib, example, destination, channels):
    error = ct.c_int()
    encoded = lib.opus_encoder_create(48_000, channels, 2048, ct.byref(error))
    assert encoded and error.value == 0
    decoded, process, reference = decoder(lib, channels), None, []
    native_file = destination / "libopus-to-rust.f32"
    try:
        # Public controls: bitrate 24k, WB, voice, FEC enabled, 30% expected loss.
        for request, value in [(4002, 24_000 * channels), (4004, 1103), (4008, 1103),
                               (4022, channels), (4024, 3001), (4012, 1), (4014, 30)]:
            assert lib.opus_encoder_ctl(encoded, request, value) == 0
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as peer:
            peer.bind(("127.0.0.1", 0))
            process, address, header = start(example, "recv", peer.getsockname(), channels, native_file)
            deadline, held = time.monotonic(), {}
            for index in range(FRAMES):
                pcm = (ct.c_float * (SAMPLES * channels))(*(sample for sample in voiced(index) for _ in range(channels)))
                packet = (ct.c_ubyte * 1276)()
                length = lib.opus_encode_float(encoded, pcm, SAMPLES, packet, len(packet))
                assert length > 0
                payload = bytes(packet[:length])
                datagram = struct.pack("!BBHII", 0x80, PT | (128 if index == 0 else 0),
                                       (65_530 + index) % 65_536,
                                       (0xFFFFFFFF - 3 * SAMPLES + index * SAMPLES) % 2**32,
                                       0x53504B41) + payload
                # One actual UDP omission, one duplicate and early reordering.
                if index < 2:
                    held[index] = datagram
                elif index != 8:
                    peer.sendto(datagram, address)
                    if index == 2:
                        peer.sendto(held[1], address)
                        peer.sendto(held[0], address)
                    if index == 3:
                        peer.sendto(datagram, address)
                if index == 9:
                    reference += interval(lib, decoded, payload, channels, fec=1)
                if index != 8:
                    reference += interval(lib, decoded, payload, channels)
                deadline += 0.020
                time.sleep(max(0, deadline - time.monotonic()))
            status = finish(process, header)
            assert "played=50" in status and "decoded=49" in status and "fec=1" in status, status
            assert int(re.search(r"final_range=(\d+)", status).group(1)) == final_range(lib, decoded)
        native = array.array("f")
        native.frombytes(native_file.read_bytes())
        if sys.byteorder != "little":
            native.byteswap()
        assert len(native) == len(reference) == FRAMES * SAMPLES * channels
        assert all(math.isfinite(value) for value in native)
        error_rms = math.sqrt(sum((left - right)**2 for left, right in zip(native, reference)) / len(native))
        (destination / "libopus-to-libopus.f32").write_bytes(struct.pack(f"<{len(reference)}f", *reference))
        return {"direction": "libopus SILK/FEC to Rust", "packets_received": FRAMES - 1,
                "played_frames": FRAMES, "channels": channels, "fec_frames": 1, "reference_pcm_error_rms": error_rms,
                "reference_generated_signal_quality": generated_quality(reference, channels),
                "status": status}
    finally:
        lib.opus_encoder_destroy(encoded)
        lib.opus_decoder_destroy(decoded)
        if process:
            terminate(process)


def dtx_to_reference(lib, example, destination, channels):
    decoded, process = decoder(lib, channels), None
    suppressed, active, audio = 0, 0, []
    try:
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as peer:
            peer.bind(("127.0.0.1", 0))
            peer.settimeout(3)
            process, address, header = start(example, "send", peer.getsockname(), channels,
                                             frames=100, extra=("--dtx", "true", "--quiet-first", "50"))
            for index in range(100):
                datagram, source = peer.recvfrom(1500)
                assert source == address
                assert len(datagram) >= 13
                first, second, sequence, timestamp, ssrc = struct.unpack("!BBHII", datagram[:12])
                assert first == 0x80 and second & 127 == PT and ssrc == 0x53504B41
                assert sequence == (65530 + index) % 65536
                assert timestamp == (0xffffffff - 3 * SAMPLES + index * SAMPLES) % 2**32
                payload = datagram[12:]
                assert 8 <= payload[0] >> 3 <= 11
                if len(payload) == 1:
                    assert index < 50, "DTX persisted into voiced input"
                    suppressed += 1
                elif index >= 50:
                    active += 1
                audio += interval(lib, decoded, payload, channels)
            status = finish(process, header)
            assert int(re.search(r"dtx_packets=(\d+)", status).group(1)) == suppressed
            assert 20 <= suppressed < 50 and active == 50, (suppressed, active, status)
            reported = int(re.search(r"final_range=(\d+)", status).group(1))
            assert reported == final_range(lib, decoded)
            active_pcm = audio[60 * SAMPLES * channels:]
            rms = math.sqrt(sum(value**2 for value in active_pcm) / len(active_pcm))
            assert rms > 0.01, "voiced signal did not resume after DTX"
            (destination / "rust-dtx-to-libopus.f32").write_bytes(struct.pack(f"<{len(audio)}f", *audio))
            return {"direction": "Rust DTX quiet-to-voiced to libopus", "packets": 100,
                    "channels": channels, "quiet_input_packets": 50, "dtx_packets": suppressed,
                    "resumed_voiced_packets": active, "resumed_pcm_rms": rms,
                    **generated_quality(audio, channels, frames=100, quiet_first=50),
                    "final_range": reported, "status": status}
    finally:
        lib.opus_decoder_destroy(decoded)
        if process:
            terminate(process)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("library", type=Path)
    parser.add_argument("--example", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--channels", type=int, choices=(1, 2), default=1)
    args = parser.parse_args()
    destination = args.output_dir.resolve()
    root = Path(__file__).resolve().parents[2]
    if destination == root or root in destination.parents:
        parser.error("output directory must be outside the repository")
    destination.mkdir(parents=True, exist_ok=True)
    lib = library(args.library.resolve())
    result = {"reference": lib.opus_get_version_string().decode(), "physical_mobile": "NOT RUN",
              "microphone_speaker": "NOT RUN", "transport": "loopback plain RTP/UDP",
              "journeys": [sender_to_reference(lib, args.example.resolve(), destination, args.channels),
                           reference_to_receiver(lib, args.example.resolve(), destination, args.channels),
                           sender_to_reference(lib, args.example.resolve(), destination, args.channels, fec=True),
                           dtx_to_reference(lib, args.example.resolve(), destination, args.channels)]}
    result["generated_signal_regression_floor_db"] = 20
    result["generated_signal_regression_pass"] = all(
        journey.get("generated_signal_aligned_snr_db", 20) >= 20 for journey in result["journeys"])
    result["payload_interoperability_pass"] = True
    (destination / "summary.json").write_text(json.dumps(result, indent=2) + "\n")
    print(json.dumps(result, indent=2))
    return 0 if result["generated_signal_regression_pass"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
