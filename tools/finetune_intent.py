#!/usr/bin/env python3
"""Fine-tunes the e5 backbone into «قفل خرید و فروش»'s frame classifier.

The lock scored a mean-pooled sentence embedding along one trained direction until this
replaced it, and the measurement that retired that design is in `trade.rs`: at zero false
positives it held 56% recall, and every negative sitting just under the limit was a *frame*
around commerce content — «نه میخرم نه میفروشم», a quoted ad inside a warning, «اگه پول
داشتم … میخریدم», «دیروز … خریدم». A linear probe on a pooled vector cannot read negation,
tense or quotation. A backbone trained end to end on the frames can.

So this trains the same backbone (`intfloat/multilingual-e5-small`, or `-base` for the
escalation tier) with a linear head over the frames `gen_intent_corpus.py` labels — offer,
want, exchange … past, quote, refusal, hypothetical, inquiry … — and the lock's score is
`logsumexp(sell frames) − logsumexp(safe frames)`, divided by a temperature chosen here so
that the untouched eval set's first zero-false-positive row lands at `DEFAULT_LIMIT` minus
two: the setting a chat stored keeps meaning what it meant.

Two things are load bearing:

* **The eval set selects, it never trains.** After every epoch the eval rows are scored
  through the evidence floor exactly as the runtime applies it, and the epoch with the
  highest recall at zero false positives is the one kept. `intent_battery.tsv` is not even
  looked at here — it is the second judge, and `evaluate_intent.py` reports on it after.
* **The embedding table is frozen**, so the export's vocabulary trim gathers rows the
  training never moved, and the tokenizer pins in `intent.rs` stay true.

    ../bin/python tools/finetune_intent.py --model small --out ./out
    ../bin/python tools/finetune_intent.py --model base  --out ./out
"""

from __future__ import annotations

import argparse
import json
import pathlib
import random
import sys
import time

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(HERE))

import evaluate_intent as ev  # noqa: E402
from export_intent import PREFIX, SAFE, SELL  # noqa: E402
from gen_intent_corpus import FRAMES, NO_FRAME, SAFE_FRAMES, SELL_FRAMES  # noqa: E402

CORPUS = HERE / "data" / "intent_corpus.tsv"
EVAL = HERE / "data" / "intent_eval.tsv"
MODELS = {"small": "intfloat/multilingual-e5-small", "base": "intfloat/multilingual-e5-base"}
TRAIN_TOKENS = 64
EVAL_TOKENS = 128
# What a false positive costs against a miss in the score loss. Two was measured first; the
# battery's discussion rows still reached the boundary, and three is the next thing tried.
SAFE_COST = 3.0


def rows() -> list[tuple[str, str, str]]:
    out = []
    for line in CORPUS.read_text(encoding="utf-8").splitlines():
        label, frame, text = line.split("\t", 2)
        out.append((label, frame, text))
    # The hand-curated exemplar sets join the training data — they are the registers a
    # human vouched for, so they carry extra weight through simple repetition.
    for text in SELL:
        out += [("sell", NO_FRAME, text)] * 2
    for text in SAFE:
        out += [("safe", NO_FRAME, text)] * 2
    return out


def prepare(text: str) -> str:
    """What the runtime feeds the model: the prefix, then the descrambled text."""
    return PREFIX + ev.descramble(text.strip())[0][:512]


class Classifier:
    def __init__(self, name: str, frames: int, seed: int):
        import torch
        from transformers import AutoModel, AutoTokenizer

        torch.manual_seed(seed)
        self.tokenizer = AutoTokenizer.from_pretrained(name)
        self.backbone = AutoModel.from_pretrained(name)
        self.backbone.embeddings.word_embeddings.weight.requires_grad_(False)
        self.head = torch.nn.Linear(self.backbone.config.hidden_size, frames)

    def parameters(self):
        return [p for p in self.backbone.parameters() if p.requires_grad] + list(self.head.parameters())

    def train(self, on: bool):
        self.backbone.train(on)
        self.head.train(on)

    def logits(self, texts: list[str], tokens: int):
        import torch

        batch = self.tokenizer(
            [prepare(t) for t in texts],
            padding=True,
            truncation=True,
            max_length=tokens,
            return_tensors="pt",
        )
        hidden = self.backbone(**batch).last_hidden_state
        mask = batch["attention_mask"].unsqueeze(-1).to(hidden.dtype)
        pooled = (hidden * mask).sum(dim=1) / mask.sum(dim=1).clamp(min=1e-9)
        return self.head(pooled)


