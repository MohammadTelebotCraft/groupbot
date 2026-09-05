#!/usr/bin/env python3
"""Bounded Persian voice sampling for the Rust group moderator."""

import argparse
import io
import json
import os
import subprocess
import sys
import wave
from concurrent.futures import ThreadPoolExecutor
from typing import Any

import speech_recognition as sr


SAMPLE_RATE = 16000
SAMPLE_WIDTH = 2
WHOLE_DECODE_SECONDS = 180.0
_WHISPER_MODEL: Any = None


def sample_windows(
    duration: float,
    window_seconds: float,
    full_scan_seconds: float,
    max_windows: int,
) -> list[tuple[float, float]]:
    if duration <= 0 or window_seconds <= 0 or max_windows <= 0:
        return []

    width = min(duration, window_seconds)
    if duration <= full_scan_seconds:
        step = max(1.0, width - 1.0)
        starts: list[float] = []
        start = 0.0
        while start < duration and len(starts) < max_windows:
            starts.append(start)
            if start + width >= duration:
                break
            start += step
        last = max(0.0, duration - width)
        if starts and starts[-1] < last and len(starts) < max_windows:
            starts.append(last)
        return [(start, min(width, duration - start)) for start in starts]

    last = max(0.0, duration - width)
    if max_windows == 1:
        starts = [last / 2.0]
    else:
        starts = [last * index / (max_windows - 1) for index in range(max_windows)]
    return [(start, width) for start in starts]


def extract_wav(path: str, start: float, duration: float) -> bytes:
    command = [
        "ffmpeg",
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-ss",
        f"{max(0.0, start):.3f}",
        "-i",
        path,
        "-t",
        f"{duration:.3f}",
        "-vn",
        "-sn",
        "-dn",
        "-ac",
        "1",
        "-ar",
        "16000",
        "-sample_fmt",
        "s16",
        "-f",
        "wav",
        "pipe:1",
    ]
    result = subprocess.run(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
        timeout=40,
    )
    return result.stdout


def extract_pcm(path: str) -> bytes:
    command = [
        "ffmpeg",
        "-nostdin",
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        path,
        "-vn",
        "-sn",
        "-dn",
        "-ac",
        "1",
        "-ar",
        str(SAMPLE_RATE),
        "-sample_fmt",
        "s16",
        "-f",
        "s16le",
        "pipe:1",
    ]
    result = subprocess.run(
        command,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        check=True,
        timeout=90,
    )
    return result.stdout


def pcm_window(pcm: bytes, start: float, duration: float) -> bytes:
    first = max(0, round(start * SAMPLE_RATE)) * SAMPLE_WIDTH
    last = max(first, round((start + duration) * SAMPLE_RATE) * SAMPLE_WIDTH)
    return pcm[first:last]


def whisper_model() -> Any:
    global _WHISPER_MODEL
    if _WHISPER_MODEL is None:
        from faster_whisper import WhisperModel

        device = os.environ.get("VOICE_WHISPER_DEVICE", "cpu")
        compute_type = os.environ.get(
            "VOICE_WHISPER_COMPUTE_TYPE", "float16" if device == "cuda" else "int8"
        )
        _WHISPER_MODEL = WhisperModel(
            os.environ.get("VOICE_WHISPER_MODEL", "large-v3"),
            device=device,
            compute_type=compute_type,
        )
    return _WHISPER_MODEL


def audio_samples(path: str, start: float, duration: float, pcm: bytes | None) -> Any:
    try:
        import numpy as np
    except ImportError as error:
        raise RuntimeError("VOICE_BACKEND=faster-whisper requires numpy") from error

    if pcm is not None:
        raw = pcm_window(pcm, start, duration)
    else:
        wav = extract_wav(path, start, duration)
        with wave.open(io.BytesIO(wav), "rb") as source:
            if source.getnchannels() != 1 or source.getframerate() != SAMPLE_RATE:
                raise RuntimeError("FFmpeg returned unexpected voice sample format")
            raw = source.readframes(source.getnframes())
    return np.frombuffer(raw, dtype=np.int16).astype(np.float32) / 32768.0


def recognize_whisper_window(
    path: str,
    start: float,
    duration: float,
    pcm: bytes | None,
) -> dict[str, Any] | None:
    segments, _ = whisper_model().transcribe(
        audio_samples(path, start, duration, pcm),
        language="fa",
        beam_size=int(os.environ.get("VOICE_WHISPER_BEAM_SIZE", "5")),
        condition_on_previous_text=False,
        vad_filter=False,
        temperature=0.0,
    )
    text = " ".join(segment.text.strip() for segment in segments if segment.text.strip()).strip()
    return {"text": text, "confidence": None} if text else None


