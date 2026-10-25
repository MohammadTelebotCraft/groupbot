#!/usr/bin/env python3
"""Lottie stickers traced from photographs, in four drawing styles: the drawn view the built-in face models learn
from (`facelock_people.py rendered`, `facelock_drawn_vision.py`) and are tested on. An animated sticker is Lottie —
vector shapes, no pixels — and real packs of a real person are drawn the same way from photographs: the
head-and-shoulders cut out, flattened to a few colours, each colour's regions traced to polygons (one layer per
colour, largest underneath) on a white outline, a slow zoom so it animates, gzipped under Telegram's 64 KB.

    ~/.cache/nsfw-train/venv-gpu/bin/python tools/lottie_trace.py train <style>   # every photograph's largest face
    ~/.cache/nsfw-train/venv-gpu/bin/python tools/lottie_trace.py test <style>    # the held-out test set

`train` writes ~/.cache/faces/train_tgs_<style>/<sha16>.tgs for every photograph `facelock_people.py embed` read
(sha1 of "owner|path"); `test` writes ~/.cache/faces/stickers_tgs_<style>/: Khamenei from 40 of his held-out
photographs (`khamenei__*`), five held-out photographs of each other built-in person (`lookalike__*`) and 30
held-out faces of nobody in particular (`everyday__*`). flat10's directories carry no suffix (`train_tgs`,
`stickers_tgs`). Then `facelock::tests::render_lottie_stickers` draws them with the bot's own renderer
(LOTTIE_DIR, LOTTIE_OUT; LOTTIE_RAW for the frames as the general model reads them, unblurred).

flat10: ten flat colours, median-smoothed, fine shapes (the first style).  cartoon6: edge-preserving smoothing,
six colours, coarse shapes.  poster8: eight colours, unsmoothed, medium shapes.  lines5: smoothed, five colours,
every shape stroked in dark ink — never trained on, so the test of a style nobody fitted to.
"""

from __future__ import annotations

import gzip
import hashlib
import json
import os
import pickle
import sys

import numpy as np

STYLES = {
    "flat10": dict(k=10, bilateral=False, median=5, eps=1.0, ring=17, stroke=0),
    "cartoon6": dict(k=6, bilateral=True, median=5, eps=2.5, ring=27, stroke=0),
    "poster8": dict(k=8, bilateral=False, median=0, eps=1.5, ring=21, stroke=0),
    "lines5": dict(k=5, bilateral=True, median=7, eps=2.0, ring=21, stroke=3),
}
LIMIT = 64 * 1024
# A photograph of the young Khamenei that Commons files under nobody in particular (black beard, 1980s): the
# general model's drawing direction found it; it is written as `young_khamenei__*`, counted on its own.
YOUNG_KHAMENEI = "/commons/ir_religion/05847881b7890017.jpg"


def cutout(path, box):
    """The head and shoulders around `box` (x, y, w, h in the photograph at 1280 px), 512 px, and the sticker's
    silhouette: a head ellipse over a shoulders ellipse."""
    from PIL import Image, ImageDraw

    im = Image.open(path).convert("RGB")
    if max(im.size) > 1280:
        s = 1280 / max(im.size)
        im = im.resize((round(im.width * s), round(im.height * s)), Image.Resampling.BICUBIC)
    x, y, w, h = box
    cx, cy, size = x + w / 2, y + h * 0.9, 3.0 * h
    crop = im.crop((int(cx - size / 2), int(cy - size / 2), int(cx + size / 2), int(cy + size / 2)))
    crop = crop.resize((512, 512), Image.Resampling.BICUBIC)
    mask = Image.new("L", (512, 512), 0)
    draw = ImageDraw.Draw(mask)
    draw.ellipse((150, 40, 362, 330), fill=255)
    draw.ellipse((40, 300, 472, 760), fill=255)
    return np.asarray(crop), np.asarray(mask) > 127


def _transform():
    return {"o": {"a": 0, "k": 100}, "r": {"a": 0, "k": 0}, "p": {"a": 0, "k": [256, 256, 0]},
            "a": {"a": 0, "k": [256, 256, 0]},
            "s": {"a": 1, "k": [{"t": 0, "s": [92, 92, 100], "i": {"x": [0.5], "y": [1]}, "o": {"x": [0.5], "y": [0]}},
                                {"t": 59, "s": [100, 100, 100]}]}}


