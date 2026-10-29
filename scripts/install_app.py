#!/usr/bin/env python3
"""Validate and stage an app before stopping or replacing the installed version."""
import argparse
import fcntl
import os
from pathlib import Path
import plistlib
import subprocess
import tempfile
import time
import uuid

IDENTIFIER = 'computer.nemo.dictate'


def run(*args):
    result = subprocess.run([str(arg) for arg in args], text=True, capture_output=True)
    if result.returncode:
        raise RuntimeError(result.stderr.strip() or result.stdout.strip() or f'{args[0]} failed')
    return result.stdout


def open_app(app):
    overrides = [f'{name}={os.environ[name]}' for name in ('NEMO_DATA_DIR', 'NEMO_RUNTIME') if name in os.environ]
    arguments = ['/usr/bin/open']
    if overrides:
        arguments.append('-n')
        for value in overrides:
            arguments.extend(['--env', value])
    run(*arguments, app)


def info(app):
    with (app / 'Contents/Info.plist').open('rb') as source:
        result = plistlib.load(source)
    if result.get('CFBundleIdentifier') != IDENTIFIER:
        raise RuntimeError(f'Refusing an unrelated application: {app}')
    if not (app / 'Contents/MacOS/nemo').is_file():
        raise RuntimeError(f'Application executable is missing: {app}')
    return result


def verify(app):
    info(app)
    run('/usr/bin/codesign', '--verify', '--deep', '--strict', app)


def designated_requirement(app):
    result = subprocess.run(['/usr/bin/codesign', '-d', '-r-', str(app)], capture_output=True, text=True, check=True)
    for line in (result.stdout + result.stderr).splitlines():
        line = line.removeprefix('# ')
        if line.startswith('designated => '):
            return line.removeprefix('designated => ')
    raise RuntimeError(f'Cannot read code identity: {app}')


def same_identity(existing, candidate):
    for app, requirement in [(candidate, designated_requirement(existing)), (existing, designated_requirement(candidate))]:
        result = subprocess.run(['/usr/bin/codesign', '--verify', '--strict', '-R', '=' + requirement, str(app)], capture_output=True)
        if result.returncode:
            return False
    return True


def running_pids(app):
    executable = str(app / 'Contents/MacOS/nemo')
    processes = run('/bin/ps', '-A', '-o', 'pid=,comm=')
    return [int(parts[0]) for line in processes.splitlines() if len(parts := line.strip().split(None, 1)) == 2 and parts[1] == executable]


def stop_idle_app(destination, staged):
    pids = running_pids(destination)
    if not pids:
        return False
    executable = staged / 'Contents/MacOS/nemo'
    state = run(executable, 'status').strip()
    if state in {'Recording', 'Opening microphone', 'Transcribing', 'Loading model', 'Downloading model', 'Canceling', 'Canceling microphone request'}:
        raise RuntimeError(f'Nemo is busy ({state}). Finish or cancel before upgrading.')
    if run(executable, 'quit', '--if-idle').strip() != 'accepted':
        raise RuntimeError('Nemo refused a safe quit. Finish the current operation or quit the older app manually before upgrading.')
    deadline = time.monotonic() + 10
    while running_pids(destination):
        if time.monotonic() >= deadline:
            raise RuntimeError('Nemo did not exit. The installed app has not been replaced.')
        time.sleep(0.05)
    return True


def copy_bundle(source, destination):
    run('/usr/bin/ditto', source, destination)


def install(source, destination, allow_permission_reset=False, launch=True):
    source = Path(source).absolute()
    destination = Path(destination).absolute()
    if source.resolve() == destination.resolve():
        raise RuntimeError('Source and destination must differ')
    if destination.suffix != '.app':
        raise RuntimeError('Destination must end in .app')
    if destination.is_symlink():
        raise RuntimeError('Destination must be an application bundle, not a symlink')
    verify(source)
    destination.parent.mkdir(parents=True, exist_ok=True)
    with (destination.parent / f'.{destination.stem}.install.lock').open('a') as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise RuntimeError('Another installer is using this destination') from error
        if destination.exists():
            verify(destination)
        with tempfile.TemporaryDirectory(prefix=f'.{destination.stem}.install-', dir=destination.parent) as temporary:
            staged = Path(temporary) / destination.name
            copy_bundle(source, staged)
            verify(staged)
            if destination.exists() and not allow_permission_reset and not same_identity(destination, staged):
                raise RuntimeError('The update changes code identity and may reset macOS permissions. Use a stable signing identity, or review the update and explicitly pass --allow-permission-reset. The installed app is unchanged.')
            backup = None
            if destination.exists():
                backup_root = destination.parent / f'.{destination.stem}.backups'
                backup_root.mkdir(exist_ok=True)
                backup = backup_root / f'{uuid.uuid4().hex}.app'
            was_running = False
            try:
                was_running = stop_idle_app(destination, staged)
                if backup is not None:
                    os.replace(destination, backup)
                os.replace(staged, destination)
                verify(destination)
                if was_running and launch:
                    open_app(destination)
            except BaseException:
                if backup is not None and backup.exists():
                    if destination.exists():
                        os.replace(destination, Path(temporary) / 'failed.app')
                    os.replace(backup, destination)
                elif backup is None and destination.exists():
                    os.replace(destination, Path(temporary) / 'failed.app')
                if was_running and destination.exists():
                    open_app(destination)
                raise
        return backup


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, default=Path(__file__).resolve().parent.parent / 'dist/Nemo.app')
    parser.add_argument('--destination', type=Path, default=Path.home() / 'Applications/Nemo.app')
    parser.add_argument('--allow-permission-reset', action='store_true')
    parser.add_argument('--no-launch', action='store_true', help='leave the upgraded app stopped after successful installation')
    args = parser.parse_args()
    try:
        backup = install(args.source, args.destination, args.allow_permission_reset, launch=not args.no_launch)
    except (RuntimeError, OSError, subprocess.CalledProcessError) as error:
        parser.exit(1, f'{error}\n')
    print(f'Installed {args.destination}')
    if backup:
        print(f'Previous app retained at {backup}')
    print(f'Start: open "{args.destination}"')


if __name__ == '__main__':
    main()
