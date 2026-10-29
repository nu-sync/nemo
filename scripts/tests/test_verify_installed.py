import contextlib
import importlib.util
import io
import json
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / 'verify-installed.py'
SPEC = importlib.util.spec_from_file_location('verify_installed', SCRIPT)
verifier = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(verifier)


class CompletedSpeech:
    returncode = 0

    def poll(self):
        return 0


class InstalledVerifierCleanupTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='nemo-verifier-test-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name).resolve()
        self.app = self.root / 'Nemo.app'
        self.binary = self.app / 'Contents/MacOS/nemo'
        self.binary.parent.mkdir(parents=True)
        self.binary.write_bytes(b'fake native')
        self.runtime = self.root / 'nemo-speech'
        self.runtime.write_bytes(b'fake runtime')
        self.data = self.root / 'data'
        self.data.mkdir()
        self.log = self.data / 'daemon.log'
        self.log.write_text('Ready\n')
        (self.data / 'config.toml').write_text(
            f'runtime = {json.dumps(str(self.runtime))}\n'
            'idle_unload_seconds = 300\nmax_recording_seconds = 120\npaste = true\n'
        )
        self.evidence = self.root / 'evidence'
        self.status = 'Ready'
        self.doctor_calls = 0
        self.commands = []

    def run_scenario(self, shortcut_during_doctor):
        def control(_data, command):
            if command == 'permissions':
                return json.dumps({'accessibility': True, 'microphone': 'authorized'})
            self.assertEqual(command, 'status')
            return self.status

        def run(command, **_kwargs):
            self.assertEqual(command[0], str(self.binary))
            action = command[1]
            self.commands.append(action)
            if action == 'doctor':
                self.doctor_calls += 1
                text = 'Foreground app: TextEdit\n'
                if self.doctor_calls == 2:
                    if shortcut_during_doctor:
                        with self.log.open('a') as stream:
                            stream.write('Shortcut toggled\n')
                    text = 'Foreground app: ChatGPT\n'
            elif action == 'toggle':
                self.status = 'Recording'
                text = 'accepted\n'
            elif action == 'cancel':
                self.status = 'Canceled'
                text = 'accepted\n'
            else:
                self.fail(f'Unexpected process command: {command}')
            return subprocess.CompletedProcess(command, 0, stdout=text, stderr='')

        def output(command, **_kwargs):
            self.assertEqual(command, ['/usr/bin/pbpaste'])
            return b'private clipboard not persisted'

        def speech(command, **_kwargs):
            self.assertEqual(command[:3], ['/usr/bin/say', '-v', 'Samantha'])
            return CompletedSpeech()

        argv = ['verify-installed.py', '--app', str(self.app), '--data-dir', str(self.data),
                '--output', str(self.evidence), '--roundtrip-only', '--warm',
                '--paste-target', 'TextEdit']
        replacements = [
            (sys, 'argv', argv),
            (verifier, 'control', control),
            (verifier, 'processes', lambda: [(123, 1, str(self.binary)), (456, 123, str(self.runtime))]),
            (verifier.subprocess, 'run', run),
            (verifier.subprocess, 'check_output', output),
            (verifier.subprocess, 'Popen', speech),
            (verifier.time, 'sleep', lambda _seconds: None),
            (verifier.signal, 'signal', lambda *_args: None),
        ]
        with contextlib.ExitStack() as stack:
            for owner, name, value in replacements:
                stack.enter_context(mock.patch.object(owner, name, value))
            stack.enter_context(contextlib.redirect_stdout(io.StringIO()))
            exit_code = verifier.main()
        report = json.loads((self.evidence / 'results.json').read_text())
        self.assertEqual(exit_code, 2)
        self.assertEqual(report['status'], 'inconclusive')
        self.assertIn('before_stop', report['error'])
        return report

    def test_target_mismatch_cancels_the_owned_recording(self):
        report = self.run_scenario(shortcut_during_doctor=False)
        self.assertEqual(self.commands, ['doctor', 'toggle', 'doctor', 'cancel'])
        self.assertEqual(self.status, 'Canceled')
        self.assertNotIn('cleanup_ownership_relinquished', report)

    def test_shortcut_during_doctor_leaves_newer_user_recording_untouched(self):
        report = self.run_scenario(shortcut_during_doctor=True)
        self.assertEqual(self.commands, ['doctor', 'toggle', 'doctor'])
        self.assertNotIn('cancel', self.commands)
        self.assertEqual(self.status, 'Recording')
        self.assertEqual(report['cleanup_ownership_relinquished'],
                         'A user shortcut interrupted the controlled test')


if __name__ == '__main__':
    unittest.main()
