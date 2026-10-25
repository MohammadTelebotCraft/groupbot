#!/usr/bin/env python3
"""Trains the concept heads: one logistic direction per concept lock, in the general model's space.

    ../bin/python tools/train_concept_heads.py                 # fit, compare with the prompt margins
    ../bin/python tools/train_concept_heads.py --write         # and write concept_head_vectors.rs

The six concept locks were prompt margins — `cos(concept) - cos(BACKGROUND)` — with a contrast
bolted on for every lookalike that was ever deleted (chainsaw, sword, scissors, tool, ammunition,
red wine, red drink, cannabis bud against dried herb). Each contrast fixed the picture it was
written for and none of them could be measured against the next one, because there was no corpus
of what each lock is *for*. `concept_data.py` builds that corpus from Open Images' human-verified
labels; this fits a direction per concept to it, the way `train_nsfw_head.py` fitted the NSFW head
after the same kind of prompt margin had run out.

The cut is chosen on a **dev slice of the training split** (a hash of the path, a sixth of it) and
never on the evaluation split, which is only reported. It is the lowest logit that keeps both

    everyday   the NSFW corpus's ordinary groups — people, food, memes, stickers, screenshots,
               anime, emoji: what a group actually sends — at or under EVERYDAY_FPR
    hard       Open Images' verified negatives and the concept's lookalikes at or under HARD_FPR

and that logit is what `DEFAULT_LIMIT` means in the bot. Each limit step is a tenth of a logit
either side of it (`concepts::LOGITS_PER_THOUSANDTH`), so a chat's existing «حساسیت» keeps its
direction: lower deletes more.

What is printed for each concept is a row per kind of picture, old prompt margin beside the new
head at the default limit, so a lookalike that the head gets wrong is a number before it is a
deletion.
"""

from __future__ import annotations

import argparse
import hashlib
import pathlib
import re
import sys
from collections import defaultdict

import numpy as np

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
import concept_data  # noqa: E402
from train_nsfw_head import auc, fit, sigmoid  # noqa: E402

HERE = pathlib.Path(__file__).resolve().parent.parent
OUT_RS = HERE / "src/handlers/concept_head_vectors.rs"
VECTORS_RS = HERE / "src/handlers/concept_vectors.rs"
RETIRED_VECTORS = HERE / "tools/data/prompt_vectors.txt"
EMB = concept_data.ROOT / "emb_base384.npz"
SWEEP = (0.1, 0.3, 1.0, 3.0, 10.0, 30.0)
EVERYDAY_FPR = 0.002
HARD_FPR = 0.03
# All negatives together may not be deleted more often than the prompt margin deleted them at the
# default limit on the same dev rows, and never more than this — so at the default every head is
# a recall gain at no more false positives, not a trade of one for the other.
TOTAL_FPR_CAP = 0.01
# ... and never less than this: the dev slice is a few thousand pictures, so a budget under a
# handful of them is noise, not a measurement, and it made gambling's cut stricter than the
# prompt margin it replaced.
TOTAL_FPR_FLOOR = 0.002
# Mirrors `concepts::LOGITS_PER_THOUSANDTH` and `concepts::DEFAULT_LIMIT`.
LOGITS_PER_THOUSANDTH = 0.1
DEFAULT_LIMIT = 20
# A concept with fewer verified pictures than this is not fitted. It keeps its caption direction,
# written as a head that reproduces the old prompt margin exactly — `weights = (CONCEPT -
# BACKGROUND) / (LOGITS_PER_THOUSANDTH / 1000)`, bias and cut chosen so the margin `concepts.rs`
# computes is the cosine difference itself. On 2026-10-14 that is the drug lock: Open Images'
# «Joint» is a knee and its «Cannabis» a potted plant, and Commons rate-limited the replacement.
MIN_POSITIVES = 60
NAMES = {"c_cig": "CIGARETTE", "c_alc": "ALCOHOL", "c_gun": "WEAPON", "c_bet": "GAMBLING",
         "c_drg": "DRUGS", "c_bld": "BLOOD"}


def rust_vector(name: str) -> np.ndarray:
    source = VECTORS_RS.read_text(encoding="utf-8") + RETIRED_VECTORS.read_text(encoding="utf-8")
    found = re.search(rf"pub const {name}: \[f32; \d+\] = \[(.*?)\];", source, re.S)
    if not found:
        sys.exit(f"{name} is not in concept_vectors.rs")
    return np.asarray([float(x) for x in re.findall(r"-?\d+\.\d+(?:e-?\d+)?", found.group(1))], dtype=np.float32)


