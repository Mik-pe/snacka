#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Stream a named Colab session file using the official CLI's existing session.

Run with the Python environment containing google-colab-cli and requests.
The CLI's default contents download wraps a whole binary in base64 JSON;
Jupyter's authenticated /files endpoint streams the same session file directly.
No credentials are printed, written to artifacts, or sent to another service.
"""
import argparse
import hashlib
import os
from pathlib import Path
from urllib.parse import quote
import requests
from colab_cli.common import state


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("session")
    parser.add_argument("remote")
    parser.add_argument("local", type=Path)
    parser.add_argument("--sha256")
    args = parser.parse_args()
    if args.local.exists():
        if args.sha256 and hashlib.sha256(args.local.read_bytes()).hexdigest() == args.sha256:
            print("Already verified:", args.local)
            return
        parser.error("destination exists and is not the verified expected file")
    session = state.get_session(args.session)
    url = session.url.rstrip("/") + "/files/" + quote(args.remote.lstrip("/"), safe="/")
    partial = args.local.with_name(args.local.name + ".partial")
    if partial.exists():
        parser.error("partial destination already exists; inspect it before retrying")
    digest = hashlib.sha256()
    size = 0
    try:
        with requests.get(url, params={"authuser":"0", "colab-runtime-proxy-token":session.token}, stream=True, timeout=(30,120)) as response:
            if response.status_code != 200:
                raise RuntimeError(f"Colab file endpoint returned HTTP {response.status_code}")
            with partial.open("xb") as stream:
                for chunk in response.iter_content(8*1024*1024):
                    stream.write(chunk); digest.update(chunk); size += len(chunk)
    except requests.RequestException as error:
        # requests exceptions can contain credential-bearing query URLs.
        raise RuntimeError("Colab file transfer failed: " + type(error).__name__) from None
    actual = digest.hexdigest()
    if args.sha256 and actual != args.sha256:
        raise RuntimeError("download checksum differs from expected source")
    os.replace(partial,args.local)
    print(f"Downloaded {size} bytes; SHA256 {actual}; {args.local}")


if __name__ == "__main__":
    main()
