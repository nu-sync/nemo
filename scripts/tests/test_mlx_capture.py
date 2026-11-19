import http.client
import importlib.util
from pathlib import Path
import struct
import tempfile
import threading
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