def prompt_margins(x: np.ndarray) -> np.ndarray:
    """`concepts::margins_from` as it shipped before the heads, whole frame only: no crop rescue,
    and an unverified blood candidate counted as failed verification (zero), which is what the
    still path does when the crops do not confirm it."""
    v = {n: rust_vector(n) for n in (
        "BACKGROUND", "CIGARETTE", "ALCOHOL", "GAMBLING", "DRUGS", "BLOOD", "WEAPON_GUN", "WEAPON_KNIFE",
        "CHAINSAW", "SWORD", "CANNABIS_BUD", "DRIED_HERB", "WEAPON_AMMO_ONLY", "WEAPON_GUN_WITH_AMMO",
        "RED_WINE", "TOOL", "SCISSORS")}
    d = {n: x @ vec for n, vec in v.items()}
    base = d["BACKGROUND"]
    out = np.zeros((len(x), 6), dtype=np.float32)
    out[:, 0] = d["CIGARETTE"] - base
    out[:, 1] = d["ALCOHOL"] - base
    gun, knife = d["WEAPON_GUN"], d["WEAPON_KNIFE"]
    weapon = np.maximum(gun, knife)
    w = np.where((weapon - d["CHAINSAW"] > 0.030) & (weapon > d["SWORD"]), weapon - base, 0.0)
    w = np.where((gun >= knife) & (d["WEAPON_AMMO_ONLY"] >= gun) & (d["WEAPON_AMMO_ONLY"] >= d["WEAPON_GUN_WITH_AMMO"]), 0.0, w)
    w = np.where((w > 0) & ~((gun >= knife) | (knife - d["SWORD"] >= 0.010)), 0.0, w)
    w = np.where((w > 0) & (knife > gun) & ((d["TOOL"] - knife >= 0.020) | (d["SCISSORS"] - knife >= 0.025)), 0.0, w)
    out[:, 2] = w
    out[:, 3] = d["GAMBLING"] - base
    drugs = d["DRUGS"] - base
    out[:, 4] = np.where(d["CANNABIS_BUD"] > d["DRIED_HERB"], np.maximum(drugs, d["CANNABIS_BUD"] - base), drugs)
    blood = d["BLOOD"] - base
    out[:, 5] = np.where((blood >= 0.005) & (d["BLOOD"] <= d["RED_WINE"]), 0.0, blood)
    return out


def load():
    rows = concept_data.ROOT.joinpath("manifest.tsv").read_text(encoding="utf-8").splitlines()
    header = rows[0].split("\t")
    slots = concept_data.SLOTS
    at = {name: i for i, name in enumerate(header)}
    meta = {}
    for line in rows[1:]:
        cols = line.split("\t")
        meta[cols[0]] = (cols[1], cols[2], [int(cols[at[s]]) for s in slots], [cols[at[f"{s}_kind"]] for s in slots])
    emb = np.load(EMB, allow_pickle=False)
    keep = [i for i, p in enumerate(emb["path"]) if str(p) in meta]
    paths = [str(emb["path"][i]) for i in keep]
    x = emb["embeddings"][keep].astype(np.float32)
    split = np.array([meta[p][0] for p in paths])
    source = np.array([meta[p][1] for p in paths])
    y = np.array([meta[p][2] for p in paths], dtype=np.int64)
    kind = np.array([meta[p][3] for p in paths])
    dev = np.array([hashlib.sha1(p.encode()).hexdigest()[0] in "ab" for p in paths]) & (split == "train")
    return paths, x, split, source, y, kind, dev, str(emb["tower"])


def everyday_of(kind: np.ndarray) -> np.ndarray:
    return np.char.startswith(kind.astype(str), "corpus:")


def cut_for(z: np.ndarray, yy: np.ndarray, everyday: np.ndarray, total: float = 1.0) -> float:
    """The lowest logit meeting every false-positive budget on these rows: everyday pictures,
    hard ones, and all negatives together."""
    neg = yy == 0
    cuts = []
    for mask, rate in ((neg & everyday, EVERYDAY_FPR), (neg & ~everyday, HARD_FPR), (neg, total)):
        values = np.sort(z[mask])[::-1]
        if len(values) == 0:
            continue
        allowed = int(np.floor(rate * len(values)))
        cuts.append(values[allowed] + 1e-4 if allowed < len(values) else -np.inf)
    return float(max(cuts)) if cuts else 0.0


def group_of(kind: str) -> str:
    if kind.startswith("corpus:"):
        return kind
    return kind.split(":")[0] if kind.split(":")[0] in ("positive", "hard", "other") else kind


