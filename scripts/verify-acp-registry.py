#!/usr/bin/env python3
"""Provider-free clean-home check of the registry's legacy ACP auth handshake."""
import argparse
import json
import os
import select
import subprocess
import tempfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('binary', help='Path to a built crabcode executable')
    args = parser.parse_args()
    binary = os.path.abspath(args.binary)
    with tempfile.TemporaryDirectory(prefix='crabcode-registry-') as home:
        env = {key: value for key, value in os.environ.items() if key in ('PATH', 'SystemRoot', 'TMPDIR', 'TEMP', 'TMP')}
        env.update(HOME=home, XDG_CONFIG_HOME=home, XDG_STATE_HOME=home)
        proc = subprocess.Popen([binary, 'acp', '--cwd', home], env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True)
        try:
            request = {'jsonrpc': '2.0', 'id': 1, 'method': 'initialize', 'params': {'protocolVersion': 1, 'clientCapabilities': {'_meta': {'terminal-auth': True}}}}
            proc.stdin.write(json.dumps(request) + '\n')
            proc.stdin.flush()
            if not select.select([proc.stdout], [], [], 30)[0]:
                raise RuntimeError('ACP initialize timed out')
            response = json.loads(proc.stdout.readline())
            methods = response.get('result', {}).get('authMethods', [])
            assert response.get('id') == 1, response
            assert any(method.get('_meta', {}).get('terminal-auth', {}).get('args') == ['acp', '--login'] for method in methods), methods
            proc.stdin.close()
            assert proc.wait(timeout=10) == 0
            print('ACP clean-home registry auth discovery: passed (no login/provider requests)')
        finally:
            if proc.poll() is None:
                proc.kill()
                proc.wait()


if __name__ == '__main__':
    main()
