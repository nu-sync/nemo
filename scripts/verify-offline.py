#!/usr/bin/env python3
"""Verify real file inference under external-network denial on macOS."""
import argparse
import errno
import hashlib
import json
import os
import pathlib
import re
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time
import tomllib
from nemo_status import control, lifecycle_status

ROOT = pathlib.Path(__file__).resolve().parents[1]
PHRASE = 'The quick brown fox jumps over the lazy dog. This is a local speech recognition test on my laptop.'


def run_owned(command, env, report, label, timeout=180):
    with subprocess.Popen(command, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          text=True, start_new_session=True) as process:
        try:
            stdout, stderr = process.communicate(timeout=timeout)
        except BaseException:
            os.killpg(process.pid, signal.SIGKILL)
            process.wait()
            raise
    report[label] = {'exit_code': process.returncode, 'stdout': stdout, 'stderr': stderr}
    if process.returncode:
        raise RuntimeError(f'{label} failed: {stderr}')
    return stdout


def words(text):
    return ''.join(c.lower() if c.isalnum() else ' ' for c in text).split()


def daemon_journey(binary, sandbox, data, env, report, output):
    def status():
        return lifecycle_status(lambda command: control(data, command))
    def await_status(expected, timeout):
        deadline = time.monotonic() + timeout
        last = 'not running'
        while time.monotonic() < deadline:
            try:
                last = status()
                if last == expected:
                    return
            except (OSError, ValueError):
                pass
            time.sleep(.05)
        raise AssertionError(f'Expected {expected}; observed {last}')
    owned = []
    with (output / 'daemon-startup.log').open('w') as log:
        daemon = subprocess.Popen(sandbox + [str(binary), 'run'], env=env, stdout=log,
                                  stderr=log, start_new_session=True)
        try:
            await_status('Ready', 15)
            identity = json.loads(control(data, 'identity'))
            assert identity['pid'] == daemon.pid
            processes = subprocess.check_output(['ps', '-axo', 'pid=,ppid='], text=True)
            owned = [int(row.split()[0]) for row in processes.splitlines() if int(row.split()[1]) == daemon.pid]
            assert not owned, f'An offline daemon with cached models must not need a download helper: {owned}'
            report['daemon'] = {'pid': daemon.pid, 'identity': identity, 'idle_children': owned}
            control(data, 'warm')
            await_status('Model loaded', 120)
            processes = subprocess.check_output(['ps', '-axo', 'pid=,ppid='], text=True)
            owned = [int(row.split()[0]) for row in processes.splitlines() if int(row.split()[1]) == daemon.pid]
            assert len(owned) == 1, f'Expected inference runtime: {owned}'
            report['daemon']['loaded_children'] = owned
            control(data, 'quit')
            assert daemon.wait(timeout=10) == 0
            deadline = time.monotonic() + 5
            while time.monotonic() < deadline:
                remaining = [pid for pid in owned if subprocess.run(['ps', '-p', str(pid), '-o', 'pid='], capture_output=True).returncode == 0]
                if not remaining:
                    break
                time.sleep(.1)
            assert not remaining, f'Owned children survived quit: {remaining}'
            report['daemon']['cleanup_verified'] = True
        finally:
            if daemon.poll() is None:
                os.killpg(daemon.pid, signal.SIGKILL)
                daemon.wait()
            for name in ('daemon.log', 'inference.log'):
                if (data / name).exists():
                    (output / name).write_bytes((data / name).read_bytes())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=pathlib.Path, default=ROOT / 'target/release/nemo')
    parser.add_argument('--output', type=pathlib.Path, required=True)
    parser.add_argument('--daemon', action='store_true', help='also launch an isolated menu daemon, warm the model, and verify quit cleanup')
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {'passed': False, 'binary': str(binary), 'binary_sha256': hashlib.sha256(binary.read_bytes()).hexdigest(),
              'platform': subprocess.check_output(['sw_vers'], text=True),
              'coverage': 'CLI WAV transcription and descendant external-network denial; no microphone or paste claim'}
    try:
        profile = (ROOT / 'scripts/local-only.sb').read_text()
        (output / 'local-only.sb').write_text(profile)
        sandbox = ['/usr/bin/sandbox-exec', '-p', profile]
        listener = socket.socket()
        listener.bind(('127.0.0.1', 0))
        listener.listen()
        listener.settimeout(10)
        port = listener.getsockname()[1]
        def serve():
            try:
                connection, _ = listener.accept()
                with connection:
                    connection.sendall(b'local-only-proof')
            finally:
                listener.close()
        server = threading.Thread(target=serve)
        server.start()
        probe = f'''
import errno,json,socket,subprocess
with socket.create_connection(('127.0.0.1',{port}),timeout=3) as stream:
    assert stream.recv(64)==b'local-only-proof'
denied=[]
for family,address in [(socket.AF_INET,('1.1.1.1',443)),(socket.AF_INET6,('2606:4700:4700::1111',443))]:
    with socket.socket(family,socket.SOCK_STREAM) as stream:
        stream.settimeout(3)
        try:stream.connect(address)
        except OSError as error:
            assert error.errno==errno.EPERM,(address,error)
            denied.append(error.errno)
        else:raise AssertionError('External connection succeeded')
child=subprocess.run(['/usr/bin/curl','--disable','--noproxy','*','--max-time','3','https://1.1.1.1'],capture_output=True)
assert child.returncode==7,child.stderr
print(json.dumps({{'loopback':'passed','external_ipv4_ipv6_errno':denied,'descendant_curl_exit':child.returncode}}))
'''
        run_owned(sandbox + [sys.executable, '-c', probe], os.environ.copy(), report, 'network_denial', 15)
        server.join(timeout=12)
        with tempfile.TemporaryDirectory(prefix='nemo-offline-') as temporary:
            data = pathlib.Path(temporary)
            config_text = subprocess.check_output([str(binary), 'config'], text=True)
            bundled_runtime = binary.parent.parent / 'Resources/runtime/nemo-speech'
            if bundled_runtime.is_file():
                config_text = re.sub(r'^runtime = .*$', 'runtime = ' + json.dumps(str(bundled_runtime)), config_text, flags=re.MULTILINE)
            config = tomllib.loads(config_text)
            for key in ('model', 'runtime'):
                if not pathlib.Path(config[key]).is_file():
                    raise RuntimeError(f'Missing configured {key}; prepare it before verification')
            report['runtime'] = config['runtime']
            report['runtime_sha256'] = hashlib.sha256(pathlib.Path(config['runtime']).read_bytes()).hexdigest()
            (data / 'config.toml').write_text(config_text)
            manifest = json.loads((ROOT / 'models/manifest.json').read_text())
            managed = data / 'models' / manifest['revision']
            managed.mkdir(parents=True)
            for asset in manifest['files']:
                (managed / asset['name']).symlink_to(pathlib.Path(config['model']).parent / asset['name'])
            env = os.environ.copy()
            env.update(NEMO_DATA_DIR=str(data), NEMO_APP_PATH=str(data / 'Nemo.app'))
            fixture = output / 'synthetic.wav'
            aiff = output / 'synthetic.aiff'
            subprocess.run(['/usr/bin/say', '-v', 'Samantha', '-o', str(aiff), PHRASE], check=True)
            subprocess.run(['/usr/bin/afconvert', '-f', 'WAVE', '-d', 'LEI16@16000', '-c', '1', str(aiff), str(fixture)], check=True)
            started = time.monotonic()
            transcript = run_owned(sandbox + [str(binary), 'transcribe', str(fixture)], env, report, 'inference')
            report['elapsed_seconds'] = time.monotonic() - started
            if words(transcript) != words(PHRASE):
                raise AssertionError(f'Transcript mismatch: {transcript}')
            events = [json.loads(line) for line in report['inference']['stderr'].splitlines() if line.startswith('{')]
            assert any(e.get('event') == 'model_load' and e.get('success') for e in events)
            assert any(e.get('event') == 'transcription' and e.get('success') for e in events)
            log = data / 'inference.log'
            report['inference_log_mode'] = oct(log.stat().st_mode & 0o777)
            assert report['inference_log_mode'] == '0o600'
            assert PHRASE.lower() not in log.read_text().lower()
            (output / 'inference.log').write_bytes(log.read_bytes())
            if args.daemon:
                daemon_journey(binary, sandbox, data, env, report, output)
            assert hashlib.sha256(binary.read_bytes()).hexdigest() == report['binary_sha256'], 'Binary changed during verification'
            assert hashlib.sha256(pathlib.Path(config['runtime']).read_bytes()).hexdigest() == report['runtime_sha256'], 'Runtime changed during verification'
            report['passed'] = True
        report['isolated_data_removed'] = not data.exists()
    except BaseException as error:
        report['error'] = str(error)
        raise
    finally:
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'passed': report['passed'], 'evidence': str(output / 'report.json')}))


if __name__ == '__main__':
    main()