def table(slot: int, name: str, kinds: np.ndarray, yy: np.ndarray, hit_old: np.ndarray, hit_new: np.ndarray) -> dict:
    groups = defaultdict(list)
    for i, k in enumerate(kinds):
        groups[group_of(k)].append(i)
    print(f"\n  {name} — eval, at the default limit: old prompt margin | new head")
    order = sorted(groups, key=lambda g: (not g.startswith("positive"), g.startswith("corpus:"), g))
    totals = {"old": [0, 0, 0, 0], "new": [0, 0, 0, 0]}
    for g in order:
        idx = np.array(groups[g])
        label = yy[idx][0] if len(set(yy[idx])) == 1 else None
        o, n = int(hit_old[idx].sum()), int(hit_new[idx].sum())
        mark = "pos" if label == 1 else ("amb" if label == -1 else "neg")
        flag = ""
        if mark == "neg" and n > o:
            flag = "  <-- more FP"
        if mark == "pos" and n < o:
            flag = "  <-- fewer caught"
        print(f"    {g:34} {mark} {o:5d} | {n:5d}  of {len(idx):5d}{flag}")
        for tag, h in (("old", o), ("new", n)):
            if mark == "pos":
                totals[tag][0] += h
                totals[tag][1] += len(idx)
            elif mark == "neg":
                totals[tag][2] += h
                totals[tag][3] += len(idx)
    for tag in ("old", "new"):
        t = totals[tag]
        print(f"    {tag}: recall {t[0] / max(1, t[1]):.3f} ({t[0]}/{t[1]})   false positives {t[2]}/{t[3]} ({t[2] / max(1, t[3]):.4f})")
    return totals


def write_rust(tower: str, heads: list[tuple[str, np.ndarray, float, float]], metrics: list[str], pins: list[float] | None) -> None:
    blocks = []
    for (slot, w, b, cut) in heads:
        values = [f"{v:.6f}" for v in w.tolist()]
        rows = ["        " + ", ".join(values[at: at + 8]) + "," for at in range(0, len(values), 8)]
        blocks.append(f"""    // {slot}
    Head {{
        bias: {b:.6f},
        cut: {cut:.6f},
        weights: [
{chr(10).join(rows)}
        ],
    }},""")
    pin_line = ""
    synthetic = ""
    if pins is not None:
        joined = ", ".join(f"{p:.4f}" for p in pins)
        pin_line = f"//! synthetic test picture logits: {joined}\n"
        synthetic = ("\n/// Each head's logit on the synthetic picture `vision::tests` draws, through the export.\n"
                     f"#[cfg(test)]\npub const SYNTHETIC_LOGITS: [f32; {len(pins)}] = [{joined}];\n")
    text = f'''//! The concept heads, generated by `tools/train_concept_heads.py` and never computed here.
//!
//! One logistic direction per concept, in `concepts::CONCEPTS` order: `z = weights · e + bias` on
//! the unit embedding `vision::embed_of` already produced, and `cut` is the logit the default
//! limit stands for. Fitted to `tools/concept_data.py`'s corpus — see `concepts.rs` for how a
//! logit becomes the margin a chat's limit is compared with.
//!
//! checkpoint: {tower}
{chr(10).join("//! " + m for m in metrics)}
{pin_line}// Generated decimal spellings are the trainer's serialized f32 parameters. Hand-grouping digits
// would make every regeneration noisy, and rounding `excessive_precision` literals can change the
// fitted model. This narrow data-only exception contains no executable control flow.
#![allow(clippy::unreadable_literal, clippy::excessive_precision)]

use super::vision::DIM;

pub struct Head {{
    pub bias: f32,
    pub cut: f32,
    pub weights: [f32; DIM],
}}

pub const COUNT: usize = {len(heads)};

pub static HEADS: [Head; COUNT] = [
{chr(10).join(blocks)}
];
{synthetic}'''
    OUT_RS.write_text(text, encoding="utf-8")
    print(f"\nwrote {OUT_RS}")


