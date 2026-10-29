import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


SCRIPT = Path(__file__).resolve().parents[1] / 'build-runtime.sh'


class RuntimeSourceSetupTests(unittest.TestCase):
    def test_failed_pinned_checkout_can_be_retried_without_publishing_partial_source(self):
        with tempfile.TemporaryDirectory(prefix='nemo-runtime-source-test-') as temporary:
            root = Path(temporary)
            upstream = root / 'upstream'
            subprocess.run(['git', 'init', '-q', str(upstream)], check=True)
            (upstream / 'fixture.txt').write_text('Local source fixture\n')
            subprocess.run(['git', '-C', str(upstream), 'add', 'fixture.txt'], check=True)
            subprocess.run([
                'git', '-C', str(upstream), '-c', 'user.name=Nemo Test',
                '-c', 'user.email=test@example.invalid', '-c', 'commit.gpgsign=false',
                'commit', '-qm', 'Add fixture',
            ], check=True)
            scripts = root / 'checkout/scripts'
            scripts.mkdir(parents=True)
            shutil.copyfile(SCRIPT, scripts / SCRIPT.name)
            environment = {
                **os.environ,
                'GIT_CONFIG_COUNT': '1',
                'GIT_CONFIG_KEY_0': f'url.{upstream.as_uri()}.insteadOf',
                'GIT_CONFIG_VALUE_0': 'https://github.com/NVIDIA/NeMo-Speech.cpp',
                'GIT_ALLOW_PROTOCOL': 'file',
            }
            for _ in range(2):
                result = subprocess.run(
                    ['/bin/bash', str(scripts / SCRIPT.name)], env=environment,
                    capture_output=True, text=True, timeout=15,
                )
                self.assertNotEqual(result.returncode, 0)
                self.assertIn('6a62501107e1894723880f1094a286ea996a0ec7', result.stderr)
                self.assertFalse((root / 'checkout/vendor/NeMo-Speech.cpp').exists())
                self.assertEqual(list((root / 'checkout/vendor').iterdir()), [])


if __name__ == '__main__':
    unittest.main()
