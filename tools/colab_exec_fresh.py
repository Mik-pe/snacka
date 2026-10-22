#!/usr/bin/env python3
# SPDX-License-Identifier: MIT
"""Execute via the installed official CLI runtime using a fresh client session.

Run in the Python environment containing google-colab-cli. This retains the
existing kernel and auth, closes client channels, and propagates Python errors.
No credentials are printed or stored in artifacts.
"""
import argparse
import os
from pathlib import Path
import sys
from colab_cli.common import state
from colab_cli.runtime import ColabRuntime


def main():
    p=argparse.ArgumentParser(description=__doc__);p.add_argument('session');p.add_argument('source',type=Path)
    p.add_argument('--timeout',type=float,default=30);a=p.parse_args()
    session=state.get_session(a.session)
    runtime=ColabRuntime(session.url,session.token,kernel_id=session.kernel_id,session_id=None)
    status=0
    try:
        outputs=runtime.execute_code(a.source.read_text(),timeout=a.timeout)
        for item in outputs:
            if item.get('output_type')=='error':
                print(item.get('ename','PythonError')+': '+item.get('evalue','execution failed'),file=sys.stderr)
                status=1
            elif 'text' in item:print(item['text'],end='')
    except BaseException as error:
        # Third-party exceptions may carry credential-bearing request URLs.
        print('Colab execution failed: '+type(error).__name__,file=sys.stderr);status=1
    finally:
        try:runtime.stop()
        except BaseException:pass
    sys.stdout.flush();sys.stderr.flush()
    # Some jupyter-kernel-client versions retain background websocket threads.
    # Channels are already closed; never shut down the remote kernel here.
    os._exit(status)

if __name__=='__main__':main()