def lottie(rgb, inside, style, eps=None):
    import cv2

    st = STYLES[style]
    eps = st["eps"] if eps is None else eps
    cv2.setRNGSeed(0)
    src = cv2.bilateralFilter(np.ascontiguousarray(rgb), 9, 75, 75) if st["bilateral"] else rgb
    _, labels, centres = cv2.kmeans(src[inside].astype(np.float32), st["k"], None,
                                    (cv2.TERM_CRITERIA_EPS + cv2.TERM_CRITERIA_MAX_ITER, 20, 1.0), 3,
                                    cv2.KMEANS_PP_CENTERS)
    lab = np.full(inside.shape, -1, np.int32)
    lab[inside] = labels.ravel()
    if st["median"]:
        lab = cv2.medianBlur((lab + 1).astype(np.uint8), st["median"]).astype(np.int32) - 1
    ring = cv2.dilate(inside.astype(np.uint8), np.ones((st["ring"], st["ring"]), np.uint8))
    plan = [("outline", ring, (1.0, 1.0, 1.0))] + [
        (f"c{c}", (lab == c).astype(np.uint8), tuple(float(v) / 255 for v in centres[c]))
        for c in sorted(range(st["k"]), key=lambda c: -(lab == c).sum())]
    layers = []
    for name, region, colour in plan:
        contours, _ = cv2.findContours(region, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
        shapes = []
        for contour in contours:
            if cv2.contourArea(contour) < 25:
                continue
            poly = cv2.approxPolyDP(contour, eps, True).reshape(-1, 2)
            if len(poly) < 3:
                continue
            v = [[int(px), int(py)] for px, py in poly]
            z = [[0, 0]] * len(v)
            shapes.append({"ty": "sh", "ks": {"a": 0, "k": {"i": z, "o": z, "v": v, "c": True}}})
        if not shapes:
            continue
        shapes.append({"ty": "fl", "c": {"a": 0, "k": [colour[0], colour[1], colour[2], 1]}, "o": {"a": 0, "k": 100},
                       "r": 1})
        if st["stroke"] and name != "outline":
            shapes.append({"ty": "st", "c": {"a": 0, "k": [0.12, 0.1, 0.1, 1]}, "o": {"a": 0, "k": 100},
                           "w": {"a": 0, "k": st["stroke"]}, "lc": 2, "lj": 2})
        tr = {"ty": "tr", "p": {"a": 0, "k": [0, 0]}, "a": {"a": 0, "k": [0, 0]}, "s": {"a": 0, "k": [100, 100]},
              "r": {"a": 0, "k": 0}, "o": {"a": 0, "k": 100}}
        layers.append({"ddd": 0, "ind": 0, "ty": 4, "nm": name, "sr": 1, "ao": 0, "ip": 0, "op": 60, "st": 0, "bm": 0,
                       "ks": _transform(), "shapes": [{"ty": "gr", "it": shapes + [tr]}]})
    layers.reverse()
    for at, layer in enumerate(layers):
        layer["ind"] = at + 1
    return {"v": "5.5.2", "fr": 30, "ip": 0, "op": 60, "w": 512, "h": 512, "nm": style, "ddd": 0, "assets": [],
            "layers": layers, "tgs": 1}


def _one(job):
    style, path, box, target = job
    if os.path.exists(target):
        return target
    rgb, inside = cutout(path, box)
    # Coarser shapes until it fits, as a sticker maker simplifies a busy picture.
    for eps in (None, 2.0, 3.0, 4.0):
        blob = gzip.compress(json.dumps(lottie(rgb, inside, style, eps), separators=(",", ":")).encode())
        if len(blob) <= LIMIT:
            with open(target, "wb") as out:
                out.write(blob)
            return target
    return None


def _folder(kind: str, style: str) -> str:
    base = {"train": "train_tgs", "test": "stickers_tgs"}[kind]
    return os.path.expanduser(f"~/.cache/faces/{base}" + ("" if style == "flat10" else f"_{style}"))


def main() -> None:
    from multiprocessing import Pool

    kind, style = sys.argv[1], sys.argv[2]
    if style not in STYLES:
        raise SystemExit(f"style is one of {', '.join(STYLES)}")
    sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
    import facelock_people as F

    data = pickle.load(open(F.EMBEDDED, "rb"))
    out = _folder(kind, style)
    os.makedirs(out, exist_ok=True)
    jobs = []
    if kind == "train":
        for key, o in data.items():
            if o is not None and len(o["boxes"]):
                stem = hashlib.sha1(f"{key[0]}|{key[1]}".encode()).hexdigest()[:16]
                jobs.append((style, key[1], o["boxes"][0], os.path.join(out, stem + ".tgs")))
    else:
        found, everyday = F.dataset()

        def box_of(person, path, views):
            o = data[(person, path)]
            return o["boxes"][int(np.argmax(o["photo"] @ views["photo"]))]

        rows = [r for r in found["ali khamenei"][1] if F._split(r[0], 5) >= 3][:40]
        for n, (path, views, _) in enumerate(rows):
            jobs.append((style, path, box_of("ali khamenei", path, views), os.path.join(out, f"khamenei__{n:03d}.tgs")))
        n = 0
        for other, (_, rows) in sorted(found.items()):
            if other == "ali khamenei":
                continue
            for path, views, _ in [r for r in rows if F._split(r[0], 5) >= 3][:5]:
                jobs.append((style, path, box_of(other, path, views), os.path.join(out, f"lookalike__{n:03d}.tgs")))
                n += 1
        n = 0
        for path, o in everyday:
            if n >= 30 or "/commons/" not in path or F._split(path, 5) != 4 or not len(o["boxes"]):
                continue
            if min(o["boxes"][0][2], o["boxes"][0][3]) < 60:
                continue
            label = "young_khamenei" if path.endswith(YOUNG_KHAMENEI) else "everyday"
            jobs.append((style, path, o["boxes"][0], os.path.join(out, f"{label}__{n:03d}.tgs")))
            n += 1
    with Pool(8) as pool:
        done = pool.map(_one, jobs, chunksize=8)
    print(f"{kind} {style}: {sum(d is not None for d in done)} of {len(jobs)} stickers in {out}")


if __name__ == "__main__":
    main()
