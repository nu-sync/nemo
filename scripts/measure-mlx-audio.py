#!/usr/bin/env -S uv run --script
# /// script
# requires-python = ">=3.12,<3.14"
# dependencies = [
#     "mlx==0.32.4",
#     "mlx-audio @ git+https://github.com/Blaizzy/mlx-audio.git@7a0986280fe295f4cf40f64164bb23abed416a22",
#     "numpy",
#     "huggingface-hub",
# ]
# ///
"""Measure MLX-Audio on the frozen corpus: single-token decode and MTP speculation.

CPU preflight by default. Only the coordinator passes --run, which takes the GPU
lock and requires the Nemo daemon to be idle.

    uv run --locked scripts/measure-mlx-audio.py --prepare
    uv run --locked scripts/measure-mlx-audio.py --run --nemo /path/to/nemo
"""
import argparse
import contextlib
import hashlib
import importlib.metadata as metadata
import json
import os
from pathlib import Path
import re
import shutil
import statistics
import subprocess
import sys
import time
import wave

ROOT = Path(__file__).resolve().parents[1]
OUT = ROOT / "artifacts" / "mlx-audio"
CACHE = Path.home() / 'Library/Caches/nemo-mlx'
REVISION = '7a0986280fe295f4cf40f64164bb23abed416a22'
CHECKPOINT = 'nvidia/NVIDIA-Nemotron-3.5-Transcribe-30B-A4B-NVFP4'
CLIPS = ('005s-lunch', '020s-weekend', '060s-design-review')


def sha256(path):
    digest = hashlib.sha256()
    with path.open('rb') as stream:
        for block in iter(lambda: stream.read(8 * 1024 * 1024), b''):
            digest.update(block)
    return digest.hexdigest()


def inventory(model, hashes=False):
    rows = []
    for path in sorted(model.rglob('*')):
        if path.is_file() and '.cache' not in path.relative_to(model).parts:
            before = path.stat()
            row = {'path': str(path.relative_to(model)), 'bytes': before.st_size}
            if hashes:
                row['sha256'] = sha256(path)
                after = path.stat()
                if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
                    raise RuntimeError(f'Checkpoint changed while hashing: {path}')
            rows.append(row)
    return rows


def words(text):
    return re.findall(r"\w+(?:'\w+)?", text.lower().replace('’', "'"))


def score(reference, hypothesis):
    ref, hyp = words(reference), words(hypothesis)
    row = list(range(len(hyp) + 1))
    for i, a in enumerate(ref, 1):
        nxt = [i]
        for j, b in enumerate(hyp, 1):
            nxt.append(min(row[j] + 1, nxt[-1] + 1, row[j - 1] + (a != b)))
        row = nxt
    return {'edits': row[-1], 'reference_words': len(ref), 'wer': row[-1] / len(ref)}


def prepare_audio():
    if not shutil.which('ffmpeg'):
        raise RuntimeError('ffmpeg is required for CPU-only FLAC conversion')
    dest = OUT / 'wav'
    dest.mkdir(exist_ok=True)
    rows = []
    manifest = json.loads((ROOT / 'benchmarks/speech/manifest.json').read_text())
    for name in CLIPS:
        source = ROOT / f'benchmarks/speech/audio/{name}.flac'
        reference = ROOT / f'benchmarks/speech/text/{name}.txt'
        # Verify against the frozen manifest before conversion, never regenerate fixtures.
        fixture = next(item for item in manifest['fixtures'] if item['id'] == name)
        for path, key in ((source, 'audio_sha256'), (reference, 'text_sha256')):
            if sha256(path) != fixture[key]:
                raise RuntimeError(f'Frozen corpus hash mismatched: {path}')
        target = dest / f'{name}.wav'
        subprocess.run(['ffmpeg', '-v', 'error', '-nostdin', '-y', '-i', str(source),
                        '-ar', '16000', '-ac', '1', '-c:a', 'pcm_s16le', str(target)], check=True)
        with wave.open(str(target)) as wav:
            assert (wav.getframerate(), wav.getnchannels(), wav.getsampwidth()) == (16000, 1, 2)
            assert wav.getnframes() == int(name[:3]) * 16000
        rows.append({'name': name, 'wav': str(target), 'wav_sha256': sha256(target),
                     'flac_sha256': sha256(source), 'reference_sha256': sha256(reference)})
    return rows


