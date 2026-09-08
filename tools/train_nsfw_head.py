#!/usr/bin/env python3
"""Trains the NSFW head: one logistic direction in the general model's space.

    ../bin/python tools/train_nsfw_head.py --tower base384
    ../bin/python tools/train_nsfw_head.py --tower base384 --write --onnx target/release/vision.onnx

The arbiter's `EXPLICIT - SAFE` margin is the unit mean of fourteen captions, and captions are
about photographs — «a pornographic photo», «full frontal nudity». Measured on live traffic a
hentai frame the five-class grader scored 0.94 sat at +0.012 on that margin, under the veto
line, because nothing in the prompt set describes it. A direction *fitted* to pictures rather
than described in words does not have that problem, provided the pictures include what it is
meant to find — which is the lesson `nsfw_data.py` records: the first head was trained on a
corpus whose only «drawn» group was ordinary anime, and it vetoed hentai.

What comes out is `DIM + 1` floats — `w` and `b`, `p = sigmoid(w · e + b)` on the unit
embedding — written as a generated Rust constant the way `concept_vectors.rs` is, so the bot
pays one dot product and loads no file. In the bot it does three things (`nsfw_head.rs`):
under `HEAD_VETO` it withholds, above `HEAD_SURE` it is positive evidence, above
`HEAD_DELETE` it is the score itself. This prints `HEAD_SURE`/`HEAD_DELETE` — the cut at which
held-out ordinary pictures produce no false positive at all, with a margin — and
`nsfw_bench.py` measures the veto and confirms all three on the untouched test set before any
of them is written into `nsfw.rs`.

Training is plain L2-regularised logistic regression, class-balanced, full-batch L-BFGS; the
regularisation strength is chosen on the held-out split by recall at zero false positives and
then AUC. There is nothing to tune by hand and the whole run is seconds.
"""

from __future__ import annotations

import argparse
import pathlib
import sys
import time

import numpy as np

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import nsfw_data  # noqa: E402

HERE = pathlib.Path(__file__).resolve().parent.parent
OUT_RS = HERE / "src/handlers/nsfw_head_vectors.rs"
MARGIN = 0.02  # above the worst held-out negative
SWEEP = (0.03, 0.1, 0.3, 1.0, 3.0, 10.0, 30.0)


def sigmoid(z: np.ndarray) -> np.ndarray:
    return 1.0 / (1.0 + np.exp(-np.clip(z, -60, 60)))


def fit(x: np.ndarray, y: np.ndarray, weight: np.ndarray, l2: float) -> tuple[np.ndarray, float]:
    """Weighted logistic regression, L2 on `w` only."""
    n, d = x.shape
    theta0 = np.zeros(d + 1)

    def objective(theta: np.ndarray):
        w, b = theta[:d], theta[d]
        z = x @ w + b
        p = sigmoid(z)
        eps = 1e-9
        loss = -(weight * (y * np.log(p + eps) + (1 - y) * np.log(1 - p + eps))).sum() / n
        loss += 0.5 * l2 * (w @ w) / n
        g = weight * (p - y) / n
        grad = np.concatenate([x.T @ g + l2 * w / n, [g.sum()]])
        return loss, grad

    try:
        from scipy.optimize import minimize

        res = minimize(objective, theta0, jac=True, method="L-BFGS-B", options={"maxiter": 500})
        theta = res.x
    except ImportError:
        theta = theta0
        lr = 1.0
        for _ in range(3000):
            _, grad = objective(theta)
            theta -= lr * grad
    return theta[:d].astype(np.float32), float(theta[d])


def auc(scores: np.ndarray, labels: np.ndarray) -> float:
    order = np.argsort(scores)
    ranks = np.empty(len(scores), dtype=np.float64)
    ranks[order] = np.arange(1, len(scores) + 1)
    pos = labels == 1
    n_pos, n_neg = pos.sum(), (~pos).sum()
    if n_pos == 0 or n_neg == 0:
        return float("nan")
    return float((ranks[pos].sum() - n_pos * (n_pos + 1) / 2) / (n_pos * n_neg))


def recall_at_zero_fp(p: np.ndarray, y: np.ndarray) -> tuple[float, float]:
    worst = p[y == 0].max() if (y == 0).any() else 0.0
    cut = min(worst + MARGIN, 0.999)
    return float((p[y == 1] >= cut).mean()), float(cut)


