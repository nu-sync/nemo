#!/usr/bin/env python3
"""Exercise the real daemon's ownership, cancellation, unload, and signal handling."""
import json
import pathlib
import subprocess
import time

root = pathlib.Path(__file__).resolve().parent.parent
binary = root / 'target/release/nemo'
artifacts = root / 'artifacts'
artifacts.mkdir(exist_ok=True)

def cli(command):
    return subprocess.check_output([str(binary), command], text=True, stderr=subprocess.DEVNULL).strip()

def await_status(expected, timeout=120):
    started = time.monotonic()
    actual = 'not running'
    while time.monotonic() - started < timeout:
        try:
            actual = cli('status')
            if actual == expected:
                return time.monotonic() - started
        except subprocess.CalledProcessError:
            pass
        time.sleep(0.05)
    raise AssertionError(f'Expected {expected!r}; last status {actual!r}')

def children(pid):
    result = subprocess.run(['pgrep', '-P', str(pid)], text=True, capture_output=True)
    return [int(line) for line in result.stdout.splitlines()]

def alive(pid):
    return subprocess.run(['ps', '-p', str(pid), '-o', 'pid='], capture_output=True).returncode == 0

running = subprocess.run([str(binary), 'status'], capture_output=True)
if running.returncode == 0:
    raise SystemExit('Quit the existing Nemo daemon before running this check.')

daemon = subprocess.Popen([str(binary), 'run'], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
report = {}
try:
    await_status('ready', 10)
    duplicate = subprocess.run([str(binary), 'run'], capture_output=True, text=True)
    assert duplicate.returncode != 0 and 'already running' in duplicate.stderr
    report['single_instance'] = 'passed'

    clipboard = subprocess.check_output(['/usr/bin/pbpaste'])
    cli('warm')
    time.sleep(0.1)
    cli('cancel')
    await_status('Canceled', 10)
    assert not children(daemon.pid)
    assert subprocess.check_output(['/usr/bin/pbpaste']) == clipboard
    report['cancel_during_model_load'] = 'passed; child reaped; clipboard unchanged'

    cli('warm')
    report['warm_load_seconds'] = await_status('Model loaded')
    model_children = children(daemon.pid)
    assert len(model_children) == 1
    report['loaded_rss_kib'] = int(subprocess.check_output(['ps', '-p', str(model_children[0]), '-o', 'rss=']))
    report['idle_unload_wait_seconds'] = await_status('Ready · model unloaded', 90)
    assert not children(daemon.pid)
    assert all(not alive(pid) for pid in model_children)
    report['idle_daemon_rss_kib'] = int(subprocess.check_output(['ps', '-p', str(daemon.pid), '-o', 'rss=']))
    report['idle_unload'] = 'passed; child no longer exists'

    cli('warm')
    await_status('Model loaded')
    model_children = children(daemon.pid)
    daemon.terminate()
    assert daemon.wait(timeout=10) == 0
    assert all(not alive(pid) for pid in model_children)
    report['sigterm'] = 'passed; model child reaped'
finally:
    if daemon.poll() is None:
        subprocess.run([str(binary), 'quit'], capture_output=True)
        try:
            daemon.wait(timeout=10)
        except subprocess.TimeoutExpired:
            daemon.kill()
            daemon.wait()
    (artifacts / 'lifecycle-verification.json').write_text(json.dumps(report, indent=2) + '\n')
print(json.dumps(report, indent=2))
