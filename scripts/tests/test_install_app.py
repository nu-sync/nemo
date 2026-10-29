import importlib.util
import os
from pathlib import Path
import plistlib
import shutil
import subprocess
import tempfile
import unittest
from unittest import mock

SCRIPT = Path(__file__).resolve().parents[1] / 'install_app.py'
SPEC = importlib.util.spec_from_file_location('install_app', SCRIPT)
installer = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(installer)


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='nemo-installer-test-')
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.source = self.root / 'source/Nemo.app'
        self.destination = self.root / 'Applications/Nemo.app'
        self.make_app(self.source, '0.2.0')

    def make_app(self, path, version, identifier=installer.IDENTIFIER):
        (path / 'Contents/MacOS').mkdir(parents=True)
        shutil.copyfile('/usr/bin/true', path / 'Contents/MacOS/nemo')
        (path / 'Contents/MacOS/nemo').chmod(0o755)
        with (path / 'Contents/Info.plist').open('wb') as output:
            plistlib.dump({'CFBundleIdentifier': identifier, 'CFBundleExecutable': 'nemo', 'CFBundleName': 'Nemo', 'CFBundleVersion': version}, output)
        subprocess.run(['/usr/bin/codesign', '--force', '--sign', '-', str(path)], check=True, capture_output=True)

    def old_app(self):
        self.make_app(self.destination, '0.1.0')

    def version(self, path):
        return installer.info(path)['CFBundleVersion']

    def test_fresh_install_copies_a_verified_bundle(self):
        self.assertIsNone(installer.install(self.source, self.destination))
        self.assertEqual(self.version(self.destination), '0.2.0')
        installer.verify(self.destination)
        self.assertTrue(self.source.exists())

    def test_same_signed_bundle_is_recognized_as_same_identity(self):
        shutil.copytree(self.source, self.destination)
        self.assertTrue(installer.same_identity(self.source, self.source))
        self.assertTrue(installer.same_identity(self.source, self.destination))

    def test_identical_signed_upgrade_does_not_require_permission_reset(self):
        shutil.copytree(self.source, self.destination)
        backup = installer.install(self.source, self.destination)
        self.assertEqual(self.version(self.destination), '0.2.0')
        self.assertEqual(self.version(backup), '0.2.0')
        installer.verify(self.destination)
        installer.verify(backup)

    def test_changed_identity_is_rejected_before_stopping_the_working_app(self):
        self.old_app()
        with mock.patch.object(installer, 'stop_idle_app') as stop:
            with self.assertRaisesRegex(RuntimeError, 'code identity'):
                installer.install(self.source, self.destination)
            stop.assert_not_called()
        self.assertEqual(self.version(self.destination), '0.1.0')

    def test_explicitly_reviewed_upgrade_retains_the_previous_app(self):
        self.old_app()
        backup = installer.install(self.source, self.destination, allow_permission_reset=True)
        self.assertEqual(self.version(self.destination), '0.2.0')
        self.assertEqual(self.version(backup), '0.1.0')
        installer.verify(backup)

    def test_staging_failure_leaves_the_old_app_untouched(self):
        self.old_app()
        with mock.patch.object(installer, 'copy_bundle', side_effect=OSError('disk full')), mock.patch.object(installer, 'stop_idle_app') as stop:
            with self.assertRaisesRegex(OSError, 'disk full'):
                installer.install(self.source, self.destination, allow_permission_reset=True)
            stop.assert_not_called()
        self.assertEqual(self.version(self.destination), '0.1.0')

    def test_commit_failure_restores_the_previous_bundle(self):
        self.old_app()
        replace = os.replace

        def failing_replace(source, destination):
            if Path(destination) == self.destination and '.install-' in str(source):
                raise OSError('commit failed')
            replace(source, destination)

        with mock.patch.object(installer.os, 'replace', side_effect=failing_replace):
            with self.assertRaisesRegex(OSError, 'commit failed'):
                installer.install(self.source, self.destination, allow_permission_reset=True)
        self.assertEqual(self.version(self.destination), '0.1.0')
        installer.verify(self.destination)

    def test_backup_failure_reopens_the_unchanged_app(self):
        self.old_app()
        run = installer.run
        reopened = []

        def checked_run(*args):
            if args[0] == '/usr/bin/open':
                reopened.append(Path(args[1]))
                return ''
            return run(*args)

        with mock.patch.object(installer.os, 'replace', side_effect=OSError('backup failed')), mock.patch.object(installer, 'stop_idle_app', return_value=True), mock.patch.object(installer, 'run', side_effect=checked_run):
            with self.assertRaisesRegex(OSError, 'backup failed'):
                installer.install(self.source, self.destination, allow_permission_reset=True)
        self.assertEqual(self.version(self.destination), '0.1.0')
        self.assertEqual(reopened, [self.destination])

    def test_failed_launch_restores_and_reopens_the_previous_app(self):
        self.old_app()
        run = installer.run
        launched_versions = []

        def checked_run(*args):
            if args[0] == '/usr/bin/open':
                version = self.version(Path(args[1]))
                launched_versions.append(version)
                if version == '0.2.0':
                    raise RuntimeError('launch failed')
                return ''
            return run(*args)

        with mock.patch.object(installer, 'stop_idle_app', return_value=True), mock.patch.object(installer, 'run', side_effect=checked_run):
            with self.assertRaisesRegex(RuntimeError, 'launch failed'):
                installer.install(self.source, self.destination, allow_permission_reset=True)
        self.assertEqual(self.version(self.destination), '0.1.0')
        self.assertEqual(launched_versions, ['0.2.0', '0.1.0'])

    def test_post_install_failure_rolls_back_and_reopens_a_previously_running_app(self):
        self.old_app()
        verify = installer.verify
        run = installer.run
        reopened = []

        def failing_verify(app):
            verify(app)
            if Path(app) == self.destination and self.version(app) == '0.2.0':
                raise RuntimeError('post-install failure')

        def checked_run(*args):
            if args[0] == '/usr/bin/open':
                reopened.append(Path(args[1]))
                return ''
            return run(*args)

        with mock.patch.object(installer, 'verify', side_effect=failing_verify), mock.patch.object(installer, 'stop_idle_app', return_value=True), mock.patch.object(installer, 'run', side_effect=checked_run):
            with self.assertRaisesRegex(RuntimeError, 'post-install failure'):
                installer.install(self.source, self.destination, allow_permission_reset=True)
        self.assertEqual(self.version(self.destination), '0.1.0')
        self.assertEqual(reopened, [self.destination])

    def test_unrelated_app_is_never_replaced(self):
        self.make_app(self.destination, '1', 'example.unrelated')
        with self.assertRaisesRegex(RuntimeError, 'unrelated'):
            installer.install(self.source, self.destination, allow_permission_reset=True)
        with (self.destination / 'Contents/Info.plist').open('rb') as source:
            self.assertEqual(plistlib.load(source)['CFBundleIdentifier'], 'example.unrelated')

    def test_busy_app_is_not_quit(self):
        with mock.patch.object(installer, 'running_pids', return_value=[123]), mock.patch.object(installer, 'run', return_value='Recording') as run:
            with self.assertRaisesRegex(RuntimeError, 'busy'):
                installer.stop_idle_app(self.destination, self.source)
        self.assertEqual(len(run.call_args_list), 1)
        self.assertEqual(run.call_args.args[-1], 'status')

    def test_recording_started_after_preflight_prevents_replacement(self):
        self.old_app()
        real_run = installer.run

        def checked_run(*args):
            if len(args) > 1 and args[1] == 'status':
                return 'Ready'
            if len(args) > 1 and args[1] == 'quit':
                self.assertEqual(args[-1], '--if-idle')
                raise RuntimeError('Safe quit refused (busy)')
            return real_run(*args)

        with mock.patch.object(installer, 'running_pids', return_value=[123]), mock.patch.object(installer, 'run', side_effect=checked_run):
            with self.assertRaisesRegex(RuntimeError, 'busy'):
                installer.install(self.source, self.destination, allow_permission_reset=True)
        self.assertEqual(self.version(self.destination), '0.1.0')


if __name__ == '__main__':
    unittest.main()
