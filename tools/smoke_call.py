#!/usr/bin/env python3
"""Exercise two real client processes without audio hardware, directly and relayed."""
import argparse
import os
from pathlib import Path
import re
import secrets
import select
import socket
import subprocess
import tempfile
import time
import threading


class ImpairedLink:
    """Two connected UDP routes; impair only A's audio or its returning feedback."""
    def __init__(self, destinations, mode):
        self.destinations = destinations
        self.mode = mode
        self.sockets = [socket.socket(socket.AF_INET, socket.SOCK_DGRAM) for _ in range(2)]
        for endpoint in self.sockets:
            endpoint.bind(("127.0.0.1", 0))
        self.stopped = threading.Event()
        self.error = None
        self.dropped = 0
        self.thread = threading.Thread(target=self.run, daemon=True)
        self.thread.start()

    def run(self):
        start = time.monotonic()
        audio_count = 0
        try:
            while not self.stopped.is_set():
                ready, _, _ = select.select(self.sockets, [], [], 0.05)
                for source in ready:
                    data, address = source.recvfrom(2048)
                    index = self.sockets.index(source)
                    if address != self.destinations[index]:
                        continue
                    kind = data[4] if len(data) >= 30 and data[:4] == b"SNC1" else 0
                    active = 2 <= time.monotonic() - start < 6
                    if index == 0 and kind == 2:
                        audio_count += 1
                    loss = self.mode == "loss-ipv4" and index == 0 and kind == 2 and audio_count % 4 == 0
                    blind = self.mode == "feedback-outage-ipv4" and index == 1 and kind == 6
                    if active and (loss or blind):
                        self.dropped += 1
                        continue
                    self.sockets[1 - index].sendto(data, self.destinations[1 - index])
        except Exception as error:
            self.error = error

    def close(self):
        self.stopped.set()
        self.thread.join(timeout=2)
        for endpoint in self.sockets:
            endpoint.close()
        if self.thread.is_alive():
            raise RuntimeError("Impairment worker did not stop")
        if self.error:
            raise RuntimeError("Impairment worker failed") from self.error


def port(family, address):
    with socket.socket(family, socket.SOCK_DGRAM) as probe:
        probe.bind((address, 0))
        return probe.getsockname()[1]


def run_pair(client, relay_binary, mode):
    env = dict(os.environ, SNACKA_CALL_KEY=secrets.token_hex(32))
    processes = []
    impairment = None
    impaired = mode in ("loss-ipv4", "feedback-outage-ipv4")
    with tempfile.TemporaryDirectory(prefix="snacka-call-") as directory:
        logs = []
        try:
            if mode.startswith("direct-") or impaired:
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
                if impaired:
                    impairment = ImpairedLink([(host, a), (host, b)], mode)
                    routes = [["--bind", address(number), "--peer", address(proxy.getsockname()[1]), flag]
                              for number, proxy in zip([a, b], impairment.sockets)]
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
                    [str(client), *route, "--role", str(role), "--test-tone", "--no-tui", "--seconds",
                     "14" if impaired else "4"],
                    env=env, stdout=log, stderr=subprocess.STDOUT)
                clients.append((process, log, role))
                processes.append(process)
            for process, log, role in clients:
                code = process.wait(timeout=20 if impaired else 15)
                log.seek(0)
                output = log.read()
                match = re.search(r"Call ended: TX=(\d+) RX=(\d+) decoded=(\d+) PLC=(\d+)", output)
                if code != 0 or match is None or min(map(int, match.groups()[:3])) < 40:
                    raise RuntimeError(f"{mode} role {role} failed (exit {code}):\n{output}")
                if impairment and impairment.dropped == 0:
                    raise RuntimeError("Impairment scenario dropped no packets")
                decoded, concealed = map(int, match.groups()[2:])
                if not impaired and concealed / (decoded + concealed) > 0.15:
                    raise RuntimeError(f"{mode} role {role} concealed too much healthy loopback audio:\n{output}")
                if not any(float(value) > 0.005 for value in re.findall(r"peer_peak=([0-9.]+)", output)):
                    raise RuntimeError(f"{mode} role {role} decoded no audible signal:\n{output}")
                rates = list(map(int, re.findall(r"bitrate=(\d+)", output)))
                rtts = re.findall(r"rtt_ms=([0-9.]+)", output)
                if not rates or not rtts or "rate=auto" not in output:
                    raise RuntimeError(f"{mode} role {role} received no adaptive feedback:\n{output}")
                if impaired and role == 1:
                    if min(rates) >= 24000 or rates[-1] <= min(rates):
                        raise RuntimeError(f"{mode} failed backoff/recovery; targets {rates}:\n{output}")
                    reason = "Loss" if mode == "loss-ipv4" else "NoFeedback"
                    if f"reason={reason}" not in output:
                        raise RuntimeError(f"{mode} did not detect {reason}:\n{output}")
                    print(f"PASS {mode} adaptation: {rates}")
                elif min(rates) < 24000:
                    raise RuntimeError(f"{mode} role {role} reduced a healthy sending direction:\n{output}")
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
            if impairment:
                impairment.close()


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
    run_pair(client, relay, "loss-ipv4")
    run_pair(client, relay, "feedback-outage-ipv4")


if __name__ == "__main__":
    main()
