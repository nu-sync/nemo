#!/usr/bin/env python3
"""Verify a relocated bundle, optionally transcribing audio with an existing model."""
import argparse
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile


def run(*args, **kwargs):
    return subprocess.check_output([str(arg) for arg in args], text=True, **kwargs).strip()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app', type=Path, default=Path(__file__).resolve().parent.parent / 'dist/Nemo.app')
    parser.add_argument('--audio', type=Path)
    parser.add_argument('--model', type=Path)
    parser.add_argument('--expected')
    args = parser.parse_args()
    if bool(args.audio) != bool(args.model):
        parser.error('--audio and --model must be supplied together')
    with tempfile.TemporaryDirectory(prefix='nemo-relocation-') as temporary:
        root = Path(temporary).resolve()
        app = root / 'Relocated Nemo.app'
        run('/usr/bin/ditto', args.app.resolve(), app)
        run('/usr/bin/codesign', '--verify', '--deep', '--strict', app)
        binary = app / 'Contents/MacOS/nemo'
        runtime = app / 'Contents/Resources/runtime'
        checked = 0
        for file in [binary, *runtime.iterdir()]:
            if not file.is_file():
                continue
            dependencies = run('/usr/bin/otool', '-L', file).splitlines()[1:]
            for line in dependencies:
                dependency = line.strip().split(' (')[0]
                if dependency.startswith(('/System/', '/usr/lib/')):
                    continue
                if dependency.startswith('@loader_path/'):
                    resolved = file.parent / dependency.removeprefix('@loader_path/')
                elif dependency == '@rpath/' + file.name:
                    continue  # A dylib's own install name.
                else:
                    raise AssertionError(f'External dependency: {file.name}: {dependency}')
                assert resolved.is_file(), f'Missing dependency: {resolved}'
            checked += 1
        env = {**os.environ, 'NEMO_DATA_DIR': str(root / 'data')}
        env.pop('NEMO_RUNTIME', None)

        def cli(*arguments):
            return run(binary, *arguments, env=env, cwd=root)

        config = cli('config')
        expected_runtime = runtime / 'nemo-speech'
        assert 'runtime = ' + json.dumps(str(expected_runtime)) in config, config
        model = Path(cli('model', 'path'))
        assert model.is_relative_to(root / 'data/models'), model
        print(f'Relocated bundle: signature and {checked} Mach-O dependency lists verified')
        print('Fresh defaults: bundled runtime and model storage outside the checkout')
        if args.audio:
            config = re.sub(r'^model = .*$', 'model = ' + json.dumps(str(args.model.resolve())), config, flags=re.MULTILINE)
            (root / 'data').mkdir(exist_ok=True)
            (root / 'data/config.toml').write_text(config + '\n')
            transcript = cli('transcribe', args.audio.resolve())
            if args.expected is not None:
                assert transcript == args.expected, f'Unexpected transcript: {transcript!r}'
            print('Transcript:', transcript, flush=True)


if __name__ == '__main__':
    main()