def score_of(logits, sell_ids, safe_ids):
    import torch

    return torch.logsumexp(logits[:, sell_ids], dim=1) - torch.logsumexp(logits[:, safe_ids], dim=1)


def judge_on_eval(model: Classifier, sell_ids, safe_ids, constants: ev.Constants):
    """Raw scores for every eval row plus the runtime's deterministic layers, and the recall
    at the first zero-false-positive threshold. The bridge is not applied here — it is
    calibrated in thousandths, and the temperature is not known until the winner is."""
    import torch

    eval_rows = ev.read_rows(EVAL)
    texts = [t for _, _, t in eval_rows]
    labels = np.array([l for l, _, _ in eval_rows])
    marked = [ev.listing_marker(t, constants.listing_words) for t in texts]
    terse = np.array([ev.is_terse(t, m) for t, m in zip(texts, marked)])
    caught = np.array([ev.suspicious(t, constants.marks) for t in texts])
    raw = []
    model.train(False)
    with torch.no_grad():
        for at in range(0, len(texts), 64):
            raw.append(score_of(model.logits(texts[at : at + 64], EVAL_TOKENS), sell_ids, safe_ids).numpy())
    raw = np.concatenate(raw)
    live = caught & ~terse
    negatives = raw[live & (labels != "sell")]
    boundary = float(negatives.max()) if len(negatives) else 0.0
    sells = raw[labels == "sell"]
    live_sells = live[labels == "sell"]
    recall = float(((sells > boundary) & live_sells).mean())
    # And the recall one and two false positives in: how steep the cliff is.
    ordered = np.sort(negatives)
    tolerant = [float(((sells > ordered[-k - 1]) & live_sells).mean()) if len(ordered) > k else recall for k in (1, 2)]
    # The negatives holding the boundary, so a label error is a line in the log and not a
    # mystery — one mislabeled row once crowned a garbage head.
    top = np.argsort(-np.where(live & (labels != "sell"), raw, -np.inf))[:3]
    for at in top:
        print(f"    negative {raw[at]:+.3f}  {texts[at][:80]}", flush=True)
    return raw, boundary, recall, tolerant


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", choices=MODELS, default="small")
    parser.add_argument("--out", default="./out")
    parser.add_argument("--epochs", type=int, default=4)
    parser.add_argument("--batch", type=int, default=32)
    parser.add_argument("--lr", type=float, default=3e-5)
    parser.add_argument("--seed", type=int, default=7)
    parser.add_argument("--threads", type=int, default=16)
    args = parser.parse_args()

    import torch

    torch.set_num_threads(args.threads)
    rng = random.Random(args.seed)
    constants = ev.Constants()
    frame_id = {frame: at for at, frame in enumerate(FRAMES)}
    sell_ids = [frame_id[f] for f in SELL_FRAMES]
    safe_ids = [frame_id[f] for f in SAFE_FRAMES]

    data = rows()
    print(f"training on {len(data)} rows ({sum(1 for l, _, _ in data if l == 'sell')} sell), {len(FRAMES)} frames")
    counts = np.array([sum(1 for _, f, _ in data if f == frame) for frame in FRAMES], dtype=np.float64)
    print(f"  {sum(1 for _, f, _ in data if f == NO_FRAME)} rows carry a label and no frame")
    # Class weights per frame, square-root damped: a frame with forty rows is not fifty
    # times as important as one with two thousand, but it is not to be drowned either.
    weights = np.sqrt(counts.sum() / (len(FRAMES) * np.maximum(counts, 1)))
    weights = torch.tensor(weights / weights.mean(), dtype=torch.float32)

    model = Classifier(MODELS[args.model], len(FRAMES), args.seed)
    steps = args.epochs * ((len(data) + args.batch - 1) // args.batch)
    optimizer = torch.optim.AdamW(model.parameters(), lr=args.lr, weight_decay=0.01)
    warmup = max(1, steps // 16)
    schedule = torch.optim.lr_scheduler.LambdaLR(
        optimizer, lambda s: min(1.0, (s + 1) / warmup) * max(0.0, (steps - s) / max(1, steps - warmup))
    )
    # A free-written row carries a label and no frame: it trains the score and sits out of
    # the frame loss, which is what `ignore_index` is for.
    frame_loss = torch.nn.CrossEntropyLoss(weight=weights, ignore_index=-1)

    out = pathlib.Path(args.out) / args.model
    out.mkdir(parents=True, exist_ok=True)
    best = None
    step = 0
    for epoch in range(args.epochs):
        rng.shuffle(data)
        model.train(True)
        started = time.time()
        running = 0.0
        for at in range(0, len(data), args.batch):
            chunk = data[at : at + args.batch]
            texts = [t for _, _, t in chunk]
            target = torch.tensor([frame_id.get(f, -1) for _, f, _ in chunk])
            is_sell = torch.tensor([1.0 if l == "sell" else 0.0 for l, _, _ in chunk])
            logits = model.logits(texts, TRAIN_TOKENS)
            score = score_of(logits, sell_ids, safe_ids)
            # The frame loss teaches the classes; the score loss teaches the one number the
            # lock thresholds, with a safe row weighing `SAFE_COST` sell rows — a false
            # positive is the expensive error, and the loss says so.
            cost = torch.where(is_sell > 0, 1.0, SAFE_COST)
            score_loss = (torch.nn.functional.binary_cross_entropy_with_logits(score, is_sell, reduction="none") * cost).mean()
            loss = frame_loss(logits, target) + score_loss
            optimizer.zero_grad()
            loss.backward()
            torch.nn.utils.clip_grad_norm_(model.parameters(), 1.0)
            optimizer.step()
            schedule.step()
            step += 1
            running += float(loss.detach())
            if step % 50 == 0:
                done = at + len(chunk)
                print(f"  epoch {epoch + 1} {done}/{len(data)} loss {running / 50:.3f} {time.time() - started:.0f}s", flush=True)
                running = 0.0
        raw, boundary, recall, tolerant = judge_on_eval(model, sell_ids, safe_ids, constants)
        print(
            f"epoch {epoch + 1}: eval zero-FP recall {recall:.1%} (boundary {boundary:+.3f}); "
            f"at one FP {tolerant[0]:.1%}, two {tolerant[1]:.1%}; {time.time() - started:.0f}s",
            flush=True,
        )
        # Selection is the mean recall at zero, one and two tolerated negatives, not the
        # zero row alone: the temperature below guarantees zero eval false positives at the
        # default whichever epoch ships, and the strict row is one outlier away from crowning
        # the wrong epoch — measured, the same head swung 55%..70% on it between epochs
        # while the one-negative row said 60%..89%. The mean rewards a boundary with the
        # bulk of the negatives far under it, which is what an unseen message meets.
        key = (round((recall + tolerant[0] + tolerant[1]) / 3, 4), round(recall, 4))
        if best is None or key > best["key"]:
            # The temperature: the boundary raw score becomes DEFAULT_LIMIT − 2 thousandths,
            # so the default keeps two points of headroom over the eval's worst negative.
            target = (constants.default_limit - 2) / 1000.0
            temperature = boundary / target if boundary > 0 else 1.0
            best = {"key": key, "epoch": epoch + 1, "recall": recall, "boundary": boundary, "temperature": temperature}
            model.backbone.save_pretrained(out)
            model.tokenizer.save_pretrained(out)
            torch.save(model.head.state_dict(), out / "head.pt")
            (out / "meta.json").write_text(
                json.dumps(
                    {
                        "model": MODELS[args.model],
                        "frames": FRAMES,
                        "sell_frames": SELL_FRAMES,
                        "temperature": temperature,
                        "epoch": epoch + 1,
                        "eval_zero_fp_recall": recall,
                        "eval_boundary_raw": boundary,
                    },
                    indent=2,
                ),
                encoding="utf-8",
            )
            print(f"  kept epoch {epoch + 1} -> {out} (temperature {temperature:.4f})", flush=True)
    print(f"\nwinner: epoch {best['epoch']}, eval zero-FP recall {best['recall']:.1%}")
    print(f"next: ../bin/python tools/export_intent.py --finetuned {out} --out {args.out} --check")


if __name__ == "__main__":
    main()
