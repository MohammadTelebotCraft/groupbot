#!/usr/bin/env python3
"""Long-running, label-free stability test for the HyperVision model stack.

This does not tune thresholds or collect annotations. It repeatedly exercises the exact three
CPU sessions used by the bot, checks tensor shapes/finiteness, and reports only binary outcomes
and throughput. Use ``--hours 1`` before a production rollout, or a short ``--seconds`` value in
CI/local development.
"""

from __future__ import annotations

import argparse
import pathlib
import time

import numpy as np
import onnxruntime as ort
from PIL import Image


ROOT = pathlib.Path(__file__).resolve().parents[1]
SIDE = 384
GRADE_SIDE = 224


def session(path: pathlib.Path) -> ort.InferenceSession:
    options = ort.SessionOptions()
    options.intra_op_num_threads = 4
    options.inter_op_num_threads = 1
    options.execution_mode = ort.ExecutionMode.ORT_SEQUENTIAL
    return ort.InferenceSession(str(path), options, providers=["CPUExecutionProvider"])


def load_frame(path: pathlib.Path | None, side: int) -> np.ndarray:
    if path is None:
        # Deterministic texture exercises resize/normalisation without writing test media.
        y, x = np.mgrid[:side, :side]
        image = np.stack(((x * 7 + y * 13) % 256, (x * 3) % 256, (y * 5) % 256), axis=-1)
        return image.astype(np.float32).transpose(2, 0, 1) / 127.5 - 1.0
    image = Image.open(path).convert("RGB").resize((side, side), Image.Resampling.BICUBIC)
    return np.asarray(image, dtype=np.float32).transpose(2, 0, 1) / 127.5 - 1.0


def outputs(session_: ort.InferenceSession, pixels: np.ndarray) -> list[np.ndarray]:
    name = session_.get_inputs()[0].name
    return [np.asarray(value, dtype=np.float32) for value in session_.run(None, {name: pixels})]


def vector_output(values: list[np.ndarray], width: int) -> np.ndarray:
    for value in values:
        if value.size == width:
            vector = value.reshape(-1)
            norm = float(np.linalg.norm(vector))
            if norm > 0 and np.isfinite(norm):
                return vector / norm
    raise RuntimeError(f"no finite {width}-wide embedding output")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--hours", type=float, default=1.0)
    parser.add_argument("--seconds", type=float)
    parser.add_argument("--vision-dir", type=pathlib.Path, default=ROOT / "target" / "release")
    parser.add_argument("--warmup", type=int, default=2)
    args = parser.parse_args()
    duration = args.seconds if args.seconds is not None else args.hours * 3600.0
    if duration <= 0:
        raise SystemExit("duration must be positive")

    main_model = session(ROOT / "assets" / "nsfw.onnx")
    grade_model = session(ROOT / "assets" / "nsfw_grade.onnx")
    vision_model = session(args.vision_dir / "vision.onnx")
    frames = sorted((ROOT / "target" / "imgfilter-eval").glob("**/*.jpg"))[:64]
    frame_index = 0

    def run_once() -> tuple[bool, bool]:
        nonlocal frame_index
        path = frames[frame_index % len(frames)] if frames else None
        frame_index += 1
        pixels = load_frame(path, SIDE)
        logits = outputs(main_model, pixels[None, ...])[0].reshape(-1)
        if logits.size != 2 or not np.all(np.isfinite(logits)):
            raise RuntimeError("NSFW classifier returned an invalid tensor")
        probabilities = np.exp(logits - np.max(logits))
        probabilities /= np.sum(probabilities)
        blocked = bool(probabilities[0] >= 0.5)

        grade = outputs(grade_model, load_frame(path, GRADE_SIDE))[0].reshape(-1)
        if grade.size != 5 or not np.all(np.isfinite(grade)):
            raise RuntimeError("NSFW grader returned an invalid tensor")

        general_pixels = load_frame(path, GRADE_SIDE)
        embedding = vector_output(outputs(vision_model, general_pixels[None, ...]), 768)
        if not np.all(np.isfinite(embedding)):
            raise RuntimeError("general vision model returned an invalid embedding")
        return blocked, bool(np.linalg.norm(embedding) > 0.99)

    for _ in range(max(0, args.warmup)):
        run_once()

    started = time.monotonic()
    passes = 0
    blocked_count = 0
    while time.monotonic() - started < duration:
        blocked, general_ready = run_once()
        if not general_ready:
            raise RuntimeError("general vision model produced a zero vector")
        blocked_count += blocked
        passes += 1

    elapsed = time.monotonic() - started
    print(
        f"ok duration={elapsed:.1f}s passes={passes} blocked={blocked_count} "
        f"throughput={passes / max(elapsed, 1e-9):.2f}/s"
    )


if __name__ == "__main__":
    main()