def recognize_window(
    path: str,
    start: float,
    duration: float,
    key: str | None,
    pcm: bytes | None,
) -> dict[str, Any] | None:
    if os.environ.get("VOICE_BACKEND", "google").lower() in {"whisper", "faster-whisper"}:
        return recognize_whisper_window(path, start, duration, pcm)

    recognizer = sr.Recognizer()
    recognizer.operation_timeout = 20
    try:
        if pcm is None:
            wav = extract_wav(path, start, duration)
            audio = sr.AudioData.from_file(io.BytesIO(wav))
        else:
            audio = sr.AudioData(pcm_window(pcm, start, duration), SAMPLE_RATE, SAMPLE_WIDTH)
        result = recognizer.recognize_google(
            audio,
            key=key,
            language="fa-IR",
            pfilter=0,
            show_all=True,
        )
    except (sr.UnknownValueError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
        return None
    except sr.RequestError:
        raise

    if not isinstance(result, dict):
        return None
    alternatives = result.get("alternative") or []
    if not alternatives:
        return None
    best = max(alternatives, key=lambda item: item.get("confidence", 0.0))
    text = str(best.get("transcript", "")).strip()
    if not text:
        return None
    confidence = best.get("confidence")
    return {
        "text": text,
        "confidence": float(confidence) if confidence is not None else None,
    }


def recognize_file(
    path: str,
    duration: float,
    window_seconds: float,
    full_scan_seconds: float,
    max_windows: int,
) -> dict[str, Any]:
    key = os.environ.get("GOOGLE_SPEECH_RECOGNITION_KEY") or None
    windows = sample_windows(duration, window_seconds, full_scan_seconds, max_windows)
    pcm: bytes | None = None
    # Short Telegram voice notes are the common case. Decode them once and slice the exact
    # same windows in memory; long recordings keep the old per-window seek so a six-hour file
    # is never expanded into hundreds of megabytes just to inspect six samples.
    if windows and duration <= WHOLE_DECODE_SECONDS:
        try:
            pcm = extract_pcm(path)
        except (OSError, subprocess.CalledProcessError, subprocess.TimeoutExpired):
            pcm = None

    # The windows are independent. Keeping the same six windows and the same Google request
    # parameters, but running them concurrently, removes five serial network waits and makes one
    # bounded voice job finish much sooner. Each worker owns its Recognizer because the library
    # object is not a shared-state requirement and this avoids a cross-thread lock.
    transcripts: list[dict[str, Any]] = []
    backend = os.environ.get("VOICE_BACKEND", "google").lower()
    window_workers = 6 if backend == "google" else int(
        os.environ.get("VOICE_WHISPER_WINDOW_WORKERS", "1")
    )
    with ThreadPoolExecutor(max_workers=min(len(windows), max(1, window_workers)) or 1) as workers:
        futures = [
            workers.submit(recognize_window, path, start, width, key, pcm)
            for start, width in windows
        ]
        for future in futures:
            result = future.result()
            if result is not None:
                transcripts.append(result)

    return {
        "transcripts": transcripts,
        "usable_windows": len(transcripts),
        "sampled_seconds": sum(width for _, width in windows),
    }


def worker_main() -> int:
    for line in sys.stdin:
        if not line.strip():
            continue
        try:
            request = json.loads(line)
            report = recognize_file(
                str(request["input"]),
                float(request["duration"]),
                float(request["window_seconds"]),
                float(request["full_scan_seconds"]),
                int(request["max_windows"]),
            )
        except Exception as error:  # one malformed job must not kill the persistent worker
            report = {"error": str(error)}
        print(json.dumps(report, ensure_ascii=False), flush=True)
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--worker", action="store_true")
    parser.add_argument("--input")
    parser.add_argument("--duration", type=float)
    parser.add_argument("--window-seconds", type=float)
    parser.add_argument("--full-scan-seconds", type=float)
    parser.add_argument("--max-windows", type=int)
    args = parser.parse_args()
    if args.worker:
        return worker_main()
    required = (args.input, args.duration, args.window_seconds, args.full_scan_seconds, args.max_windows)
    if any(value is None for value in required):
        parser.error("--input, --duration, --window-seconds, --full-scan-seconds, and --max-windows are required")
    print(json.dumps(recognize_file(*required), ensure_ascii=False))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