def synthetic_logits(onnx: str, heads) -> list[float] | None:
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
    return [float(e @ w + b) for (_, w, b, _) in heads]


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--onnx", default=str(HERE / "target/release/vision.onnx"))
    parser.add_argument("--save", help="also save the heads as .npz")
    args = parser.parse_args()

    paths, x, split, source, y, kind, dev, tower = load()
    print(f"{tower}: {len(paths)} pictures, train {int((split == 'train').sum())} "
          f"(dev {int(dev.sum())}), eval {int((split == 'eval').sum())}")
    old = prompt_margins(x)
    fit_rows = (split == "train") & ~dev
    heads, metrics = [], []
    for s, slot in enumerate(concept_data.SLOTS):
        ys = y[:, s]
        everyday = everyday_of(kind[:, s])
        tr = fit_rows & (ys >= 0)
        dv = dev & (ys >= 0)
        ev = (split == "eval") & (ys >= 0)
        n_pos, n_neg = int((ys[tr] == 1).sum()), int((ys[tr] == 0).sum())
        if n_pos < MIN_POSITIVES:
            scale = 1000.0 * LOGITS_PER_THOUSANDTH
            w = scale * (rust_vector(NAMES[slot]) - rust_vector("BACKGROUND"))
            b = -DEFAULT_LIMIT * LOGITS_PER_THOUSANDTH
            print(f"\n{slot} ({NAMES[slot]}): {n_pos} verified positives — keeping the caption "
                  f"direction as its head (margin identical to the prompt margin)")
            metrics.append(f"{slot}: caption direction kept, {n_pos} verified pictures to fit on")
            heads.append((slot, w.astype(np.float32), float(b), 0.0))
            continue
        weight = np.where(ys[tr] == 1, 0.5 * tr.sum() / n_pos, 0.5 * tr.sum() / n_neg)
        old_dev_fpr = float((old[dv, s][ys[dv] == 0] * 1000 >= DEFAULT_LIMIT).mean())
        total = min(max(old_dev_fpr, TOTAL_FPR_FLOOR), TOTAL_FPR_CAP)
        best = None
        for l2 in SWEEP:
            w, b = fit(x[tr], ys[tr].astype(np.float64), weight, l2)
            z = x[dv] @ w + b
            cut = cut_for(z, ys[dv], everyday[dv], total)
            recall = float((z[ys[dv] == 1] >= cut).mean())
            key = (round(recall, 3), auc(z, ys[dv]))
            if best is None or key > best[0]:
                best = (key, l2, w, b, cut)
        (recall, dev_auc), l2, w, b, cut = best
        # Refit on all of train (fit + dev) at the chosen strength; the cut stays the dev one,
        # re-derived for the refit so it refers to the same false-positive budget.
        tr_all = (split == "train") & (ys >= 0)
        weight = np.where(ys[tr_all] == 1, 0.5 * tr_all.sum() / (ys[tr_all] == 1).sum(),
                          0.5 * tr_all.sum() / (ys[tr_all] == 0).sum())
        w2, b2 = fit(x[tr_all], ys[tr_all].astype(np.float64), weight, l2)
        z_dev = x[dv] @ w2 + b2
        cut2 = cut_for(z_dev, ys[dv], everyday[dv], total)
        name = NAMES[slot]
        print(f"\n{slot} ({name}): train {n_pos} positive / {n_neg} negative; l2 {l2}; dev recall {recall:.3f} "
              f"AUC {dev_auc:.4f}; cut {cut2:+.3f} (refit); dev false-positive budget {total:.4f} "
              f"(prompt margin {old_dev_fpr:.4f})")

        z_ev = x[ev] @ w2 + b2
        yy = ys[ev]
        hit_new = z_ev >= cut2
        hit_old = old[ev, s] * 1000 >= DEFAULT_LIMIT
        totals = table(s, name, kind[ev, s], yy, hit_old, hit_new)
        a_new = auc(z_ev, yy)
        a_old = auc(old[ev, s], yy)
        print(f"    AUC old {a_old:.4f} new {a_new:.4f}")
        # Old prompt margin at the limit that gives the same false positives as the head —
        # the fair comparison, since the old default was never tuned to this corpus.
        neg_scores = np.sort(old[ev, s][yy == 0])[::-1]
        fp_new = int(totals["new"][2])
        if fp_new < len(neg_scores):
            old_cut = neg_scores[fp_new] + 1e-6
            rec_old_eq = float((old[ev, s][yy == 1] >= old_cut).mean())
            print(f"    at equal false positives ({fp_new}): old recall {rec_old_eq:.3f} vs new {totals['new'][0] / max(1, totals['new'][1]):.3f}")
        metrics.append(f"{slot}: eval recall {totals['new'][0] / max(1, totals['new'][1]):.3f} "
                       f"false positives {totals['new'][2]}/{totals['new'][3]} AUC {a_new:.4f} (prompt margin {a_old:.4f})")
        heads.append((slot, w2.astype(np.float32), float(b2), float(cut2)))

    if args.save:
        np.savez(args.save, **{f"{s}_w": h[1] for s, h in zip(concept_data.SLOTS, heads)},
                 **{f"{s}_b": np.array(h[2]) for s, h in zip(concept_data.SLOTS, heads)},
                 **{f"{s}_cut": np.array(h[3]) for s, h in zip(concept_data.SLOTS, heads)})
        print(f"saved {args.save}")
    if args.write:
        pins = synthetic_logits(args.onnx, heads) if pathlib.Path(args.onnx).exists() else None
        write_rust(tower, heads, metrics, pins)
        if pins is not None:
            print("synthetic picture logits (pin these in concepts tests):", ", ".join(f"{p:.4f}" for p in pins))
    return 0


if __name__ == "__main__":
    sys.exit(main())