def load(tower: str, name: str):
    path = nsfw_data.EMB / f"{name}_{tower}.npz"
    if not path.exists():
        return None
    d = np.load(path, allow_pickle=False)
    return {
        "x": d["embeddings"].astype(np.float32),
        "group": d["group"].astype(str),
        "split": d["split"].astype(str),
        "path": d["path"].astype(str),
        "tower": str(d["tower"]),
        "side": int(d["side"]),
    }


def labels_of(groups: np.ndarray, positive: list[str], negative: list[str]) -> np.ndarray:
    """1 / 0 / -1 (excluded)."""
    y = np.full(len(groups), -1, dtype=np.int64)
    y[np.isin(groups, positive)] = 1
    y[np.isin(groups, negative)] = 0
    return y


def report(name: str, p: np.ndarray, groups: np.ndarray, y: np.ndarray, cuts: dict[str, float]) -> None:
    keep = y >= 0
    print(f"\n{name}: {int((y == 1).sum())} positive, {int((y == 0).sum())} ordinary, "
          f"AUC {auc(p[keep], y[keep]):.4f}")
    r0, c0 = recall_at_zero_fp(p[keep], y[keep])
    print(f"  recall at 0 FP {r0:.3f} (cut {c0:.3f})")
    for label, cut in cuts.items():
        print(f"  at {label} = {cut:.3f}:")
        for g in sorted(set(groups)):
            m = groups == g
            hit = int((p[m] >= cut).sum())
            kind = "pos" if g in nsfw_data.POSITIVE + nsfw_data.EVAL_POSITIVE else (
                "amb" if g in nsfw_data.AMBIGUOUS else "neg")
            rate = hit / max(1, m.sum())
            flag = "  <-- FP" if kind == "neg" and hit else ""
            print(f"    {g:15} {kind} {hit:5d}/{int(m.sum()):5d} {rate:.3f}{flag}")


def synthetic_head(onnx: str, w: np.ndarray, b: float) -> float | None:
    """The head on the synthetic picture `vision::tests` pins — drawn at the graph's own side,
    where nothing resamples."""
    try:
        import onnxruntime as ort
    except ImportError:
        return None
    session = ort.InferenceSession(onnx, providers=["CPUExecutionProvider"])
    inp = session.get_inputs()[0]
    side = int(inp.shape[2]) if isinstance(inp.shape[2], int) else 224
    xs, ys = np.meshgrid(np.arange(side), np.arange(side))
    r = (xs * 7 + ys * 13) % 256
    g = (xs * 3) % 256
    bb = (ys * 5) % 256
    pixels = np.stack([r, g, bb]).astype(np.float32) / 127.5 - 1.0
    e = session.run(None, {inp.name: pixels[None]})[0][0]
    e = e / np.linalg.norm(e)
    return float(sigmoid(e @ w + b))


