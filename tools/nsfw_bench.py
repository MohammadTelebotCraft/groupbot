#!/usr/bin/env python3
"""The whole NSFW cascade, offline, on the corpus's held-out split or the untouched test set.

    ../bin/python tools/nsfw_bench.py --set heldout
    ../bin/python tools/nsfw_bench.py --set test --tower base224 --head /tmp/head.npz
    ../bin/python tools/nsfw_bench.py --time --onnx ~/.cache/nsfw-train/towers/base384/vision.onnx

Three policies are scored side by side, each of them the rule the bot runs or is about to:

    cascade     Marqo + grader, what a deploy without vision.onnx does
    deployed    + the arbiter (`AGREE` veto, `RECOVER` recovery), without the head
    head        + the fitted head: veto under HEAD_VETO, witness above HEAD_SURE, the
                  score itself above HEAD_DELETE — what runs now

`evaluate_nsfw.py` owns the transforms and reads every threshold out of `nsfw.rs`; this file
imports it rather than retyping any of that. What it adds is the corpus: the first evaluator
measured on photographs of people and food, and a lock that is 98% on those is still the lock
that lets a hentai sticker through, because a group's traffic is stickers, gifs, memes and
anime. `nsfw_data.py` names the groups; this prints one row per group so a regression in any
of them is visible rather than averaged away.

The first model and the grader are run once per picture and cached in
`emb/scores_<set>.npz` — a rerun with a different head or cut is instant. The embedding comes
from `emb/<set>_<tower>.npz` (`vision_embed.py`), so a tower is compared without re-running it.
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys
import time

import numpy as np
from PIL import Image

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import evaluate_nsfw as ev  # noqa: E402
import nsfw_data  # noqa: E402

HERE = pathlib.Path(__file__).resolve().parent.parent
GRADE_ABOVE = 0.10  # LIMIT_RANGE.0 / 100 in nsfw.rs
THREADS = 4


def sigmoid(z):
    return 1.0 / (1.0 + np.exp(-np.clip(z, -60, 60)))


def rows_of(which: str) -> tuple[pathlib.Path, list[tuple[str, str]]]:
    if which == "test":
        root = nsfw_data.EVAL
        rows = [(p, g) for p, g, _ in _manifest(root / "manifest.tsv")]
    else:
        root = nsfw_data.CORPUS
        rows = [(p, g) for p, g, s in _manifest(root / "manifest.tsv") if s == "heldout"]
    return root, rows


def _manifest(path: pathlib.Path):
    out = []
    for line in path.read_text(encoding="utf-8").splitlines()[1:]:
        cols = line.split("\t")
        if len(cols) >= 4:
            out.append((cols[0], cols[1], cols[3]))
    return out


def load_scores(cache: pathlib.Path) -> dict[str, np.ndarray]:
    if not cache.exists():
        return {}
    d = np.load(cache, allow_pickle=False)
    return {str(p): np.array([w, k, f, h, n, dr]) for p, w, k, f, h, n, dr in zip(
        d["path"], d["whole"], d["peak"], d["frail"], d["hard"], d["neutral"], d["drawn"])}


def save_scores(cache: pathlib.Path, scores: dict[str, np.ndarray]) -> None:
    paths = sorted(scores)
    arr = np.stack([scores[p] for p in paths]) if paths else np.zeros((0, 6))
    np.savez(cache, path=np.array(paths), whole=arr[:, 0], peak=arr[:, 1], frail=arr[:, 2],
             hard=arr[:, 3], neutral=arr[:, 4], drawn=arr[:, 5])


def score_one(image: Image.Image, main, grader) -> np.ndarray:
    """`whole, peak, frail, hard, neutral, drawn` — the two small models the way `look` and
    `grade` run them, mirrored pass included."""
    main_in, grade_in = main.get_inputs()[0].name, grader.get_inputs()[0].name
    frail = min(image.size) < ev.DETAIL_FLOOR or ev.sharpness(image) < ev.SHARPNESS_FLOOR
    views = [ev.fit(tile, ev.MAIN_RESIZE, ev.MAIN_SIDE) for tile in ev.tiles(image)]
    scores = [ev.view_score(v, main, main_in, GRADE_ABOVE) for v in views]
    whole, ends = scores[0], scores[1:]
    verdict = whole if (whole < ev.CORROBORATE or frail) else max([whole, *ends])
    peak = max(scores)
    hard = neutral = drawn = 0.0
    if peak >= GRADE_ABOVE:
        grades = []
        for tile in ev.tiles(image):
            logits = grader.run(None, {grade_in: ev.planar(ev.fit(tile, ev.GRADE_RESIZE, ev.GRADE_SIDE), 0.0, 1.0)})[0]
            grades.append(ev.softmax(logits[0] if logits.ndim == 2 else logits))
            if grades[-1][1] + grades[-1][3] >= ev.EXPLICIT_FLOOR:
                break
        hard = max(float(p[1] + p[3]) for p in grades)
        neutral = min(float(p[2]) for p in grades)
        drawn = max(float(p[0] + p[1]) for p in grades)
    return np.array([verdict, peak, float(frail), hard, neutral, drawn])


def ensure_scores(which: str, root: pathlib.Path, rows) -> dict[str, np.ndarray]:
    import onnxruntime as ort

    cache = nsfw_data.EMB / f"scores_{which}.npz"
    scores = load_scores(cache)
    todo = [p for p, _ in rows if p not in scores]
    if not todo:
        return scores
    opts = ort.SessionOptions()
    opts.intra_op_num_threads = THREADS
    main = ort.InferenceSession(str(HERE / "assets/nsfw.onnx"), opts, providers=["CPUExecutionProvider"])
    grader = ort.InferenceSession(str(HERE / "assets/nsfw_grade.onnx"), opts, providers=["CPUExecutionProvider"])
    print(f"scoring {len(todo)} pictures through the first model and the grader", flush=True)
    t = time.time()
    for at, p in enumerate(todo):
        try:
            with Image.open(root / p) as im:
                scores[p] = score_one(im.convert("RGB"), main, grader)
        except Exception as e:  # noqa: BLE001
            print(f"  unreadable {p}: {e}", flush=True)
        if at % 200 == 199:
            print(f"  {at + 1}/{len(todo)} {(at + 1) / (time.time() - t):.1f}/s", flush=True)
            save_scores(cache, scores)
    save_scores(cache, scores)
    print(f"  done in {time.time() - t:.0f}s", flush=True)
    return scores


def load_head(path: str | None) -> tuple[np.ndarray, float, float, float] | None:
    if path:
        d = np.load(path, allow_pickle=False)
        return d["w"].astype(np.float32), float(d["b"]), float(d["head_sure"]), float(d["head_delete"])
    rs = HERE / "src/handlers/nsfw_head_vectors.rs"
    if not rs.exists():
        return None
    text = rs.read_text(encoding="utf-8")
    b = float(re.search(r"pub const BIAS: f32 = (-?[\d.]+);", text).group(1))
    body = re.search(r"pub const WEIGHTS: \[f32; \d+\] = \[(.*?)\];", text, re.S).group(1)
    w = np.asarray([float(x) for x in re.findall(r"-?\d+\.\d+", body)], dtype=np.float32)
    src = (HERE / "src/handlers/nsfw.rs").read_text(encoding="utf-8")
    sure = float(re.search(r"const HEAD_SURE: f32 = ([\d.]+);", src).group(1))
    delete = float(re.search(r"const HEAD_DELETE: f32 = ([\d.]+);", src).group(1))
    return w, b, sure, delete


def load_arbiter(path: str | None, tower: str):
    """The shipped `concept_vectors.rs` unless `--arbiter` names an npz from
    `export_concepts.py --npz` — which is how a tower that is not the shipped one is compared
    on equal terms."""
    if path:
        d = np.load(path, allow_pickle=False)
        return d["EXPLICIT"].astype(np.float32), d["SAFE"].astype(np.float32)
    shipped = ev.rust_vector("EXPLICIT")
    npz = nsfw_data.EMB / f"arbiter_{tower}.npz"
    if npz.exists():
        d = np.load(npz, allow_pickle=False)
        if not np.allclose(d["EXPLICIT"], shipped, atol=1e-4):
            print(f"  arbiter: concept_vectors.rs is not {tower}'s; using {npz.name}")
            return d["EXPLICIT"].astype(np.float32), d["SAFE"].astype(np.float32)
    return shipped, ev.rust_vector("SAFE")


class Row:
    __slots__ = ("path", "group", "nsfw", "peak", "frail", "hard", "neutral", "drawn", "arbiter", "head", "veto")

    def __init__(self, path, group, s, arbiter, head, veto=None):
        self.path, self.group = path, group
        self.nsfw, self.peak, frail, self.hard, self.neutral, self.drawn = (float(x) for x in s)
        self.frail = bool(frail)
        self.arbiter, self.head, self.veto = arbiter, head, veto


def confirmed_of(r: Row) -> bool:
    return (
        r.nsfw >= ev.GRADED_CONFIDENT
        or (r.nsfw >= ev.BRIDGE_SCORE and r.hard >= ev.BRIDGE_FLOOR)
        or (r.nsfw >= ev.STRONG_BRIDGE_SCORE and r.hard >= ev.STRONG_BRIDGE_FLOOR)
    )


def cascade(r: Row, limit: float) -> bool:
    if r.nsfw * 100 < limit:
        return False
    innocent = r.neutral >= ev.NEUTRAL_FLOOR and r.nsfw < ev.CONFIDENT
    weak = r.frail and r.hard < ev.EXPLICIT_FLOOR
    return confirmed_of(r) and not innocent and not weak


def deployed(r: Row, limit: float) -> bool:
    """`allow_delete` with the arbiter: the veto, `RECOVER`, and the crops unlocked by `sure`."""
    if r.arbiter is None:
        return cascade(r, limit)
    sure = r.arbiter >= ev.RECOVER
    score = r.peak if sure else r.nsfw
    if score * 100 < limit or r.arbiter < ev.AGREE:
        return False
    innocent = r.neutral >= ev.NEUTRAL_FLOOR and r.nsfw < ev.CONFIDENT
    weak = r.frail and r.hard < ev.EXPLICIT_FLOOR
    return (confirmed_of(r) or sure) and (not innocent or sure) and (not weak or sure)


HEAD_VETO = ev.rust_const("nsfw", "HEAD_VETO")


def with_head(r: Row, limit: float, head_sure: float, head_delete: float, always: bool,
              head_veto: float = HEAD_VETO) -> bool:
    """`nsfw::allow_delete` with `Arbiter::agrees`/`sure` as they are with the head:

        agrees = head >= HEAD_SURE or (margin >= AGREE and head >= HEAD_VETO)
        sure   = margin >= RECOVER or head >= HEAD_SURE
        score  = max(score, head) if head >= HEAD_DELETE

    Asked only when the picture would have been embedded anyway (`peak >= GRADE_ABOVE`) unless
    `always` — which measures what embedding every picture would buy.
    """
    head = r.head if (always or r.peak >= GRADE_ABOVE) else None
    if head is None or r.arbiter is None:
        return deployed(r, limit)
    head_is_sure = head >= head_sure
    sure = r.arbiter >= ev.RECOVER or head_is_sure
    agrees = head_is_sure or (r.arbiter >= ev.AGREE and head >= head_veto)
    score = r.peak if sure else r.nsfw
    if head >= head_delete:
        score = max(score, head)
    if score * 100 < limit or not agrees:
        return False
    innocent = r.neutral >= ev.NEUTRAL_FLOOR and r.nsfw < ev.CONFIDENT
    weak = r.frail and r.hard < ev.EXPLICIT_FLOOR
    return (confirmed_of(r) or sure) and (not innocent or sure) and (not weak or sure)


def table(rows: list[Row], policies: dict, limit: float, positive, ambiguous, negative) -> None:
    groups = sorted(set(r.group for r in rows))
    names = list(policies)
    print(f"\nlimit {limit:.0f}    " + "".join(f"{n:>12}" for n in names))
    totals = {n: {"tp": 0, "pos": 0, "fp": 0, "neg": 0} for n in names}
    for g in groups:
        kind = "pos" if g in positive else ("amb" if g in ambiguous else ("neg" if g in negative else "?"))
        sub = [r for r in rows if r.group == g]
        cells = []
        for n, f in policies.items():
            hit = sum(f(r, limit) for r in sub)
            cells.append(f"{hit:5d}/{len(sub):<5d}")
            if kind == "pos":
                totals[n]["tp"] += hit
                totals[n]["pos"] += len(sub)
            elif kind == "neg":
                totals[n]["fp"] += hit
                totals[n]["neg"] += len(sub)
        print(f"  {g:15} {kind} " + "".join(f"{c:>12}" for c in cells))
    print("  " + "-" * (20 + 12 * len(names)))
    print(f"  {'recall':15}     " + "".join(f"{t['tp'] / max(1, t['pos']):12.3f}" for t in totals.values()))
    print(f"  {'false positives':15}     " + "".join(f"{t['fp']:5d}/{t['neg']:<5d}"[:12].rjust(12) for t in totals.values()))


def worst(rows: list[Row], policy, limit: float, negative, n: int = 12) -> None:
    bad = [r for r in rows if r.group in negative and policy(r, limit)]
    bad.sort(key=lambda r: -(r.head or 0))
    if bad:
        print(f"\n  ordinary pictures deleted ({len(bad)}):")
        for r in bad[:n]:
            arb = f" arbiter {r.arbiter:+.4f}" if r.arbiter is not None else ""
            head = f" head {r.head:.3f}" if r.head is not None else ""
            print(f"    {r.path:40} score {r.nsfw:.3f} peak {r.peak:.3f} hard {r.hard:.2f} neutral {r.neutral:.2f}"
                  f"{' frail' if r.frail else ''}{arb}{head}")


def timing(onnx: str, n: int) -> None:
    import onnxruntime as ort

    opts = ort.SessionOptions()
    opts.intra_op_num_threads = THREADS
    root, rows = rows_of("test")
    pics = [Image.open(root / p).convert("RGB") for p, _ in rows[:n]]
    main = ort.InferenceSession(str(HERE / "assets/nsfw.onnx"), opts, providers=["CPUExecutionProvider"])
    grader = ort.InferenceSession(str(HERE / "assets/nsfw_grade.onnx"), opts, providers=["CPUExecutionProvider"])
    tower = ort.InferenceSession(onnx, opts, providers=["CPUExecutionProvider"])
    inp = tower.get_inputs()[0]
    side = int(inp.shape[2])
    for name, fn in [
        ("first model, one view", lambda im: main.run(None, {main.get_inputs()[0].name: ev.planar(ev.fit(im, 384, 384), -1, 1)[None]})),
        ("grader, one view", lambda im: grader.run(None, {grader.get_inputs()[0].name: ev.planar(ev.fit(im, 256, 224), 0, 1)})),
        (f"tower {pathlib.Path(onnx).parent.name} @ {side}px", lambda im: tower.run(None, {inp.name: ev.planar(im.resize((side, side), Image.Resampling.BILINEAR), -1, 1)[None]})),
    ]:
        fn(pics[0])
        t = time.time()
        for im in pics:
            fn(im)
        print(f"  {name:32} {(time.time() - t) / len(pics) * 1000:7.0f} ms")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--set", choices=("heldout", "test"), default="heldout")
    parser.add_argument("--tower", default="base384")
    parser.add_argument("--head", help="head .npz from train_nsfw_head.py --save (default: the shipped nsfw_head_vectors.rs)")
    parser.add_argument("--arbiter", help=".npz with EXPLICIT/SAFE for a tower that is not base224")
    parser.add_argument("--head-sure", type=float)
    parser.add_argument("--head-delete", type=float)
    parser.add_argument("--limits", default="50,80")
    parser.add_argument("--time", action="store_true")
    parser.add_argument("--onnx", default=str(HERE / "target/release/vision.onnx"))
    parser.add_argument("--count", type=int, default=40)
    args = parser.parse_args()

    if args.time:
        timing(args.onnx, args.count)
        return 0

    root, rows = rows_of(args.set)
    scores = ensure_scores(args.set, root, rows)
    emb_path = nsfw_data.EMB / f"{'test' if args.set == 'test' else 'corpus'}_{args.tower}.npz"
    emb = np.load(emb_path, allow_pickle=False)
    by_path = {str(p).replace("anime_nsfw/", "anime_scenes/"): e for p, e in zip(emb["path"], emb["embeddings"])}
    arbiter = load_arbiter(args.arbiter, args.tower)
    head = load_head(args.head)
    if head is None:
        print("no head: pass --head or write nsfw_head_vectors.rs first")
    head_sure = args.head_sure if args.head_sure is not None else (head[2] if head else 0.5)
    head_delete = args.head_delete if args.head_delete is not None else (head[3] if head else 0.9)
    print(f"{args.set}: {len(rows)} pictures, tower {emb['tower']}, "
          f"arbiter {'yes' if arbiter else 'NO'}, head {'yes' if head else 'NO'} "
          f"(HEAD_SURE {head_sure}, HEAD_DELETE {head_delete})")

    out: list[Row] = []
    missing = 0
    for p, g in rows:
        if p not in scores or p not in by_path:
            missing += 1
            continue
        e = by_path[p].astype(np.float32)
        a = float(e @ arbiter[0] - e @ arbiter[1]) if arbiter else None
        h = float(sigmoid(e @ head[0] + head[1])) if head else None
        out.append(Row(p, g, scores[p], a, h))
    if missing:
        print(f"  {missing} rows without scores or embeddings were skipped")

    if args.set == "test":
        pos, amb, neg = nsfw_data.EVAL_POSITIVE, nsfw_data.EVAL_AMBIGUOUS, nsfw_data.EVAL_NEGATIVE
    else:
        pos, amb, neg = nsfw_data.POSITIVE, nsfw_data.AMBIGUOUS, nsfw_data.NEGATIVE
    policies = {"cascade": cascade, "deployed": deployed}
    if head:
        policies["head"] = lambda r, l: with_head(r, l, head_sure, head_delete, False)
        policies["head-always"] = lambda r, l: with_head(r, l, head_sure, head_delete, True)
    for limit in (float(x) for x in args.limits.split(",")):
        table(out, policies, limit, pos, amb, neg)
        if head:
            worst(out, policies["head-always"], limit, neg)
    # How many positives never reach the gate at all — what «always» would be buying.
    for g in pos:
        sub = [r for r in out if r.group == g]
        if sub:
            under = sum(r.peak < GRADE_ABOVE for r in sub)
            print(f"  {g}: {under}/{len(sub)} score under GRADE_ABOVE (never embedded today)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
