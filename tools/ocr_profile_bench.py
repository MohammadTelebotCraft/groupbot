#!/usr/bin/env python3
"""Reproducible comparison of the pinned Tesseract OCR segmentation profiles.

The benchmark deliberately separates labelled synthetic renderings from unlabelled, cached
public photographs.  Character error rate and required-token recall are reported only where
the expected text was specified by the fixture author.  Public-image results report OCR output
rate and cross-profile disagreement, not accuracy.

By default transcripts are never written to the report; only length and SHA-256 are retained.
Use ``--include-text`` only with non-private fixtures.
"""

from __future__ import annotations

import argparse
import collections
import hashlib
import json
import math
import os
import pathlib
import shutil
import statistics
import subprocess
import tempfile
import time
import unicodedata
from dataclasses import dataclass
from typing import Any, Sequence

from PIL import Image, ImageDraw, ImageEnhance, ImageFilter, ImageFont, ImageOps


ROOT = pathlib.Path(__file__).resolve().parent.parent
DEFAULT_ROOT = ROOT / "target" / "ocr-profile-bench"
FONT_REGULAR = pathlib.Path("/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf")
FONT_BOLD = pathlib.Path("/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf")
PROFILE_ARGUMENTS = {
    "block6": (6, "none"),
    "auto3": (3, "none"),
    "sparse11": (11, "none"),
    "block6_autocontrast": (6, "autocontrast"),
    "sparse11_upscale2": (11, "upscale2"),
    # Orientation probes are reported separately; they are not automatic routing candidates.
    "sparse11_rotate90": (11, "rotate90"),
    "sparse11_rotate270": (11, "rotate270"),
}


