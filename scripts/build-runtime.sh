#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
revision=6a62501107e1894723880f1094a286ea996a0ec7
runtime=vendor/NeMo-Speech.cpp
export PATH="/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin:$PATH"
if [[ ! -d "$runtime/.git" ]]; then
    git clone https://github.com/NVIDIA/NeMo-Speech.cpp "$runtime"
    git -C "$runtime" checkout "$revision"
fi
if [[ "$(git -C "$runtime" rev-parse HEAD)" != "$revision" ]]; then
    echo "Runtime revision differs from the tested revision: $revision" >&2
    exit 1
fi
git -C "$runtime" submodule update --init --depth 1 llama.cpp third_party/cpp-httplib
cd "$runtime"
scripts/configure.sh metal-server \
    -DNEMO_SPEECH_BUILD_TTS=OFF \
    -DNEMO_SPEECH_BUILD_NMT=OFF \
    -DNEMO_SPEECH_BUILD_DIAR=OFF \
    -DNEMO_SPEECH_BUILD_MIC_CAPTURE=OFF \
    -DCMAKE_PREFIX_PATH="$(brew --prefix)"
cmake --build --preset metal-server -j "$(sysctl -n hw.ncpu)"