def preflight(model):
    problems = []
    versions = {d.metadata['Name']: d.version for d in metadata.distributions()}
    try:
        dist = metadata.distribution('mlx-audio')
        source = Path(dist.locate_file('mlx_audio/stt/models/nemotron_transcribe/nemotron_transcribe.py'))
        direct = json.loads(dist.read_text('direct_url.json') or '{}')
        if direct.get('vcs_info', {}).get('commit_id') != REVISION:
            problems.append(f'mlx-audio must be installed from pinned commit {REVISION}')
        if not source.is_file():
            problems.append('Nemotron Transcribe implementation missing')
    except metadata.PackageNotFoundError:
        problems.append('mlx-audio is not installed')
    required = {'config.json', 'tokenizer.json', 'tokenizer_config.json',
                'perception.safetensors', 'model-ctc-bf16.safetensors', 'model.safetensors.index.json'}
    index = model / 'model.safetensors.index.json'
    if index.exists():
        required.update(json.loads(index.read_text())['weight_map'].values())
    for name in sorted(required):
        if not (model / name).is_file():
            problems.append(f'Checkpoint file missing (download owner must finish): {name}')
    pending = list((model / '.cache').rglob('*.incomplete')) if (model / '.cache').exists() else []
    if pending:
        problems.append(f'{len(pending)} incomplete download files remain')
    return {'versions': versions, 'problems': problems, 'files': inventory(model)}


def idle(nemo):
    result = subprocess.run([nemo, 'status'], capture_output=True, text=True, timeout=10)
    status = result.stdout.strip()
    if result.returncode or not status.startswith('Ready') or re.search(r'recording|transcribing|opening microphone|loading model', status, re.I):
        raise RuntimeError(f'Nemo status must be verifiably idle: {status} {result.stderr.strip()}')
    return status


