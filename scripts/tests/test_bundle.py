import hashlib
import json
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import sys
import tempfile
import unittest


PROJECT = Path(__file__).resolve().parents[2]


class BundleTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='nemo-bundle-test-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.script = self.root / 'scripts/bundle.py'
        self.script.parent.mkdir()
        shutil.copyfile(PROJECT / 'scripts/bundle.py', self.script)
        shutil.copyfile(PROJECT / 'scripts/entitlements.plist', self.script.parent / 'entitlements.plist')
        self.write('LICENSE', 'Nemo license\n')
        self.write('assets/sounds/Handy-MIT.txt', 'Handy license\n')
        self.write('models/LICENSE', 'Model license\n')
        self.write('models/manifest.json', '{"revision":"test"}\n')
        for name in ['LICENSE', 'NOTICE', 'THIRD_PARTY_NOTICES.md']:
            self.write('vendor/NeMo-Speech.cpp/' + name, 'Native ' + name + '\n')
        self.tools = self.root / 'tools'
        self.tools.mkdir()
        self.environment = {**os.environ, 'PATH': str(self.tools), 'NEMO_SIGN_IDENTITY': '-'}

    def write(self, relative, text):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(text)
        return path

    def executable(self, relative):
        path = self.root / relative
        path.parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile('/usr/bin/true', path)
        path.chmod(0o755)
        return path

    def bundle(self, *arguments):
        result = subprocess.run([sys.executable, str(self.script), *map(str, arguments)],
                                env=self.environment, capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr or result.stdout)
        return Path(result.stdout.strip())

    def read_version(self, app):
        with (app / 'Contents/Info.plist').open('rb') as source:
            return plistlib.load(source)['CFBundleVersion']

    def verify(self, app):
        result = subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict', str(app)],
                                capture_output=True, text=True)
        self.assertEqual(result.returncode, 0, result.stderr)

    def test_explicit_inputs_package_without_cargo_and_sign_the_original_binary_receipt(self):
        binary = self.executable('cargo-bin/nemo')
        native = self.executable('native-bin/nemo-speech')
        digest = hashlib.sha256(binary.read_bytes()).hexdigest()
        app = self.bundle('--binary', binary, '--native-dir', native.parent,
                          '--version', '9.8.7', '--output', self.root / 'stage/Nemo.app')
        self.assertEqual(self.read_version(app), '9.8.7')
        receipt = app / 'Contents/Resources/install-receipt.json'
        self.assertEqual(json.loads(receipt.read_text()), {'source_sha256': digest, 'version': '9.8.7'})
        self.assertEqual(hashlib.sha256(binary.read_bytes()).hexdigest(), digest)
        self.assertTrue((app / 'Contents/Resources/runtime/nemo-speech').is_file())
        self.assertEqual((app / 'Contents/Resources/licenses/Handy-MIT.txt').read_text(), 'Handy license\n')
        self.verify(app)
        receipt.write_text('{}\n')
        changed = subprocess.run(['/usr/bin/codesign', '--verify', '--deep', '--strict', str(app)],
                                 capture_output=True)
        self.assertNotEqual(changed.returncode, 0, 'The install receipt must be covered by the app signature')

    def test_default_paths_and_cargo_metadata_remain_supported(self):
        self.executable('target/release/nemo')
        self.executable('vendor/NeMo-Speech.cpp/build/metal-server/bin/nemo-speech')
        cargo = self.tools / 'cargo'
        cargo.write_text('#!/bin/sh\nprintf \'%s\\n\' \'{"packages":[{"name":"nemo-dictate","version":"1.2.3"}]}\'\n')
        cargo.chmod(0o755)
        app = self.bundle()
        self.assertEqual(app, self.root / 'dist/Nemo.app')
        self.assertEqual(self.read_version(app), '1.2.3')
        self.assertEqual(json.loads((app / 'Contents/Resources/install-receipt.json').read_text())['version'], '1.2.3')
        self.verify(app)


if __name__ == '__main__':
    unittest.main()
