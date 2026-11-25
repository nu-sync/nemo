from contextlib import closing
import http.client
import importlib.util
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import threading
import time
from unittest.mock import patch
import unittest
import wave


SCRIPT = Path(__file__).resolve().parents[1] / "mlx-serve.py"
SPEC = importlib.util.spec_from_file_location("mlx_serve", SCRIPT)
serve = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(serve)


def write_pcm(path, pcm, rate=48000, channels=1):
    with wave.open(path, "wb") as audio:
        audio.setparams((channels, 2, rate, 0, "NONE", "not compressed"))
        audio.writeframes(pcm)


class MlxCaptureTests(unittest.TestCase):
    @unittest.skipUnless(shutil.which("uv"), "requires uv")
    def test_runner_execs_the_locked_script_environment_with_a_stale_cache(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            stale = root / ".cache/uv/environments-v2/mlx-serve-old/bin/python"
            stale.parent.mkdir(parents=True)
            stale.write_text("#!/bin/sh\nexit 42\n")
            stale.chmod(0o755)
            script = root / "mlx-serve.py"
            script.write_text(
                "# /// script\n# dependencies = []\n# ///\n"
                "import os, sys\nprint(os.getpid(), sys.executable)\n"
            )
            env = dict(os.environ, HOME=directory, UV_CACHE_DIR=str(root / "uv"),
                       UV_PYTHON=sys.executable, NEMO_MLX_SCRIPT=str(script), NEMO_ENGINE_CAP="99")
            subprocess.run(["uv", "lock", "--offline", "--script", script],
                           env=env, check=True, capture_output=True)
            with subprocess.Popen([SCRIPT.with_name("mlx-runner.sh")], env=env,
                                  stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True) as process:
                stdout, stderr = process.communicate(timeout=10)
                self.assertEqual(process.returncode, 0, stderr)
                pid, interpreter = stdout.strip().split(" ", 1)
                self.assertEqual(int(pid), process.pid)
                self.assertTrue(Path(interpreter).is_relative_to(root / "uv"))

    @unittest.skipUnless(importlib.util.find_spec("mlx_audio"), "requires the MLX helper environment")
    def test_capture_matches_mlx_audio_filter_across_chunk_boundaries(self):
        import numpy as np
        from mlx_audio.audio_io import read

        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "capture.wav")
            # Short captures and the three possible decimation phases at a block edge.
            for frames in (1, 2, 7805, 7806, 7807, 48000 + 23):
                with self.subTest(frames=frames):
                    pcm = struct.pack(
                        f"<{frames}h", *((index * 1777) % 65536 - 32768 for index in range(frames))
                    )
                    write_pcm(path, pcm)

                    samples = serve.resample_capture(path, 16000)
                    reference, _ = read(path, dtype="float32", sample_rate=16000, nchannels=1)
                    self.assertEqual(samples.dtype, np.dtype("float32"))
                    self.assertEqual(samples.shape, ((frames + 2) // 3,))
                    np.testing.assert_allclose(samples, reference, atol=1e-7, rtol=0)

    @unittest.skipUnless(importlib.util.find_spec("mlx"), "requires the MLX helper environment")
    def test_gpu_wake_requires_the_key_and_works_without_a_speech_model(self):
        class Handler(serve.Handler):
            key = "fixture-key"
            model = None

        with serve.HTTPServer(("127.0.0.1", 0), Handler) as server:
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            try:
                for token, expected in [("wrong", 401), (Handler.key, 200)]:
                    with self.subTest(token=token):
                        client = http.client.HTTPConnection(*server.server_address, timeout=3)
                        try:
                            client.request("POST", "/v1/gpu/wake", body=b"",
                                           headers={"Authorization": f"Bearer {token}"})
                            response = client.getresponse()
                            self.assertEqual(response.status, expected)
                            response.read()
                        finally:
                            client.close()
            finally:
                server.shutdown()
                thread.join()

    def test_final_request_interrupts_an_active_preview(self):
        started = threading.Event()

        class Handler(serve.Handler):
            draft_head = True

            def preview(self, path):
                started.set()
                deadline = time.monotonic() + 5
                while time.monotonic() < deadline:
                    self.preview_checkpoint()
                    time.sleep(.005)
                return {"tokens": [42]}

        def post(path, body, headers):
            with closing(http.client.HTTPConnection(*server.server_address, timeout=3)) as client:
                client.request("POST", path, body, headers)
                response = client.getresponse()
                return response.status, response.read()

        with serve.HTTPServer(("127.0.0.1", 0), Handler) as server, patch.object(
            serve, "transcribe", return_value="finished"
        ):
            thread = threading.Thread(target=server.serve_forever, daemon=True)
            thread.start()
            result = []
            try:
                preview = threading.Thread(target=lambda: result.append(post("/v1/audio/preview", b"wav", {})))
                preview.start()
                self.assertTrue(started.wait(2))
                body = b'--x\r\nContent-Disposition: form-data; name="file"\r\n\r\nwav\r\n--x--\r\n'
                self.assertEqual(post("/v1/audio/transcriptions", body,
                                      {"Content-Type": "multipart/form-data; boundary=x"}),
                                 (200, b'{"text": "finished"}'))
                preview.join(2)
                self.assertEqual(result, [(200, b'{"tokens": []}')])
            finally:
                server.shutdown()
                thread.join()

    def test_other_audio_and_truncated_capture_use_the_model_reader(self):
        with tempfile.TemporaryDirectory() as directory:
            path = str(Path(directory) / "recording.wav")
            for rate, channels, model_rate in [(16000, 1, 16000), (48000, 2, 16000), (48000, 1, 8000)]:
                with self.subTest(rate=rate, channels=channels, model_rate=model_rate):
                    write_pcm(path, b"\x00\x00" * channels * 480, rate, channels)
                    self.assertEqual(serve.resample_capture(path, model_rate), path)

            write_pcm(path, b"\x00\x00" * 480)
            file = Path(path)
            file.write_bytes(file.read_bytes()[:-2])
            self.assertEqual(serve.resample_capture(path, 16000), path)