def write_rust(tower: str, w: np.ndarray, b: float, metrics: str, pin: float | None) -> None:
    values = [f"{v:.6f}" for v in w.tolist()]
    rows = ["    " + ", ".join(values[at : at + 8]) + "," for at in range(0, len(values), 8)]
    pin_line = f"//! synthetic test picture: {pin:.6f}\n" if pin is not None else ""
    text = f'''//! The NSFW head, generated by `tools/train_nsfw_head.py` and never computed here.
//!
//! One logistic direction in the general model's space: `p = sigmoid(WEIGHTS · e + BIAS)` on
//! the unit embedding `vision::embed_of` already produced. Fitted, not described — see
//! `nsfw_head.rs` for what it may and may not do, and `tools/nsfw_data.py` for what it was
//! fitted to.
//!
//! checkpoint: {tower}
//! {metrics}
{pin_line}// Generated decimal spellings are the trainer's serialized f32 parameters. Hand-grouping digits
// would make every regeneration noisy, and rounding `excessive_precision` literals can change the
// fitted model. This narrow data-only exception contains no executable control flow.
#![allow(clippy::unreadable_literal, clippy::excessive_precision)]

pub const BIAS: f32 = {b:.6f};

pub const WEIGHTS: [f32; {len(w)}] = [
{chr(10).join(rows)}
];
'''
    OUT_RS.write_text(text, encoding="utf-8")
    print(f"\nwrote {OUT_RS}")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--tower", default="base384")
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--onnx", default=str(HERE / "target/release/vision.onnx"))
    parser.add_argument("--l2", type=float, help="skip the sweep and use this")
    parser.add_argument("--save", help="also save w/b as .npz for nsfw_bench.py")
    args = parser.parse_args()

    corpus = load(args.tower, "corpus")
    if corpus is None:
        sys.exit(f"no {nsfw_data.EMB}/corpus_{args.tower}.npz — run vision_embed.py first")
    test = load(args.tower, "test")
    y_all = labels_of(corpus["group"], nsfw_data.POSITIVE, nsfw_data.NEGATIVE)
    train = (corpus["split"] == "train") & (y_all >= 0)
    held = corpus["split"] == "heldout"
    x_tr, y_tr = corpus["x"][train], y_all[train]
    n_pos, n_neg = (y_tr == 1).sum(), (y_tr == 0).sum()
    weight = np.where(y_tr == 1, 0.5 * len(y_tr) / n_pos, 0.5 * len(y_tr) / n_neg)
    print(f"{corpus['tower']} @ {corpus['side']}px: train {n_pos} positive / {n_neg} ordinary, "
          f"held-out {int(held.sum())}")
    print("groups:", ", ".join(f"{g} {int((corpus['group'] == g).sum())}" for g in sorted(set(corpus["group"]))))

    best = None
    for l2 in ([args.l2] if args.l2 else SWEEP):
        t = time.time()
        w, b = fit(x_tr, y_tr, weight, l2)
        p = sigmoid(corpus["x"][held] @ w + b)
        yh = y_all[held]
        keep = yh >= 0
        r0, cut = recall_at_zero_fp(p[keep], yh[keep])
        a = auc(p[keep], yh[keep])
        print(f"  l2 {l2:>5}: held-out recall@0FP {r0:.3f} (cut {cut:.3f}) AUC {a:.4f} {time.time() - t:.1f}s")
        key = (round(r0, 3), a)
        if best is None or key > best[0]:
            best = (key, l2, w, b)
    _, l2, w, b = best
    print(f"\nchosen l2 {l2}")

    # The cuts: the worst held-out ordinary picture plus a margin is HEAD_SURE; HEAD_DELETE is
    # the stricter of that and 0.90, because the head alone raising a score past a chat's limit
    # is the one thing here that can delete without any other model agreeing.
    p_held = sigmoid(corpus["x"][held] @ w + b)
    yh = y_all[held]
    keep = yh >= 0
    _, cut0 = recall_at_zero_fp(p_held[keep], yh[keep])
    head_sure = round(min(max(cut0, 0.5), 0.95), 2)
    head_delete = round(max(head_sure, 0.90), 2)
    cuts = {"HEAD_SURE": head_sure, "HEAD_DELETE": head_delete}
    report("held-out", p_held, corpus["group"][held], yh, cuts)

    metrics = f"held-out recall@0FP {recall_at_zero_fp(p_held[keep], yh[keep])[0]:.3f} AUC {auc(p_held[keep], yh[keep]):.4f}"
    if test is not None:
        if test["tower"] != corpus["tower"]:
            print(f"\nWARNING: test file is {test['tower']}, corpus is {corpus['tower']}")
        p_t = sigmoid(test["x"] @ w + b)
        y_t = labels_of(test["group"], nsfw_data.EVAL_POSITIVE, nsfw_data.EVAL_NEGATIVE)
        report("test (untouched)", p_t, test["group"], y_t, cuts)
        kt = y_t >= 0
        metrics += f"; test recall@0FP {recall_at_zero_fp(p_t[kt], y_t[kt])[0]:.3f} AUC {auc(p_t[kt], y_t[kt]):.4f}"
    print(f"\nHEAD_SURE = {head_sure}  HEAD_DELETE = {head_delete}")

    if args.save:
        np.savez(args.save, w=w, b=np.array(b, dtype=np.float32), tower=np.array(corpus["tower"]),
                 head_sure=np.array(head_sure), head_delete=np.array(head_delete))
        print(f"saved {args.save}")
    if args.write:
        pin = synthetic_head(args.onnx, w, b) if pathlib.Path(args.onnx).exists() else None
        write_rust(corpus["tower"], w, b, metrics, pin)
        if pin is not None:
            print(f"synthetic picture head value (pin this in vision.rs tests): {pin:.6f}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