def write_jsonl(path: pathlib.Path, rows: Sequence[dict[str, Any]]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    with path.open("w", encoding="utf-8", newline="\n") as handle:
        for row in rows:
            handle.write(json.dumps(row, ensure_ascii=False, sort_keys=True) + "\n")


def read_jsonl(path: pathlib.Path) -> list[dict[str, Any]]:
    rows = []
    for number, line in enumerate(path.read_text(encoding="utf-8").splitlines(), 1):
        if not line.strip():
            continue
        value = json.loads(line)
        if not isinstance(value, dict):
            raise ValueError(f"{path}:{number}: row must be an object")
        rows.append(value)
    return rows


def _font(size: int, bold: bool = False) -> ImageFont.FreeTypeFont:
    path = FONT_BOLD if bold else FONT_REGULAR
    if not path.is_file():
        raise RuntimeError(f"required fixture font is missing: {path}")
    return ImageFont.truetype(str(path), size)


def _background(size: tuple[int, int], style: str) -> Image.Image:
    width, height = size
    if style == "plain":
        return Image.new("RGB", size, "white")
    image = Image.new("RGB", size)
    pixels = image.load()
    for y in range(height):
        for x in range(width):
            base = int(35 + 150 * x / max(1, width - 1))
            noise = ((x * 17 + y * 31) % 29) - 14
            pixels[x, y] = (
                max(0, min(255, base + noise)),
                max(0, min(255, 95 + y * 100 // max(1, height) - noise)),
                max(0, min(255, 165 - noise)),
            )
    return image.filter(ImageFilter.GaussianBlur(1.2))


def _draw_lines(
    path: pathlib.Path,
    lines: Sequence[tuple[str, tuple[int, int], int, str, bool]],
    *,
    size: tuple[int, int] = (900, 600),
    style: str = "plain",
    rotate: int = 0,
    jpeg_quality: int | None = None,
) -> None:
    image = _background(size, style)
    draw = ImageDraw.Draw(image)
    for text, xy, font_size, language, bold in lines:
        fill = (12, 12, 12) if style == "plain" else (250, 250, 245)
        direction = "rtl" if language == "fa" else "ltr"
        draw.text(
            xy,
            text,
            font=_font(font_size, bold),
            fill=fill,
            direction=direction,
            language=language,
            stroke_width=1 if style != "plain" else 0,
            stroke_fill=(0, 0, 0),
        )
    if rotate:
        image = image.rotate(rotate, expand=True, fillcolor="white")
    path.parent.mkdir(parents=True, exist_ok=True)
    if jpeg_quality is None:
        image.save(path, "PNG")
    else:
        image.save(path, "JPEG", quality=jpeg_quality, optimize=True)


def generate_fixtures(output_root: pathlib.Path) -> pathlib.Path:
    fixtures = output_root / "fixtures"
    rows: list[dict[str, Any]] = []

    def add(
        sample_id: str,
        lines,
        expected: str,
        tokens: Sequence[str],
        *,
        layout: str,
        language: str,
        **render,
    ):
        suffix = ".jpg" if render.get("jpeg_quality") is not None else ".png"
        path = fixtures / f"{sample_id}{suffix}"
        _draw_lines(path, lines, **render)
        rows.append(
            {
                "id": sample_id,
                "path": str(path),
                "source_type": "synthetic_rendered",
                "layout": layout,
                "language": language,
                "expected_text": expected,
                "required_tokens": list(tokens),
            }
        )

    add(
        "en-clean-block",
        [
            ("SALE 50% OFF", (90, 120), 64, "en", True),
            ("ORDER NOW @shop_channel", (90, 230), 46, "en", False),
            ("https://shop.example.com", (90, 320), 42, "en", False),
        ],
        "SALE 50% OFF\nORDER NOW @shop_channel\nhttps://shop.example.com",
        ["sale", "@shop_channel", "shop.example.com"],
        layout="dense-block",
        language="en",
    )
    add(
        "fa-clean-block",
        [
            ("فروش ویژه", (520, 100), 68, "fa", True),
            ("ثبت سفارش تماس", (500, 220), 50, "fa", False),
            ("۰۹۱۲۱۲۳۴۵۶۷", (500, 330), 52, "fa", False),
        ],
        "فروش ویژه\nثبت سفارش تماس\n۰۹۱۲۱۲۳۴۵۶۷",
        ["فروش", "سفارش", "۰۹۱۲۱۲۳۴۵۶۷"],
        layout="dense-block",
        language="fa",
    )
    add(
        "mixed-handle",
        [
            ("فروش iPhone", (540, 110), 64, "fa", True),
            ("DM @iphone_store", (110, 245), 52, "en", False),
            ("قیمت ۱۰۰٬۰۰۰ تومان", (450, 350), 44, "fa", False),
        ],
        "فروش iPhone\nDM @iphone_store\nقیمت ۱۰۰٬۰۰۰ تومان",
        ["فروش", "@iphone_store", "تومان"],
        layout="mixed-block",
        language="mixed",
    )
    add(
        "sparse-corners",
        [
            ("PROMO", (35, 40), 42, "en", True),
            ("t.me/deals", (570, 485), 36, "en", False),
            ("09121234567", (45, 500), 32, "en", False),
        ],
        "PROMO\nt.me/deals\n09121234567",
        ["promo", "t.me/deals", "09121234567"],
        layout="sparse-scene",
        language="en",
        style="scene",
    )
    add(
        "telegram-ui",
        [
            ("News Center", (70, 50), 42, "en", True),
            ("45.6K subscribers", (70, 110), 27, "en", False),
            ("Pinned Message", (70, 190), 27, "en", False),
            ("Read more at example.com", (70, 300), 29, "en", False),
            ("Message", (70, 510), 28, "en", False),
        ],
        "News Center\n45.6K subscribers\nPinned Message\nRead more at example.com\nMessage",
        ["subscribers", "pinned", "example.com"],
        layout="screenshot",
        language="en",
    )
    add(
        "small-compressed",
        [
            ("فروش ویژه @myshop", (370, 175), 24, "fa", False),
            ("contact 09121234567", (180, 250), 22, "en", False),
        ],
        "فروش ویژه @myshop\ncontact 09121234567",
        ["فروش", "@myshop", "09121234567"],
        layout="small-compressed",
        language="mixed",
        size=(640, 400),
        style="scene",
        jpeg_quality=38,
    )
    add(
        "rotated-90",
        [
            ("ORDER @rotate_shop", (80, 180), 46, "en", True),
            ("t.me/rotate", (80, 270), 42, "en", False),
        ],
        "ORDER @rotate_shop\nt.me/rotate",
        ["@rotate_shop", "t.me/rotate"],
        layout="rotated-90",
        language="en",
        size=(800, 500),
        rotate=90,
    )
    add(
        "legit-receipt",
        [
            ("رسید خرید", (570, 80), 54, "fa", True),
            ("شماره 1234567", (510, 190), 44, "fa", False),
            ("مبلغ 100,000 تومان", (430, 300), 44, "fa", False),
        ],
        "رسید خرید\nشماره 1234567\nمبلغ 100,000 تومان",
        ["رسید", "1234567", "تومان"],
        layout="legitimate-document",
        language="mixed",
    )

    # Unlabelled public photographs are a false-positive-surface diagnostic only.  Their lack of
    # expected text is explicit so they can never enter CER/token accuracy denominators.
    public_root = ROOT / "target" / "media-eval-public" / "nsfw"
    for group in ("people", "faces", "nsfw"):
        for path in sorted((public_root / group).glob("*.jpg"))[:16]:
            rows.append(
                {
                    "id": f"public-{group}-{path.stem}",
                    "path": str(path),
                    "source_type": "public_unlabelled",
                    "layout": group,
                    "language": "unknown",
                    "expected_text": None,
                    "required_tokens": [],
                }
            )
    manifest = output_root / "manifest.jsonl"
    write_jsonl(manifest, rows)
    return manifest


def stage_runtime(output_root: pathlib.Path) -> tuple[pathlib.Path, pathlib.Path]:
    source = ROOT / "assets" / "tesseract"
    runtime = output_root / "runtime"
    tessdata = runtime / "tessdata"
    runtime.mkdir(parents=True, exist_ok=True)
    tessdata.mkdir(parents=True, exist_ok=True)
    files = {
        "tesseract": "tesseract",
        "libarchive.so.13": "libarchive.so.13.7.2",
        "libgif.so.7": "libgif.so.7.2.0",
        "libicudata.so.74": "libicudata.so.74.2",
        "libicuuc.so.74": "libicuuc.so.74.2",
        "liblept.so.5": "liblept.so.5.0.4",
        "libtesseract.so.5": "libtesseract.so.5.0.3",
        "libxml2.so.2": "libxml2.so.2.9.14",
    }
    for target, name in files.items():
        shutil.copy2(source / name, runtime / target)
    for language in ("fas", "eng"):
        shutil.copy2(source / f"{language}.traineddata", tessdata / f"{language}.traineddata")
    executable = runtime / "tesseract"
    executable.chmod(0o755)
    return executable, tessdata


def bounded(image: Image.Image) -> Image.Image:
    image = image.convert("RGB")
    longest = max(image.size)
    if longest <= 1000:
        return image
    scale = 1000 / longest
    return image.resize(
        (max(1, round(image.width * scale)), max(1, round(image.height * scale))),
        Image.Resampling.BILINEAR,
    )


def preprocess(image: Image.Image, operation: str) -> Image.Image:
    image = bounded(image)
    if operation == "none":
        return image
    if operation == "autocontrast":
        return ImageOps.autocontrast(image.convert("L"), cutoff=1).convert("RGB")
    if operation == "upscale2":
        scale = min(2.0, 1000 / max(image.size))
        if scale <= 1:
            return image
        return image.resize(
            (max(1, round(image.width * scale)), max(1, round(image.height * scale))),
            Image.Resampling.LANCZOS,
        )
    if operation == "rotate90":
        return image.rotate(90, expand=True, fillcolor="white")
    if operation == "rotate270":
        return image.rotate(270, expand=True, fillcolor="white")
    raise ValueError(operation)


@dataclass(frozen=True)
class OcrResult:
    text: str
    latency_ms: float
    timed_out: bool


def recognize(
    path: pathlib.Path,
    profile: str,
    executable: pathlib.Path,
    tessdata: pathlib.Path,
    timeout: float,
) -> OcrResult:
    psm, operation = PROFILE_ARGUMENTS[profile]
    with Image.open(path) as source:
        image = preprocess(source, operation)
    with tempfile.TemporaryDirectory(prefix="groupbot-ocr-profile-") as directory:
        temporary = pathlib.Path(directory)
        input_path = temporary / "input.ppm"
        output_base = temporary / "output"
        image.save(input_path, "PPM")
        env = os.environ.copy()
        env.update(
            LD_LIBRARY_PATH=str(executable.parent),
            TESSDATA_PREFIX=str(tessdata),
            OMP_THREAD_LIMIT="1",
            OMP_NUM_THREADS="1",
            LC_ALL="C.UTF-8",
        )
        command = [
            str(executable),
            str(input_path),
            str(output_base),
            "-l",
            "fas+eng",
            "--psm",
            str(psm),
            "--dpi",
            "150",
            "--tessdata-dir",
            str(tessdata),
        ]
        started = time.perf_counter()
        try:
            process = subprocess.run(command, capture_output=True, timeout=timeout, env=env)
        except subprocess.TimeoutExpired:
            return OcrResult("", (time.perf_counter() - started) * 1000, True)
        latency = (time.perf_counter() - started) * 1000
        if process.returncode != 0:
            stderr_hash = hashlib.sha256(process.stderr).hexdigest()
            raise RuntimeError(
                f"Tesseract exited {process.returncode}; stderr_sha256={stderr_hash}"
            )
        text_path = output_base.with_suffix(".txt")
        text = text_path.read_text(encoding="utf-8").strip() if text_path.is_file() else ""
        return OcrResult(text[:8192], latency, False)


def normalized(text: str) -> str:
    text = unicodedata.normalize("NFKC", text).casefold()
    text = text.replace("ي", "ی").replace("ك", "ک")
    return " ".join(text.split())


def compact(text: str) -> str:
    return "".join(character for character in normalized(text) if character.isalnum() or character in "@./_+")


def edit_distance(left: str, right: str) -> int:
    if len(left) < len(right):
        left, right = right, left
    previous = list(range(len(right) + 1))
    for left_at, left_char in enumerate(left, 1):
        current = [left_at]
        for right_at, right_char in enumerate(right, 1):
            current.append(
                min(
                    current[-1] + 1,
                    previous[right_at] + 1,
                    previous[right_at - 1] + (left_char != right_char),
                )
            )
        previous = current
    return previous[-1]


def percentile(values: Sequence[float], percent: float) -> float | None:
    if not values:
        return None
    values = sorted(values)
    rank = (len(values) - 1) * percent / 100
    low, high = math.floor(rank), math.ceil(rank)
    return values[low] if low == high else values[low] + (values[high] - values[low]) * (rank - low)


def summarize(profile: str, rows: Sequence[dict[str, Any]]) -> dict[str, Any]:
    synthetic = [row for row in rows if row["source_type"] == "synthetic_rendered"]
    public = [row for row in rows if row["source_type"] == "public_unlabelled"]
    latencies = [row["latency_ms"] for row in rows]
    expected_characters = sum(len(normalized(row["expected_text"])) for row in synthetic)
    edits = sum(
        edit_distance(normalized(row["expected_text"]), normalized(row["transcript_for_metrics"]))
        for row in synthetic
    )
    tokens = [
        (token, row["transcript_for_metrics"])
        for row in synthetic
        for token in row["required_tokens"]
    ]
    synthetic_cases = []
    for row in synthetic:
        expected = normalized(row["expected_text"])
        found = normalized(row["transcript_for_metrics"])
        required = row["required_tokens"]
        synthetic_cases.append(
            {
                "id": row["id"],
                "layout": row["layout"],
                "language": row["language"],
                "character_error_rate": edit_distance(expected, found) / max(1, len(expected)),
                "required_token_recall": (
                    sum(compact(token) in compact(found) for token in required) / len(required)
                    if required
                    else None
                ),
                "text_chars": row["text_chars"],
            }
        )
    public_by_group = {}
    for group in sorted({row["layout"] for row in public}):
        group_rows = [row for row in public if row["layout"] == group]
        public_by_group[group] = {
            "samples": len(group_rows),
            "nonempty_rate": sum(bool(row["text_chars"]) for row in group_rows) / len(group_rows),
            "mean_text_chars": statistics.fmean(row["text_chars"] for row in group_rows),
        }
    return {
        "profile": profile,
        "samples": len(rows),
        "synthetic_samples": len(synthetic),
        "public_unlabelled_samples": len(public),
        "synthetic_character_error_rate": edits / expected_characters if expected_characters else None,
        "synthetic_required_token_recall": (
            sum(compact(token) in compact(text) for token, text in tokens) / len(tokens)
            if tokens
            else None
        ),
        "synthetic_nonempty_rate": sum(bool(row["text_chars"]) for row in synthetic) / len(synthetic),
        "public_nonempty_rate": sum(bool(row["text_chars"]) for row in public) / len(public) if public else None,
        "public_mean_text_chars": statistics.fmean(row["text_chars"] for row in public) if public else None,
        "synthetic_cases": synthetic_cases,
        "public_by_group": public_by_group,
        "timeout_count": sum(row["timed_out"] for row in rows),
        "latency_ms": {
            "count": len(latencies),
            "mean": statistics.fmean(latencies),
            "p50": percentile(latencies, 50),
            "p95": percentile(latencies, 95),
            "p99": percentile(latencies, 99),
            "max": max(latencies),
        },
    }


def run_benchmark(args) -> pathlib.Path:
    rows = read_jsonl(args.manifest)
    if args.source_type:
        rows = [row for row in rows if row.get("source_type") in set(args.source_type)]
    if not rows:
        raise ValueError("no manifest rows selected")
    executable, tessdata = stage_runtime(args.output_dir)
    if args.tessdata_dir is not None:
        tessdata = args.tessdata_dir.resolve()
    model_files = {}
    for language in ("fas", "eng"):
        model = tessdata / f"{language}.traineddata"
        if not model.is_file():
            raise FileNotFoundError(model)
        model_files[language] = {
            "path": str(model),
            "bytes": model.stat().st_size,
            "sha256": hashlib.sha256(model.read_bytes()).hexdigest(),
        }
    per_profile: dict[str, list[dict[str, Any]]] = {}
    summaries = []
    for profile in args.profile:
        measured = []
        for row in rows:
            result = recognize(
                pathlib.Path(row["path"]), profile, executable, tessdata, args.timeout
            )
            output = {
                "id": row["id"],
                "source_type": row["source_type"],
                "layout": row["layout"],
                "language": row["language"],
                "latency_ms": result.latency_ms,
                "timed_out": result.timed_out,
                "text_chars": len(result.text),
                "text_sha256": hashlib.sha256(result.text.encode()).hexdigest(),
                "required_tokens": row.get("required_tokens", []),
                "expected_text": row.get("expected_text"),
                "transcript_for_metrics": result.text,
            }
            if args.include_text:
                output["text"] = result.text
            measured.append(output)
        per_profile[profile] = measured
        summaries.append(summarize(profile, measured))
        persisted = [
            {key: value for key, value in row.items() if key != "transcript_for_metrics"}
            for row in measured
        ]
        write_jsonl(args.output_dir / f"results-{profile}.jsonl", persisted)

    disagreements = []
    profiles = list(per_profile)
    for left_at, left in enumerate(profiles):
        for right in profiles[left_at + 1 :]:
            left_rows = {row["id"]: row for row in per_profile[left]}
            right_rows = {row["id"]: row for row in per_profile[right]}
            different = [
                sample_id
                for sample_id in left_rows
                if left_rows[sample_id]["text_sha256"] != right_rows[sample_id]["text_sha256"]
            ]
            disagreements.append(
                {
                    "left": left,
                    "right": right,
                    "count": len(different),
                    "sample_ids": different,
                }
            )
    report = {
        "schema_version": 1,
        "tessdata": model_files,
        "evidence_boundaries": {
            "synthetic_rendered": "labelled text; valid for CER/token regression, not production accuracy",
            "public_unlabelled": "real public images without text labels; output-rate/disagreement diagnostic only",
        },
        "profiles": summaries,
        "paired_transcript_disagreements": disagreements,
        "transcripts_persisted": args.include_text,
    }
    report_path = args.output_dir / "report.json"
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return report_path


def run_rust_decisions(args) -> pathlib.Path:
    rows = read_jsonl(args.manifest)
    requests = []
    request_index: dict[str, tuple[str, dict[str, Any]]] = {}
    for profile in args.profile:
        if profile not in {"block6", "auto3", "sparse11"}:
            raise ValueError(f"Rust adapter does not expose profile {profile!r}")
        for row in rows:
            request_id = f"{profile}-{row['id']}"
            request = {
                "id": request_id,
                "media_path": str(pathlib.Path(row["path"]).resolve()),
                "pipeline": "advert",
                "ocr_profile": profile,
                "include_text": False,
            }
            requests.append(json.dumps(request, ensure_ascii=False))
            request_index[request_id] = (profile, row)
    process = subprocess.run(
        [str(args.binary), "--media-eval"],
        input="\n".join(requests) + "\n",
        capture_output=True,
        text=True,
        timeout=args.timeout,
    )
    if process.returncode != 0:
        stderr_hash = hashlib.sha256(process.stderr.encode()).hexdigest()
        raise RuntimeError(
            f"Rust media evaluator exited {process.returncode}; stderr_sha256={stderr_hash}"
        )
    outputs = [json.loads(line) for line in process.stdout.splitlines() if line.strip()]
    if len(outputs) != len(requests):
        raise RuntimeError(f"Rust adapter returned {len(outputs)}/{len(requests)} rows")

    measured: dict[str, list[dict[str, Any]]] = collections.defaultdict(list)
    for output in outputs:
        request_id = output.get("id")
        if request_id not in request_index:
            raise RuntimeError("Rust adapter returned an unknown id")
        profile, source = request_index[request_id]
        prediction = output.get("prediction") or {}
        evidence = output.get("evidence") or {}
        measured[profile].append(
            {
                "id": source["id"],
                "source_type": source["source_type"],
                "layout": source["layout"],
                "language": source["language"],
                "predicted_label": prediction.get("predicted_label"),
                "decision": prediction.get("decision"),
                "latency_ms": prediction.get("latency_ms"),
                "ocr_ms": (prediction.get("stage_ms") or {}).get("ocr"),
                "text_chars": evidence.get("text_characters"),
                "reason_codes": evidence.get("reasons", []),
                "error": prediction.get("error"),
            }
        )

    summaries = []
    for profile in args.profile:
        profile_rows = measured[profile]
        by_source = {}
        for source_type in sorted({row["source_type"] for row in profile_rows}):
            source_rows = [row for row in profile_rows if row["source_type"] == source_type]
            by_source[source_type] = {
                "samples": len(source_rows),
                "decisions": dict(sorted(collections.Counter(row["decision"] for row in source_rows).items())),
                "action_rate": sum(row["decision"] == "ACTION" for row in source_rows) / len(source_rows),
                "uncertain_rate": sum(row["decision"] == "UNCERTAIN" for row in source_rows) / len(source_rows),
                "error_rate": sum(row["decision"] == "ERROR" for row in source_rows) / len(source_rows),
            }
        ocr_times = [float(row["ocr_ms"]) for row in profile_rows if isinstance(row["ocr_ms"], (int, float))]
        summaries.append(
            {
                "profile": profile,
                "samples": len(profile_rows),
                "by_source": by_source,
                "ocr_latency_ms": {
                    "count": len(ocr_times),
                    "p50": percentile(ocr_times, 50),
                    "p95": percentile(ocr_times, 95),
                    "p99": percentile(ocr_times, 99),
                },
            }
        )
        write_jsonl(args.output_dir / f"rust-decisions-{profile}.jsonl", profile_rows)

    disagreements = []
    for left_at, left in enumerate(args.profile):
        for right in args.profile[left_at + 1 :]:
            left_rows = {row["id"]: row for row in measured[left]}
            right_rows = {row["id"]: row for row in measured[right]}
            different = [
                sample_id
                for sample_id in left_rows
                if left_rows[sample_id]["decision"] != right_rows[sample_id]["decision"]
            ]
            disagreements.append(
                {
                    "left": left,
                    "right": right,
                    "decision_disagreement_count": len(different),
                    "sample_ids": different,
                }
            )
    report = {
        "schema_version": 1,
        "policy": "unchanged Rust handlers::ocr::signals classifier",
        "evidence_boundary": (
            "synthetic decisions are regression evidence; public images are unlabelled, so action "
            "rates measure candidate surface rather than false-positive rate"
        ),
        "profiles": summaries,
        "paired_decision_disagreements": disagreements,
        "raw_ocr_persisted": False,
    }
    args.output_dir.mkdir(parents=True, exist_ok=True)
    report_path = args.output_dir / "rust-decision-report.json"
    report_path.write_text(json.dumps(report, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    return report_path


def build_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    generate = sub.add_parser("generate", help="render labelled fixtures and add cached public images")
    generate.add_argument("--output-dir", type=pathlib.Path, default=DEFAULT_ROOT)
    run = sub.add_parser("run", help="run pinned Tesseract candidate profiles")
    run.add_argument("manifest", type=pathlib.Path)
    run.add_argument("--output-dir", type=pathlib.Path, default=DEFAULT_ROOT)
    run.add_argument("--profile", action="append", choices=sorted(PROFILE_ARGUMENTS))
    run.add_argument("--timeout", type=float, default=2.0)
    run.add_argument("--include-text", action="store_true")
    run.add_argument(
        "--source-type",
        action="append",
        choices=("synthetic_rendered", "public_unlabelled"),
        help="optional evidence partition filter",
    )
    run.add_argument(
        "--tessdata-dir",
        type=pathlib.Path,
        help="candidate fas/eng directory; executable and native libraries remain pinned",
    )
    decisions = sub.add_parser(
        "rust-decisions", help="run profiles through the exact Rust OCR and policy classifier"
    )
    decisions.add_argument("manifest", type=pathlib.Path)
    decisions.add_argument("--binary", type=pathlib.Path, default=ROOT / "target" / "release" / "groupbot")
    decisions.add_argument("--output-dir", type=pathlib.Path, default=DEFAULT_ROOT)
    decisions.add_argument("--profile", action="append", choices=("block6", "auto3", "sparse11"))
    decisions.add_argument("--timeout", type=float, default=180.0)
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.command == "generate":
        print(generate_fixtures(args.output_dir))
        return 0
    if args.command == "rust-decisions":
        args.profile = args.profile or ["block6", "auto3", "sparse11"]
        print(run_rust_decisions(args))
        return 0
    args.profile = args.profile or ["block6", "auto3", "sparse11", "sparse11_upscale2"]
    print(run_benchmark(args))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
