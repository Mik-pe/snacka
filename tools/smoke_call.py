#!/usr/bin/env python3
"""Exercise two real client processes without audio hardware, directly and relayed."""
import argparse
import os
from pathlib import Path
import re
import secrets
import socket
import subprocess
import tempfile
import time


def port(family, address):
    with socket.socket(family, socket.SOCK_DGRAM) as probe:
        probe.bind((address, 0))
        return probe.getsockname()[1]


def run_pair(client, relay_binary, mode):
    env = dict(os.environ, SNACKA_CALL_KEY=secrets.token_hex(32))
    processes = []
    with tempfile.TemporaryDirectory(prefix="snacka-call-") as directory:
        logs = []
        try:
            if mode.startswith("direct-"):
                ipv6 = mode == "direct-ipv6"
                family, host = (socket.AF_INET6, "::1") if ipv6 else (socket.AF_INET, "127.0.0.1")
                flag = "--ipv6" if ipv6 else "--ipv4"
                a = port(family, host)
                b = port(family, host)
                while b == a:
                    b = port(family, host)
                address = lambda number: f"[{host}]:{number}" if ipv6 else f"{host}:{number}"
                routes = [["--bind", address(a), "--peer", address(b), flag],
                          ["--bind", address(b), "--peer", address(a), flag]]
            else:
                relay_port = port(socket.AF_INET, "127.0.0.1")
                relay_log = open(Path(directory) / "relay.log", "w+", encoding="utf-8")
                logs.append(relay_log)
                processes.append(subprocess.Popen(
                    [str(relay_binary), "--bind", f"127.0.0.1:{relay_port}", "--seconds", "10"],
                    env=env, stdout=relay_log, stderr=subprocess.STDOUT))
                # Poll readiness instead of assuming the server has started.
                deadline = time.monotonic() + 5
                while True:
                    relay_log.seek(0)
                    if "Relay listening" in relay_log.read():
                        break
                    if processes[0].poll() is not None or time.monotonic() >= deadline:
                        raise RuntimeError("Relay failed to start")
                    time.sleep(0.02)
                routes = [["--relay", f"127.0.0.1:{relay_port}"]] * 2
            clients = []
            for role, route in enumerate(routes, 1):
                log = open(Path(directory) / f"client-{role}.log", "w+", encoding="utf-8")
                logs.append(log)
                process = subprocess.Popen(
                    [str(client), *route, "--role", str(role), "--test-tone", "--no-tui", "--seconds", "4"],
                    env=env, stdout=log, stderr=subprocess.STDOUT)
                clients.append((process, log, role))
                processes.append(process)
            for process, log, role in clients:
                code = process.wait(timeout=15)
                log.seek(0)
                output = log.read()
                match = re.search(r"Call ended: TX=(\d+) RX=(\d+) decoded=(\d+) PLC=(\d+)", output)
                if code != 0 or match is None or min(map(int, match.groups()[:3])) < 40:
                    raise RuntimeError(f"{mode} role {role} failed (exit {code}):\n{output}")
                decoded, concealed = map(int, match.groups()[2:])
                if concealed / (decoded + concealed) > 0.15:
                    raise RuntimeError(f"{mode} role {role} concealed too much healthy loopback audio:\n{output}")
                if not any(float(value) > 0.005 for value in re.findall(r"peer_peak=([0-9.]+)", output)):
                    raise RuntimeError(f"{mode} role {role} decoded no audible signal:\n{output}")
                print(f"PASS {mode} role {role}: TX/RX/decoded/PLC = {'/'.join(match.groups())}")
        finally:
            for process in processes:
                if process.poll() is None:
                    process.terminate()
            for process in processes:
                try:
                    process.wait(timeout=3)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()
            for log in logs:
                log.close()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary-dir", type=Path, default=Path("target/release/examples"))
    args = parser.parse_args()
    suffix = ".exe" if os.name == "nt" else ""
    client = (args.binary_dir / f"call{suffix}").resolve()
    relay = (args.binary_dir / f"call_relay{suffix}").resolve()
    for binary in [client, relay]:
        if not binary.is_file():
            parser.error(f"Build the examples first; missing {binary}")
    run_pair(client, relay, "direct-ipv4")
    run_pair(client, relay, "direct-ipv6")
    run_pair(client, relay, "relay-ipv4")


if __name__ == "__main__":
    main()
