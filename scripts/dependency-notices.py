#!/usr/bin/env python3
"""Generate locked macOS Rust dependency notices from locally cached sources."""
import argparse
import hashlib
import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
DESTINATION = ROOT / 'assets/licenses/Rust-dependencies.txt'


def notices():
    metadata = json.loads(subprocess.check_output([
        'cargo', 'metadata', '--locked', '--offline', '--format-version', '1',
        '--filter-platform', 'aarch64-apple-darwin',
    ], cwd=ROOT))
    selected = {node['id'] for node in metadata['resolve']['nodes']}
    lock_digest = hashlib.sha256((ROOT / 'Cargo.lock').read_bytes()).hexdigest()
    parts = [f'Rust dependency notices\n\nCargo.lock SHA256: {lock_digest}\nGenerated with scripts/dependency-notices.py from Cargo.lock.\nIncludes build and test dependencies for Apple Silicon macOS.\n']
    texts = {}
    for package in sorted(metadata['packages'], key=lambda p: (p['name'], p['version'])):
        if package['id'] not in selected or package['source'] is None:
            continue
        root = pathlib.Path(package['manifest_path']).parent
        files = [p for p in root.iterdir() if p.is_file() and p.name.upper().startswith(('LICENSE', 'LICENCE', 'COPYING', 'NOTICE'))]
        if (root / 'LICENSES').is_dir():
            files += [p for p in (root / 'LICENSES').rglob('*') if p.is_file()]
        if not files:
            override = ROOT / f"assets/licenses/upstream/{package['name']}-{package['version']}.txt"
            if override.is_file():
                files = [override]
        if not package['license'] or not files:
            raise RuntimeError(f"Missing license metadata/text: {package['name']} {package['version']}")
        parts.append(f"\n{'=' * 72}\n{package['name']} {package['version']}\nSPDX: {package['license']}\nSource: https://crates.io/crates/{package['name']}/{package['version']}\n")
        for file in sorted(files):
            label = file.relative_to(root) if file.is_relative_to(root) else file.name
            body = file.read_text().strip()
            digest = hashlib.sha256(body.encode()).hexdigest()
            texts[digest] = body
            parts.append(f'  {label}: {digest}\n')
    for digest, body in sorted(texts.items()):
        parts.append(f"\n{'=' * 72}\nLicense text: {digest}\n\n{body}\n")
    return ''.join(parts)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--check', action='store_true')
    args = parser.parse_args()
    text = notices()
    if args.check:
        if not DESTINATION.exists() or DESTINATION.read_text() != text:
            raise SystemExit('Rust notices are stale; run python3 scripts/dependency-notices.py')
    else:
        DESTINATION.parent.mkdir(parents=True, exist_ok=True)
        DESTINATION.write_text(text)
    print(f'Rust notices verified: {DESTINATION.relative_to(ROOT)}')


if __name__ == '__main__':
    main()