@contextlib.contextmanager
def gpu_lock():
    path = ROOT / 'artifacts/coordination/gpu.lock'
    fd = os.open(path, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
    identity = os.fstat(fd)
    try:
        with os.fdopen(fd, 'w') as stream:
            json.dump({'pid': os.getpid(), 'unit': 'U1c', 'started': time.time()}, stream)
        yield
    finally:
        if path.exists() and path.stat().st_ino == identity.st_ino:
            path.unlink()


def measure(args, info, clips):
    os.environ['HF_HUB_OFFLINE'] = '1'
    os.environ['TRANSFORMERS_OFFLINE'] = '1'
    with gpu_lock():
        status = idle(args.nemo)
        import mlx.core as mx
        from mlx_audio.stt import load
        from mlx_audio.stt.models.nemotron_transcribe import nemotron_transcribe as impl
        mx.reset_peak_memory()
        start = time.perf_counter()
        model = load(str(args.model), strict=True)
        mx.synchronize()
        load_seconds = time.perf_counter() - start
        if model.llm.mtp is None or model.ctc_timestamp is None:
            raise RuntimeError('Checkpoint must retain MTP and CTC heads')
        data = {'checkpoint': CHECKPOINT, 'engine_revision': REVISION, 'environment': info,
                'checkpoint_files': inventory(args.model, hashes=True), 'clips': clips,
                'load_seconds': load_seconds, 'load_peak_bytes': mx.get_peak_memory(),
                'macos': subprocess.check_output(['sw_vers'], text=True),
                'chip': subprocess.check_output(['sysctl', '-n', 'machdep.cpu.brand_string'], text=True).strip(),
                'initial_nemo_status': status, 'runs': [], 'summaries': []}
        original = {name: getattr(impl, name) for name in ('generate_step', 'speculative_generate_step')}
        timing = {}
        def instrument(function):
            def generate(*pos, **kw):
                iterator = function(*pos, **kw)
                count = 0
                started = None
                try:
                    for token in iterator:
                        count += 1
                        if count == 1:
                            mx.synchronize()
                            started = time.perf_counter()
                        yield token
                finally:
                    mx.synchronize()
                    timing.update(decode_seconds=time.perf_counter() - started if started else None,
                                  decode_tokens=max(0, count - 1))
                    iterator.close()
            return generate
        for name, function in original.items():
            setattr(impl, name, instrument(function))
        try:
            for clip in clips:
                reference = (ROOT / f"benchmarks/speech/text/{clip['name']}.txt").read_text()
                for draft in (0, 2, 4, 8):
                    group = []
                    for trial in range(args.repeats + 1):
                        idle(args.nemo)
                        timing.clear()
                        mx.reset_peak_memory()
                        mx.synchronize()
                        start = time.perf_counter()
                        result = model.generate(clip['wav'], speculative=bool(draft),
                                                num_draft_tokens=draft or 4, temperature=0,
                                                max_tokens=2048, task='asr', word_timestamps=False)
                        mx.synchronize()
                        elapsed = time.perf_counter() - start
                        tokens = result.generation_tokens
                        stats = result.speculative_stats
                        if not tokens or tokens >= 2048:
                            raise RuntimeError('Empty or capped output; invalid measurement')
                        if draft and (not stats or not stats.get('verification_steps')):
                            raise RuntimeError('Speculative decoding did not engage')
                        row = {'clip': clip['name'], 'draft_tokens': draft, 'trial': trial,
                               'warmup': trial == 0, 'text': result.text, 'generation_tokens': tokens,
                               'wall_seconds': elapsed, 'effective_ms_per_token': elapsed * 1000 / tokens,
                               'peak_memory_bytes': mx.get_peak_memory(), 'speculative_stats': stats,
                               'accept_rate': stats['accepted_tokens'] / stats['proposed_tokens']
                               if stats and stats['proposed_tokens'] else None,
                               **timing, **score(reference, result.text)}
                        row['decode_ms_per_token'] = (timing['decode_seconds'] * 1000 / timing['decode_tokens']
                                                      if timing.get('decode_tokens') else None)
                        data['runs'].append(row)
                        (OUT / 'results.json').write_text(json.dumps(data, indent=2) + '\n')
                        print(json.dumps(row), flush=True)
                        if trial:
                            group.append(row)
                    summary = {'clip': clip['name'], 'draft_tokens': draft,
                               'effective_median_ms': statistics.median(r['effective_ms_per_token'] for r in group),
                               'decode_median_ms': statistics.median(r['decode_ms_per_token'] for r in group),
                               'max_wer': max(r['wer'] for r in group)}
                    summary['effective_min_ms'] = min(r['effective_ms_per_token'] for r in group)
                    summary['effective_max_ms'] = max(r['effective_ms_per_token'] for r in group)
                    summary['peak_memory_median_bytes'] = statistics.median(r['peak_memory_bytes'] for r in group)
                    summary['accept_rate_median'] = statistics.median(r['accept_rate'] for r in group) if draft else None
                    summary['speculative_stats_medians'] = {key: statistics.median(r['speculative_stats'][key] for r in group)
                                                            for key in ('proposed_tokens', 'accepted_tokens', 'verification_steps')} if draft else None
                    summary['decode_ratio_to_8_38'] = summary['decode_median_ms'] / 8.38
                    summary['gate_pass'] = summary['effective_median_ms'] <= 6.3 and summary['max_wer'] == 0
                    data['summaries'].append(summary)
            (OUT / 'results.json').write_text(json.dumps(data, indent=2) + '\n')
            lines = ['# MLX-Audio measurement', '', f'Engine: `{REVISION}`. Load: {load_seconds:.3f} s.', '',
                     'Effective timing includes audio encoding, prefill, decoding and detokenization.',
                     'Decode timing starts after first yielded token and includes subsequent EOS verification.',
                     'The 8.38 ms/token comparison is descriptive: runtime workload equivalence needs coordinator review.',
                     'Gate uses conservative end-to-end effective timing and zero WER on every measured repeat.', '',
                     '| Clip | Draft | Effective median ms/token | Decode median ms/token | Max WER | Gate |',
                     '| --- | ---: | ---: | ---: | ---: | --- |']
            for row in data['summaries']:
                lines.append(f"| {row['clip']} | {row['draft_tokens']} | {row['effective_median_ms']:.3f} | {row['decode_median_ms']:.3f} | {row['max_wer']:.4f} | {row['gate_pass']} |")
            lines += ['', '| Clip | Draft | Median acceptance | Median proposed | Median accepted | Median verification steps |',
                      '| --- | ---: | ---: | ---: | ---: | ---: |']
            for row in data['summaries']:
                stats = row['speculative_stats_medians']
                if stats:
                    lines.append(f"| {row['clip']} | {row['draft_tokens']} | {row['accept_rate_median']:.4f} | {stats['proposed_tokens']} | {stats['accepted_tokens']} | {stats['verification_steps']} |")
            lines += ['', 'Raw repeats (including warmups), acceptance counts/rates, memory, file hashes, versions, and host identity: `results.json`.',
                      'No measured hardware bottleneck is established by this script; profile before attributing a speedup.',
                      'This synthetic three-clip result does not establish real-microphone quality.']
            (OUT / 'report.md').write_text('\n'.join(lines) + '\n')
        finally:
            for name, function in original.items():
                setattr(impl, name, function)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--model', type=Path, default=CACHE / 'NVFP4')
    parser.add_argument('--prepare', action='store_true', help='CPU-only WAV conversion and preflight')
    parser.add_argument('--run', action='store_true', help='COORDINATOR ONLY: acquire GPU lock and measure')
    parser.add_argument('--nemo', default='/Applications/Nemo.app/Contents/MacOS/nemo')
    parser.add_argument('--repeats', type=int, default=3)
    args = parser.parse_args()
    if args.repeats < 3:
        parser.error('at least three measured repeats are required')
    info = preflight(args.model)
    clips = prepare_audio() if args.prepare or args.run else []
    (OUT / 'preflight.json').write_text(json.dumps(info, indent=2) + '\n')
    print(json.dumps(info, indent=2))
    if args.run:
        if info['problems']:
            raise RuntimeError('Preflight failed; no GPU work started')
        measure(args, info, clips)
    return bool(info['problems'])


if __name__ == '__main__':
    sys.exit(main())
