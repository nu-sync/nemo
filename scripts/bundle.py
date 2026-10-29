#!/usr/bin/env python3
"""Bundle the Rust daemon and the native Metal runtime without external dylib paths."""
import argparse
import hashlib
import json
import os
import pathlib
import plistlib
import shutil
import subprocess
import tempfile

root = pathlib.Path(__file__).resolve().parent.parent
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--output', type=pathlib.Path, default=root / 'dist/Nemo.app')
parser.add_argument('--binary', type=pathlib.Path, default=root / 'target/release/nemo')
parser.add_argument('--native-dir', type=pathlib.Path, default=root / 'vendor/NeMo-Speech.cpp/build/metal-server/bin')
parser.add_argument('--version', help='bundle version; omit to read it from Cargo metadata')
args = parser.parse_args()
output = args.output.absolute()
if output.suffix != '.app':
    raise SystemExit('Output must end in .app')
if output.resolve() == (pathlib.Path.home() / 'Applications/Nemo.app').resolve():
    raise SystemExit('Packaging cannot overwrite the installed app. Use install_app.py after review.')
output.parent.mkdir(parents=True, exist_ok=True)
workspace = tempfile.TemporaryDirectory(prefix='.nemo-package-', dir=output.parent)
app = pathlib.Path(workspace.name) / 'Nemo.app'
contents = app / 'Contents'
runtime = contents / 'Resources/runtime'
runtime.mkdir(parents=True, exist_ok=True)
(contents / 'MacOS').mkdir(exist_ok=True)
shutil.copy2(args.binary, contents / 'MacOS/nemo')
source_sha256 = hashlib.sha256((contents / 'MacOS/nemo').read_bytes()).hexdigest()
native = args.native_dir.absolute()
queue = [native / 'nemo-speech']
copied = set()
identity = os.environ.get('NEMO_SIGN_IDENTITY', '-')
signing = ['/usr/bin/codesign', '--force', '--sign', identity]
if identity != '-':
    signing.extend(['--timestamp', '--options', 'runtime'])

def run(*args):
    result = subprocess.run(args, text=True, capture_output=True)
    if result.returncode:
        raise SystemExit(result.stderr or result.stdout)

while queue:
    source = queue.pop()
    if source.name in copied:
        continue
    copied.add(source.name)
    destination = runtime / source.name
    destination.unlink(missing_ok=True)
    shutil.copy2(source, destination)
    destination.chmod(destination.stat().st_mode | 0o200)
    dependencies = subprocess.check_output(['/usr/bin/otool', '-L', str(source)], text=True).splitlines()[1:]
    changes = []
    for line in dependencies:
        dep = line.strip().split(' (')[0]
        if dep.startswith(('/System/', '/usr/lib/')):
            continue
        name = pathlib.Path(dep).name
        candidates = [pathlib.Path(dep), source.parent / name, native / name, pathlib.Path('/opt/homebrew/lib') / name]
        resolved = next((p for p in candidates if p.is_file()), None)
        if resolved is None:
            raise SystemExit(f'Cannot resolve {dep} used by {source}')
        if name == source.name:
            continue
        queue.append(resolved)
        changes.extend(['-change', dep, '@loader_path/' + name])
    if source.suffix == '.dylib':
        changes.extend(['-id', '@rpath/' + source.name])
    if changes:
        run('/usr/bin/install_name_tool', *changes, str(destination))

for library in runtime.glob('*.dylib'):
    run(*signing, str(library))
run(*signing, str(runtime / 'nemo-speech'))
version = args.version
if version is None:
    metadata = json.loads(subprocess.check_output(['cargo', 'metadata', '--no-deps', '--locked', '--format-version', '1', '--manifest-path', str(root / 'Cargo.toml')], text=True))
    version = next(package['version'] for package in metadata['packages'] if package['name'] == 'nemo-dictate')

plist = {
    'CFBundleExecutable': 'nemo',
    'CFBundleIdentifier': 'computer.nemo.dictate',
    'CFBundleName': 'Nemo',
    'CFBundleDisplayName': 'Nemo',
    'CFBundlePackageType': 'APPL',
    'CFBundleShortVersionString': version,
    'CFBundleVersion': version,
    'LSMinimumSystemVersion': '26.0',
    'LSUIElement': True,
    'NSMicrophoneUsageDescription': 'Nemo records your voice when you toggle dictation. Audio stays on this Mac.',
    'NSHighResolutionCapable': True,
}
with (contents / 'Info.plist').open('wb') as plist_file:
    plistlib.dump(plist, plist_file)
licenses = contents / 'Resources/licenses'
licenses.mkdir(exist_ok=True)
shutil.copy2(root / 'LICENSE', licenses / 'Nemo-MIT.txt')
shutil.copy2(root / 'assets/sounds/Handy-MIT.txt', licenses / 'Handy-MIT.txt')
if (root / 'models/LICENSE').is_file():
    shutil.copy2(root / 'models/LICENSE', licenses / 'Model-OpenMDW-1.1.txt')
for name in ['LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md']:
    shutil.copy2(root / 'vendor/NeMo-Speech.cpp' / name, licenses / ('NeMo-Speech-' + name))
shutil.copy2(root / 'models/manifest.json', contents / 'Resources/model-manifest.json')
(contents / 'Resources/install-receipt.json').write_text(
    json.dumps({'source_sha256': source_sha256, 'version': version}, indent=2) + '\n'
)
run(*signing, '--entitlements', str(root / 'scripts/entitlements.plist'), str(app))
run('/usr/bin/codesign', '--verify', '--deep', '--strict', str(app))
previous = pathlib.Path(workspace.name) / 'previous.app'
if output.exists():
    os.replace(output, previous)
try:
    os.replace(app, output)
except BaseException:
    if previous.exists():
        os.replace(previous, output)
    raise
workspace.cleanup()
print(output)
