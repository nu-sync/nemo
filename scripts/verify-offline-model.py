#!/usr/bin/env python3
"""Check Nemo's finite offline lifecycle model with checksum-pinned TLC."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import urllib.request

ROOT = Path(__file__).resolve().parents[1]
MODEL = ROOT / 'verification/offline'
TLC_URL = 'https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar'
TLC_SHA256 = '936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88'


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def check_sources(root):
    bindings = json.loads((MODEL / 'source-map.json').read_text())
    changed = [name for name, entry in bindings.items()
               if not (root / name).is_file() or digest(root / name) != entry['sha256']]
    if changed:
        raise RuntimeError('Modeled source changed; review the action mapping and source-map.json: ' + ', '.join(changed))


def java_command():
    candidates = [os.environ.get('JAVA'),
                  str(Path(os.environ['JAVA_HOME']) / 'bin/java') if os.environ.get('JAVA_HOME') else None,
                  '/opt/homebrew/opt/openjdk@21/bin/java', shutil.which('java')]
    for candidate in filter(None, candidates):
        try:
            result = subprocess.run([candidate, '-version'], capture_output=True, text=True, timeout=10)
            if result.returncode == 0:
                return candidate, result.stderr.strip() or result.stdout.strip()
        except (OSError, subprocess.TimeoutExpired):
            pass
    raise RuntimeError('Install Java 17+ (for example brew install openjdk@21), or set JAVA to its executable')


def run_case(java, jar, output, name, configuration):
    folder = output / name
    folder.mkdir()
    config = folder / 'Offline.cfg'
    config.write_text(configuration)
    shutil.copyfile(MODEL / 'Offline.tla', folder / 'Offline.tla')
    command = [java, '-XX:+UseParallelGC', '-Xmx512m', '-cp', str(jar), 'tlc2.TLC',
               '-workers', '1', '-seed', '1', '-fp', '0', '-metadir', 'states',
               '-config', 'Offline.cfg', 'Offline.tla']
    try:
        result = subprocess.run(command, cwd=folder, capture_output=True, text=True, timeout=60)
    except subprocess.TimeoutExpired as error:
        chunks = [error.stdout or '', error.stderr or '']
        log = ''.join(chunk.decode(errors='replace') if isinstance(chunk, bytes) else chunk for chunk in chunks)
        (folder / 'tlc.log').write_text(log + '\nTLC exceeded the 60-second timeout.\n')
        raise RuntimeError(f'{name}: TLC timed out; partial output retained in {folder / "tlc.log"}') from error
    log = result.stdout + result.stderr
    (folder / 'tlc.log').write_text(log)
    violations = [line for line in log.splitlines() if line.startswith('Error: Invariant ')]
    if result.returncode == 0 and 'Model checking completed. No error has been found.' in log:
        status = 'passed'
    elif result.returncode == 12 and violations:
        status = 'counterexample'
    else:
        raise RuntimeError(f'{name}: TLC did not complete a valid check; inspect {folder / "tlc.log"}')
    return {'status': status, 'exit_code': result.returncode, 'violations': violations,
            'command': command, 'cwd': str(folder), 'log': str(folder / 'tlc.log'),
            'states': [line for line in log.splitlines() if 'distinct states found' in line]}


def source_mutation_check(output):
    with tempfile.TemporaryDirectory(prefix='nemo-model-mutation-') as temporary:
        scratch = Path(temporary)
        for name in json.loads((MODEL / 'source-map.json').read_text()):
            target = scratch / name
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(ROOT / name, target)
        main = scratch / 'src/main.rs'
        before = main.read_text()
        prefix, body = before.split('fn run(mut config: Config)', 1)
        assert '    network::local_only()?;' in body
        body = body.replace('    network::local_only()?;', '    let _ = network::local_only();', 1)
        main.write_text(prefix + 'fn run(mut config: Config)' + body)
        try:
            check_sources(scratch)
        except RuntimeError as error:
            (output / 'source-mutation.txt').write_text(
                'Scratch Rust change: ignored the result of network::local_only() in run.\n'
                f'Drift gate rejected it: {error}\n'
                'This is source-drift detection, not automatic Rust-to-TLA refinement.\n')
        else:
            raise AssertionError('Source drift gate missed ignoring the real Rust confinement result')
    check_sources(ROOT)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--output', type=Path, required=True, help='new evidence directory')
    parser.add_argument('--jar', type=Path, default=ROOT / 'artifacts/formal-tools/tla2tools-1.7.4.jar')
    parser.add_argument('--fetch-tools', action='store_true', help='download the pinned developer-only TLC JAR if missing')
    parser.add_argument('--self-test', action='store_true', help='require four broken models and a scratch Rust edit to be caught')
    parser.add_argument('--privacy-only', action='store_true', help='omit the stronger ownership/retirement diagnostics with known counterexamples')
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    report = {'status': 'incomplete', 'model_sha256': digest(MODEL / 'Offline.tla'), 'cases': {}}
    try:
        check_sources(ROOT)
        java, version = java_command()
        jar = args.jar.resolve()
        if not jar.exists() and args.fetch_tools:
            data = urllib.request.urlopen(TLC_URL, timeout=30).read()
            if hashlib.sha256(data).hexdigest() != TLC_SHA256:
                raise RuntimeError('Downloaded TLC checksum mismatch')
            jar.parent.mkdir(parents=True, exist_ok=True)
            jar.write_bytes(data)
        if not jar.is_file() or digest(jar) != TLC_SHA256:
            raise RuntimeError('Missing or incorrect TLC 1.7.4 JAR; use --fetch-tools or --jar')
        report.update(java=version, tlc_sha256=TLC_SHA256, privacy_only=args.privacy_only)
        baseline = (MODEL / 'Offline.cfg').read_text()
        report['cases']['privacy'] = run_case(java, jar, output, 'privacy', baseline)
        if report['cases']['privacy']['status'] != 'passed':
            raise RuntimeError('Privacy invariant failed; preserve and investigate the counterexample')
        if args.self_test:
            mutations = [
                ('fail-open', 'FailClosed', 'OperationalConfined'),
                ('retained-helper', 'RetireHelper', 'OfflineNoDownloadAuthority'),
                ('helper-audio', 'IsolateHelperAudio', 'HelperHasNoAudio'),
                ('registered-only-kill', 'KillWholeGroup', 'CleanupRevokesDownloadAuthority'),
            ]
            constants = baseline.split('INVARIANTS', 1)[0]
            for name, constant, invariant in mutations:
                config = constants.replace(f'{constant} = TRUE', f'{constant} = FALSE') + f'INVARIANT {invariant}\n'
                case = run_case(java, jar, output, name, config)
                report['cases'][name] = case
                if case['status'] != 'counterexample' or not any(f'Invariant {invariant} is violated' in line for line in case['violations']):
                    raise RuntimeError(f'Mutation {name} was not caught by its intended invariant')
            source_mutation_check(output)
            report['source_mutation_caught'] = True
        if not args.privacy_only:
            for name in ['Ownership', 'Retirement']:
                report['cases'][name.lower()] = run_case(java, jar, output, name.lower(), (MODEL / f'{name}.cfg').read_text())
        check_sources(ROOT)
        assert digest(MODEL / 'Offline.tla') == report['model_sha256'], 'Model changed during verification'
        findings = [name for name in ('ownership', 'retirement')
                    if report['cases'].get(name, {}).get('status') == 'counterexample']
        report['status'] = 'findings' if findings else 'passed'
        report['findings'] = findings
    except BaseException as error:
        report['error'] = str(error)
        raise
    finally:
        (output / 'report.json').write_text(json.dumps(report, indent=2) + '\n')
    print(json.dumps({'status': report['status'], 'findings': report['findings'], 'report': str(output / 'report.json')}))
    return 2 if report['status'] == 'findings' else 0


if __name__ == '__main__':
    raise SystemExit(main())
