#!/usr/bin/env python3
"""Verify real microphone dictation to the clipboard without a foreground editor."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import time
import tomllib

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', type=Path, default=Path.home() / 'Applications/Nemo.app')
    parser.add_argument('--output', type=Path, required=True, help='new evidence directory')
    args = parser.parse_args()
    app = args.app.expanduser().resolve()
    output = args.output.expanduser().resolve()
    source = Path.home() / 'Library/Application Support/Nemo'
    binary = app / 'Contents/MacOS/nemo'
    original = (source / 'config.toml').read_bytes()
    config, count = re.subn(r'(?m)^paste\s*=\s*(?:true|false)[ \t]*$',
                            'paste = false', original.decode(), count=1)
    if count != 1 or tomllib.loads(config).get('paste') is not False:
        raise RuntimeError('Cannot locate the top-level paste preference')
    output.mkdir(parents=True, exist_ok=False)
    data = output / 'data'
    data.mkdir()
    (data / 'config.toml').write_text(config)
    normal_env = {**os.environ, 'NEMO_DATA_DIR': str(source), 'NEMO_APP_PATH': str(app)}
    test_env = {**normal_env, 'NEMO_DATA_DIR': str(data)}
    was_running = subprocess.run([str(binary), 'status'], env=normal_env,
                                 capture_output=True).returncode == 0
    if was_running:
        subprocess.run([str(binary), 'quit', '--if-idle'], env=normal_env,
                       check=True, capture_output=True)
        deadline = time.monotonic() + 5
        while (source / 'run/control.sock').exists():
            if time.monotonic() >= deadline:
                raise RuntimeError('Previous daemon did not stop; no test started')
            time.sleep(.05)
    child = None
    code = 1
    report = {'scope': 'installed executable; real microphone to clipboard; isolated paste=false',
              'paste_verified': False, 'external_networking': 'denied by sandbox-exec',
              'normal_app_was_running': was_running, 'normal_app_restored': False}
    try:
        command = ['/usr/bin/sandbox-exec', '-f', str(ROOT / 'scripts/local-only.sb'),
                   str(binary), 'run']
        report['command'] = command
        with (output / 'daemon.log').open('w') as log:
            child = subprocess.Popen(command, env=test_env, stdout=log, stderr=log,
                                     start_new_session=True)
        report['daemon_pid'] = child.pid
        deadline = time.monotonic() + 10
        while True:
            if child.poll() is not None:
                raise RuntimeError('Test daemon exited during startup; inspect daemon.log')
            status = subprocess.run([str(binary), 'status'], env=test_env,
                                    capture_output=True, text=True, timeout=5)
            if status.returncode == 0 and 'Idle' in status.stdout:
                break
            if time.monotonic() >= deadline:
                raise RuntimeError('Test daemon did not become idle')
            time.sleep(.05)
        report['identity'] = json.loads(subprocess.check_output(
            [str(binary), 'identity'], env=test_env, text=True, timeout=5))
        assert report['identity']['pid'] == child.pid, 'Unexpected daemon answered'
        code = subprocess.run([sys.executable, str(ROOT / 'scripts/verify-installed.py'),
                               '--roundtrip-only', '--app', str(app), '--data-dir', str(data),
                               '--output', str(output / 'roundtrip')], cwd=ROOT).returncode
        if code == 0:
            result = json.loads((output / 'roundtrip/results.json').read_text())
            assert result['status'] == 'passed' and result['clipboard_matches_reference']
            assert result['daemon_pid'] == child.pid
            assert result['configuration']['paste'] is False
            assert result['binary_sha256'] == report['identity']['sha256']
    finally:
        stopped = child is None or child.poll() is not None
        if not stopped:
            quit_result = subprocess.run([str(binary), 'quit', '--if-idle'], env=test_env,
                                         capture_output=True, timeout=5)
            if quit_result.returncode == 0:
                child.wait(timeout=5)
                stopped = True
        report['test_daemon_stopped'] = stopped
        # A user shortcut can take over recording. Never terminate that recording.
        if was_running and stopped:
            subprocess.run(['/usr/bin/open', str(app)], check=True)
            deadline = time.monotonic() + 10
            while time.monotonic() < deadline:
                result = subprocess.run([str(binary), 'identity'], env=normal_env,
                                        capture_output=True, text=True, timeout=5)
                if result.returncode == 0:
                    identity = json.loads(result.stdout)
                    if child is None or identity['pid'] != child.pid:
                        report['normal_app_restored'] = True
                        break
                time.sleep(.1)
        report['normal_configuration_unchanged'] = (source / 'config.toml').read_bytes() == original
        report['normal_config_sha256'] = hashlib.sha256(original).hexdigest()
        report['passed'] = (code == 0 and stopped and report['normal_configuration_unchanged']
                            and (not was_running or report['normal_app_restored']))
        (output / 'summary.json').write_text(json.dumps(report, indent=2) + '\n')
    print(f"{'passed' if report['passed'] else 'incomplete'}: {output / 'summary.json'}")
    return 0 if report['passed'] else code or 2


if __name__ == '__main__':
    sys.exit(main())
