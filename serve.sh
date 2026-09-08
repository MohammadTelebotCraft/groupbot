#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$ROOT"

if [ ! -f .env ]; then
    echo ".env is missing; copy .env.example and fill in the Telegram/Postgres values" >&2
    exit 1
fi

set -a
. ./.env
set +a

if [ ! -x target/release/groupbot ]; then
    echo "target/release/groupbot is missing; run: cargo build --release" >&2
    exit 1
fi

MODEL_DIR=${VISION_FILES-target/release}
if [ -z "$MODEL_DIR" ]; then
    echo "VISION_FILES is present but empty" >&2
    exit 1
fi
for model in vision.onnx vision_text.onnx vision_text_vocab.txt vision_text_merges.txt intent.onnx intent_big.onnx intent_vocab.txt; do
    if [ ! -f "$MODEL_DIR/$model" ]; then
        echo "$MODEL_DIR/$model is missing; install the general filtering assets in the configured model directory" >&2
        exit 1
    fi
done

for tool in ffprobe ffmpeg; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "$tool is missing; animated GIF filtering needs the full-media decoder" >&2
        exit 1
    fi
done

if [ "${1:-}" = "--check" ]; then
    echo "serve-ready binary=target/release/groupbot models=$MODEL_DIR"
    exit 0
fi

exec ./target/release/groupbot "$@"
