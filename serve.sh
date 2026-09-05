#!/bin/sh
set -eu

ROOT=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
cd "$ROOT"

if [ ! -x target/release/groupbot ]; then
    echo "target/release/groupbot is missing; run: cargo build --release" >&2
    exit 1
fi

for model in vision.onnx vision_text.onnx vision_text_vocab.txt vision_text_merges.txt intent.onnx intent_big.onnx intent_vocab.txt; do
    if [ ! -f "target/release/$model" ]; then
        echo "target/release/$model is missing; install the general filtering assets beside the binary" >&2
        exit 1
    fi
done

for tool in ffprobe ffmpeg; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "$tool is missing; animated GIF filtering needs the full-media decoder" >&2
        exit 1
    fi
done

if [ ! -f .env ]; then
    echo ".env is missing; copy .env.example and fill in the Telegram/Postgres values" >&2
    exit 1
fi

set -a
. ./.env
set +a

if [ "${1:-}" = "--check" ]; then
    echo "serve-ready binary=target/release/groupbot vision=target/release/vision.onnx text=target/release/vision_text.onnx"
    exit 0
fi

exec ./target/release/groupbot "$@"
