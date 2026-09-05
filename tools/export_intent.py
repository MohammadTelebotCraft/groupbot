#!/usr/bin/env python3
"""Exports «قفل خرید و فروش»'s classifier — the fine-tuned e5 backbone and its frame head.

The vision tower cannot do this job: SigLIP's text tower lands phrases in the *image*
space — two sentences are near each other when they would caption the same picture, not
when they mean the same thing. Selling intent is a fact about meaning, so the text features
get their own encoder, and since the model is fine-tuned end to end (`finetune_intent.py`)
it is not an encoder any more but a classifier over the *frames* a message can carry:

    intfloat/multilingual-e5-small    the always-on model (384 wide, XLM-R tokenizer,
                                      every input prefixed "query: ")
    intfloat/multilingual-e5-base     the escalation tier (768 wide, same tokenizer)

**The backbone was chosen by the eval sweep, not by reputation.** Against the labeled set in
`tools/data/intent_eval.tsv` — whose negative half is dominated by *hard* negatives,
sentences that mention commerce without transacting — MiniLM's best formulation reached 5%
recall at zero false positives where e5 reached ~60% along one trained direction. Fine-tuning
the backbone is what moved it past that: the near-misses at the zero-FP boundary were all
negation, tense and quotation, which a pooled embedding cannot carry and a trained model can.

Three files come out, and all go beside the executable (they are loaded like `vision.onnx`
and are inert when missing):

    intent.onnx / intent_big.onnx   input_ids + attention_mask in; `score` [batch, 1] and
                                    `frames` [batch, K] out — the score already divided by
                                    the temperature, in the unit `trade.rs` scales by 1000
    intent_vocab.txt                the Unigram vocabulary, one `piece score` per line,
                                    id = line number
    intent_frames.txt               the frame names, one per line, id = line number

**The tokenizer is SentencePiece Unigram, not BPE, and the differences are the trap.** The
file in the checkpoint is named `sentencepiece.bpe.model` and is not BPE. There are no
merges; each whitespace-separated word gets `▁` prepended (Gemma's tower prepends nothing —
copying that rule across is exactly the drift `--check` exists to catch) and is segmented by
Viterbi over per-piece log scores. There is **no byte fallback**: a character outside the
vocabulary becomes `<unk>`, and adjacent unknowns fuse into one. The Rust side (`intent.rs`)
implements all of that; this file implements it a second time and `--check` compares both
against the reference tokenizer phrase by phrase, then prints the ids the Rust pin test
locks in.

**The normalizer is a precompiled charsmap, approximated as NFKC plus a short list.** The
reference tokenizer ships the map as an opaque blob. Measured against it (see `--check`):
NFKC, plus tab/newline/CR/FF, U+200B..U+200F (ZWSP, ZWNJ, ZWJ, LRM, RLM), U+2028, U+2029 and
U+FEFF each becoming a space, and VT/DEL simply removed. The one that matters most here is
**ZWNJ → space**: «می‌فروشم» reaches the model as «می فروشم», which is why neither the Rust
lexicon nor the training rows need half-space variants.

**The vocabulary is trimmed the way the vision tower's is, and the argument transfers.** A
Unigram piece can only enter the Viterbi lattice by matching a substring of the input, so for
input entirely in the kept scripts every reachable piece is itself entirely in the kept
scripts and survives the trim — the segmentation of Persian and English is bit-identical to
the full vocabulary's. What is lost is out-of-script text, which now hits `<unk>` instead of
its own pieces; `intent.rs` refuses to score a message that is mostly `<unk>` rather than
guess. The fine-tune froze the embedding table, so the gathered rows are the checkpoint's
own and the vocabulary file is byte-identical across every export.

**The embedding table is int8 and the matmuls are fp32 by default**, the same split as the
vision text tower and for the same reason: `quantize_dynamic` cannot touch a Gather, and the
table is most of the file. `--int8` additionally quantises the matmuls and prints the
per-phrase cosine so the decision is a number — measured on the embedding generation it cost
six recall points at zero false positives and was refused. Judge from the print.

    pip install torch transformers onnx onnxruntime numpy
    ../bin/python tools/finetune_intent.py --model small --out ./out
    ../bin/python tools/export_intent.py --finetuned out/small --out ./out --check
    ../bin/python tools/export_intent.py --finetuned out/base  --out ./out --check
"""

from __future__ import annotations

import argparse
import json
import math
import pathlib
import sys
import unicodedata

import torch

DIM = 384
TOKENS = 128

IDS_INPUT = "input_ids"
MASK_INPUT = "attention_mask"
SCORE_OUTPUT = "score"
FRAMES_OUTPUT = "frames"

# e5 was trained with this in front of every input, and an embedding computed without it is
# from a slightly different model. The Rust side prepends the same constant; the first CASES
# entry pins that the two agree about what the prefixed text tokenizes to.
PREFIX = "query: "

# Printed at the end so `intent::tests` can pin the Rust tokenizer against the reference one.
# Adversarial on purpose: ZWNJ, Persian digits, presentation forms, a card number, emoji, and
# one case the trimmed vocabulary cannot reach at all.
CASES = [
    "query: گوشی فروشی، پیام بدید",
    "سلام دنیا",
    "میفروشم گوشی ۲۰۰ تومن",
    "می\u200cفروشم",
    "کارت 6037-9918-1234-5670",
    "قیمت چنده؟",
    "for sale, only $25!",
    "\ufeb3\ufef4\ufeb3",  # Arabic presentation forms; NFKC folds them to the plain letters
    "Hello World!",
    "😀 چه روز خوبی",
    "中文测试",
]

# The same scripts the vision export keeps, for the same admins.
KEEP_RANGES = (
    (0x0021, 0x007E),   # ASCII, minus the space the pre-tokenizer has already split on
    (0x00A1, 0x024F),   # Latin-1 Supplement, Latin Extended-A and -B
    (0x0300, 0x036F),   # combining marks, which Latin Extended pieces are built from
    (0x0600, 0x06FF),   # Arabic — Persian lives here
    (0x0750, 0x077F),   # Arabic Supplement
    (0x08A0, 0x08FF),   # Arabic Extended-A
    (0x200C, 0x200F),   # unreachable after normalisation, kept for symmetry with vision
    (0x2010, 0x205E),   # General Punctuation, including « » and the Persian quotes
    (0x20A0, 0x20BF),   # currency signs
    (0x2190, 0x21FF),   # arrows
    (0x2600, 0x27BF),   # Miscellaneous Symbols and Dingbats
    (0xFB50, 0xFDFF),   # Arabic Presentation Forms-A
    (0xFE00, 0xFE0F),   # variation selectors, which emoji pieces carry
    (0xFE70, 0xFEFF),   # Arabic Presentation Forms-B
    (0x1F000, 0x1FAFF), # emoji
)

SPACE_MARK = "\u2581"

# SentencePiece's penalty for a character the vocabulary lacks: the worst real score, minus
# ten. The exact value only matters when it competes with real pieces, which it never wins.
UNK_PENALTY = 10.0


def keeps(piece: str) -> bool:
    if piece == SPACE_MARK:
        return True
    return all(
        c == SPACE_MARK or any(low <= ord(c) <= high for low, high in KEEP_RANGES)
        for c in piece
    )


# ---------------------------------------------------------------------------------------
# the tokenizer, reimplemented
# ---------------------------------------------------------------------------------------
#
# Same reason as the vision export: the Rust side has to implement this, and here is where
# that implementation is proved against the reference, case by case.

# Measured against the precompiled charsmap (see the module docstring): these become spaces.
TO_SPACE = "\t\n\r\x0c\u200b\u200c\u200d\u200e\u200f\u2028\u2029\ufeff"
# And these vanish.
TO_NOTHING = "\x0b\x7f"


def normalize(text: str) -> str:
    text = unicodedata.normalize("NFKC", text)
    return "".join(
        " " if c in TO_SPACE else c for c in text if c not in TO_NOTHING
    )


class Unigram:
    """XLM-R's SentencePiece Unigram, in the order `tokenizer.json` specifies it.

    * normalise, split on whitespace, prepend `▁` to every word — the opposite of Gemma's
      no-dummy-prefix rule, and the single likeliest thing to copy across wrongly;
    * per word, Viterbi: the segmentation whose summed piece scores is highest wins;
    * a character no piece covers costs `min_score - 10` and becomes `<unk>`; adjacent
      unknowns fuse into one token (`fuse_unk`, and `byte_fallback` is false).
    """

    def __init__(self, pieces: list[tuple[str, float]], unk: int):
        self.ids = {piece: at for at, (piece, _) in enumerate(pieces)}
        self.scores = [score for _, score in pieces]
        self.unk = unk
        self.longest = max(len(piece) for piece, _ in pieces)
        self.floor = min(self.scores) - UNK_PENALTY

    def word(self, word: str) -> list[int]:
        """One `▁`-prefixed word to ids, by Viterbi over the piece scores."""
        chars = list(word)
        n = len(chars)
        best = [-math.inf] * (n + 1)
        best[0] = 0.0
        # (start, id) of the piece ending at each position; id None marks the unk edge.
        back: list[tuple[int, int | None]] = [(0, None)] * (n + 1)
        for end in range(1, n + 1):
            for start in range(max(0, end - self.longest), end):
                if best[start] == -math.inf:
                    continue
                piece = "".join(chars[start:end])
                at = self.ids.get(piece)
                if at is not None:
                    score = best[start] + self.scores[at]
                    if score > best[end]:
                        best[end], back[end] = score, (start, at)
            # The unk edge: one character, priced below every real piece.
            if best[end - 1] != -math.inf and best[end - 1] + self.floor > best[end]:
                best[end], back[end] = best[end - 1] + self.floor, (end - 1, None)
        out: list[int] = []
        end = n
        while end > 0:
            start, at = back[end]
            out.append(self.unk if at is None else at)
            end = start
        out.reverse()
        fused: list[int] = []
        for at in out:
            if at == self.unk and fused and fused[-1] == self.unk:
                continue
            fused.append(at)
        return fused

    def body(self, text: str) -> list[int]:
        """The ids between the specials — what the Rust side computes before framing."""
        out: list[int] = []
        for word in normalize(text).split():
            out.extend(self.word(SPACE_MARK + word))
        return out

    def encode(self, text: str, bos: int, eos: int) -> list[int]:
        ids = self.body(text)[: TOKENS - 2]
        return [bos] + ids + [eos]


def spec_of(tokenizer) -> dict:
    return json.loads(tokenizer.backend_tokenizer.to_str())


def read_unigram(tokenizer) -> list[tuple[str, float]]:
    spec = spec_of(tokenizer)
    model = spec["model"]
    if model["type"] != "Unigram":
        sys.exit(f"the tokenizer is {model['type']}, not Unigram; the Rust side implements Unigram")
    if model.get("byte_fallback"):
        sys.exit("the tokenizer grew a byte fallback; the Rust side assumes there is none")
    pre = spec.get("pre_tokenizer") or {}
    kinds = [p.get("type") for p in pre.get("pretokenizers", [pre])]
    if "Metaspace" not in kinds:
        sys.exit(f"the pre-tokenizer is {kinds}, not Metaspace; the Rust side prepends ▁ per word")
    return [(piece, float(score)) for piece, score in model["vocab"]]


def trim(
    pieces: list[tuple[str, float]], specials: set[int], keep_all: bool
) -> tuple[list[tuple[str, float]], dict[int, int]]:
    """The pieces a group's message can reach, in original id order, and the id remapping.

    Original order so the embedding rows are gathered with one index list and the specials
    keep the low ids the graph was trained with.
    """
    keep = [
        (at, piece, score)
        for at, (piece, score) in enumerate(pieces)
        if keep_all or at in specials or keeps(piece)
    ]
    remap = {old: new for new, (old, _, _) in enumerate(keep)}
    return [(piece, score) for _, piece, score in keep], remap


# ---------------------------------------------------------------------------------------
# the graph
# ---------------------------------------------------------------------------------------


class Classifier(torch.nn.Module):
    """Token ids and their mask to the lock's score and the frame logits.

    Mean pooling over the real tokens — what sentence-transformers does for this model,
    reimplemented so the export does not need the dependency — then the fine-tuned head.
    Two outputs: `score` is `logsumexp(sell frames) − logsumexp(safe frames)` divided by
    the temperature `finetune_intent.py` chose, so it is already in the unit `trade.rs`
    multiplies by a thousand; `frames` are the raw logits, for the journal.

    The mask is a real input, unlike SigLIP's tower: this family does not attend over its
    padding, and the position ids are themselves computed from the mask inside the graph.
    """

    def __init__(self, model, head: torch.nn.Linear, sell: list[int], safe: list[int], temperature: float):
        super().__init__()
        self.model = model
        self.head = head
        self.register_buffer("sell", torch.tensor(sell))
        self.register_buffer("safe", torch.tensor(safe))
        self.temperature = temperature

    def forward(self, input_ids, attention_mask):
        hidden = self.model(input_ids=input_ids, attention_mask=attention_mask).last_hidden_state
        mask = attention_mask.unsqueeze(-1).to(hidden.dtype)
        pooled = (hidden * mask).sum(dim=1) / mask.sum(dim=1).clamp(min=1e-9)
        logits = self.head(pooled)
        score = torch.logsumexp(logits[:, self.sell], dim=1) - torch.logsumexp(logits[:, self.safe], dim=1)
        return (score / self.temperature).unsqueeze(1), logits


class Table(torch.nn.Module):
    """The token embedding as int8 rows with one scale each — the vision export's trick,
    verbatim, because `quantize_dynamic` cannot touch a Gather and the table is most of
    the file."""

    def __init__(self, weight: torch.Tensor):
        super().__init__()
        scale = weight.abs().amax(dim=1, keepdim=True).clamp(min=1e-8) / 127.0
        self.register_buffer("table", (weight / scale).round().clamp(-127, 127).to(torch.int8))
        self.register_buffer("scale", scale.to(torch.float32))

    def forward(self, ids):
        return self.table[ids].to(torch.float32) * self.scale[ids]


def export(module, path: pathlib.Path) -> None:
    ids = torch.ones(1, TOKENS, dtype=torch.long)
    mask = torch.ones(1, TOKENS, dtype=torch.long)
    torch.onnx.export(
        module,
        (ids, mask),
        str(path),
        input_names=[IDS_INPUT, MASK_INPUT],
        output_names=[SCORE_OUTPUT, FRAMES_OUTPUT],
        dynamic_axes={
            IDS_INPUT: {0: "batch"},
            MASK_INPUT: {0: "batch"},
            SCORE_OUTPUT: {0: "batch"},
            FRAMES_OUTPUT: {0: "batch"},
        },
        opset_version=17,
        do_constant_folding=True,
        # The TorchScript exporter, pinned, for the vision export's reason: the torch.export
        # path spills weights into a sibling `.onnx.data` and leaves value_info the quantiser
        # rejects.
        dynamo=False,
        external_data=False,
    )


def quantise(source: pathlib.Path, target: pathlib.Path) -> None:
    from onnxruntime.quantization import QuantType, quantize_dynamic

    quantize_dynamic(str(source), str(target), weight_type=QuantType.QInt8)
    source.unlink()


# ---------------------------------------------------------------------------------------
# checking
# ---------------------------------------------------------------------------------------


def cosine(a, b) -> float:
    import numpy as np

    return float(np.dot(a, b) / (np.linalg.norm(a) * np.linalg.norm(b) + 1e-12))


def run(session, ids: list[int]):
    import numpy as np

    mask = [1] * len(ids) + [0] * (TOKENS - len(ids))
    ids = ids + [1] * (TOKENS - len(ids))  # <pad> is id 1 and survives the trim in place
    score, frames = session.run(
        None,
        {
            IDS_INPUT: np.asarray([ids], dtype=np.int64),
            MASK_INPUT: np.asarray([mask], dtype=np.int64),
        },
    )
    return float(score[0][0]), frames[0]


def check(path, reference: Classifier, tokenizer, trimmed: Unigram, remap: dict[int, int], bos: int, eos: int) -> float:
    """The export and the reimplemented tokenizer against the library, on the same phrases.

    Two failures, reported differently, exactly as in the vision export: a tokenizer that
    disagrees prints both id lists, and a graph that drifted shows as a cosine below one
    between the exported frame logits and the torch module's. A case the trim cannot reach
    is reported and excluded rather than counted as drift.
    """
    import numpy as np
    import onnxruntime as ort

    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    worst = 1.0
    for case in CASES:
        mine = trimmed.encode(case, bos, eos)
        theirs = tokenizer(case)["input_ids"][:TOKENS]
        reachable = all(i in remap for i in theirs)
        if reachable and mine != [remap[i] for i in theirs]:
            print(
                f"  TOKENIZER MISMATCH {case!r}\n"
                f"    mine   {mine}\n    theirs {[remap[i] for i in theirs]}"
            )
        score, got = run(session, mine)
        with torch.no_grad():
            ids = torch.tensor([theirs + [1] * (TOKENS - len(theirs))])
            mask = torch.tensor([[1] * len(theirs) + [0] * (TOKENS - len(theirs))])
            want_score, want = reference(ids, mask)
            want = want[0].numpy()
        value = cosine(got, want)
        note = "" if reachable else "   (outside the trimmed vocabulary, <unk>)"
        print(f"  {case!r:36} cosine {value:.6f}  score {score:+.4f} (torch {float(want_score[0][0]):+.4f}){note}")
        if reachable:
            worst = min(worst, value)
    return worst


# ---------------------------------------------------------------------------------------
# the hand-written exemplars
# ---------------------------------------------------------------------------------------
#
# Two generations of scoring were built on these — nearest-exemplar cosines, then a ridge
# head fitted with them repeated — and both are retired: the fine-tuned classifier reads the
# frames these sets were standing in for. They remain as training rows `finetune_intent.py`
# repeats twice, because they are the registers a human vouched for, one incident at a time.
#
# SELL is what the lock deletes. SAFE is the hard-negative pole: many of its sentences
# *mention* commerce without being a transaction, because those are exactly the messages
# the lock must not delete.

SELL = [
    # offering goods, Persian
    "گوشی آیفون ۱۳ فروشی، ۲۰ میلیون، تماس بگیرید",
    "فروش ویژه لباس زنانه با قیمت مناسب، سفارش در خصوصی",
    "میفروشم پلی استیشن ۵ نو، قیمت توافقی",
    "لپ تاپ دست دوم فروشی در حد نو",
    "کفش ورزشی اورجینال موجود شد، ارسال به سراسر کشور",
    "عسل طبیعی فروش عمده و خرده، نمونه رایگان",
    "ماشین پراید مدل ۹۸ فروشی، تک برگ سند",
    "فروشنده انواع گوشی و لوازم جانبی هستم، لیست قیمت در کانال",
    "تیشرت با چاپ دلخواه فقط ۲۵۰ تومن، سفارش محدود",
    "پکیج آموزش زبان فروشی، نصف قیمت",
    # buying / seeking, Persian
    "خریدارم گوشی سالم تا ۱۰ میلیون، خصوصی پیام بدید",
    "خریدار ماشین نقدی هستم، فوری",
    "دنبال لپ تاپ دست دوم میگردم برای خرید",
    # services, ordering, payment, Persian
    "طراحی لوگو انجام میدم، قیمت از ۵۰۰ تومن، سفارش در پیوی",
    "ثبت سفارش فقط با واریز بیعانه، کارت به کارت",
    "شماره کارت 6037991812345670 به نام محمدی، بعد از واریز رسید بفرستید",
    "اکانت پرمیوم تلگرام موجوده، پرداخت با ترون یا کارت",
    "انجام پروژه دانشجویی با قیمت توافقی، پیام بدید",
    "اجاره سوئیت مبله روزانه، شبی ۸۰۰ تومن",
    "رهن و اجاره آپارتمان دو خوابه، بازدید با هماهنگی",
    "معاوضه گوشی سامسونگ با آیفون، پیام بدید",
    "حراج پایان فصل، همه چیز نصف قیمت، فقط امروز",
    "فروش شارژ و بسته اینترنت با تخفیف",
    "سفارش کیک تولد پذیرفته می شود، ارسال رایگان",
    "پیش فروش بلیط کنسرت، ظرفیت محدود",
    # English
    "iPhone 13 for sale, barely used, $450 or best offer",
    "selling my gaming PC, DM for price",
    "brand new sneakers available, worldwide shipping, order now",
    "I do logo design, prices start at $20, DM to order",
    "buying used laptops for cash, message me",
    "limited discount today only, order via private message",
    "USDT accepted, send payment and screenshot the receipt",
    "room for rent near the university, 200 per month, bills included",
    # services, rates and rentals — the shapes the first exemplar set under-covered, found by
    # the eval sweep's per-class bands
    "نصب و تعمیر انواع کولر گازی، هزینه توافقی",
    "خدمات برق کشی ساختمان، بازدید رایگان، اجرت منصفانه",
    "تدریس خصوصی فیزیک، جلسه ای ۳۰۰ تومن",
    "مشاوره تحصیلی و برنامه ریزی کنکور، تعرفه ماهانه",
    "ترجمه تخصصی متون، صفحه ای ۲۰ تومن، تحویل فوری",
    "برنج طارم اعلا، کیسه ای، ارسال با باربری، تضمین مرجوعی",
    "زعفران درجه یک، مثقالی، بسته بندی شیک، عمده و جزئی",
    "میوه و تره بار عمده، کیلویی، تحویل درب مغازه",
    "لوازم آرایشی اورجینال، دونه ای و کلی، لیست بگیرید",
    "کاشت ناخن و اکستنشن مژه، وقت آزاد داریم، قیمت مناسب",
    "دوخت سفارشی مانتو و شومیز، اجرت جزئی",
    "واگذاری امتیاز وام، شرایط عالی",
    "شارژ حساب پی پال و خرید از سایت های خارجی انجام میدم",
    "اجاره ویلا استخردار، شبی توافقی، رزرو با پیش پرداخت",
    "photography services for events, affordable packages, book now",
    "math tutoring online, first session free, flexible rates",
    # terse listings — the register a group seller actually types. The first probe was fitted
    # on long advertisements with prices and calls to action, and «گوشی فروشی» then scored
    # like ordinary chat (measured: −25, against a limit of +40). A listing is often two
    # words, so the pole has to hold two-word listings.
    "ماشین فروشی",
    "خونه فروشی",
    "گوشی سالم فروشی",
    "پراید صفر فروشی",
    "موتور فروشی فوری",
    "لپ تاپ فروشی در حد نو",
    "اکانت میفروشم",
    "تتر میفروشم",
    "سی پی میفروشم",
    "جم فری فایر میفروشم",
    "میو پوینت میفروشم",
    "الماس بازی میفروشم",
    "اکانت کلش میفروشم بیگ لول",
    "فالوور میفروشم",
    "ممبر کانال میفروشم",
    "نیترو میفروشم",
    "اشتراک اسپاتیفای میفروشم",
    "گیفت کارت میفروشم",
    "یوسی میزنم",
    "شارژ میزنم همه اپراتورها",
    "خریدارم گوشی",
    "ماشین خریدارم نقد",
    "اکانت خریدارم",
    "دلار خریدارم",
    "فروشی: دوچرخه بچگانه",
    "کفش فروشی سایز ۴۳",
    "کتاب کنکور فروشی نصف قیمت",
    "پلی استیشن فروشی با دو دسته",
    "selling my account",
    "car for sale",
    "selling cheap dm",
    # the exchange register — barter is a transaction too, and «طاق زدن» is its slang.
    # Game-currency trades carry no buy/sell word at all, which is how one shipped past
    # both the net and the exemplars.
    "سکه سلف طاق میزنم با میو پوینت",
    "طاق میزنم اکانتمو با یوسی",
    "جم طاق میزنم با سکه",
    "کی طاق میزنه؟ سکه دارم جم میخوام",
    "معاوضه میکنم گوشیمو با تبلت",
    "ماشینمو معاوضه میکنم با وانت",
    "تبادل سکه با جم انجام میدم",
    "تبادل ارز دیجیتال، نرخ توافقی",
    "ترید میکنم برات با درصد",
    "اسکین سی اس میدم، اسکین ولورانت بگیرم",
    "trading my skins for csgo knife",
    "swap my account for coins",
    # the soft-sell register — shop announcements and offers that name no verb of selling.
    # Every line here scored under the limit in the adversarial battery.
    "شرایط اقساطی هم داریم، پیش پرداخت کم",
    "موجودی محدوده، برا رزرو پیام بدید",
    "قیمت همکاری بدم بهت، پیام بده",
    "مزون لباس ما افتتاح شد، سفارش میپذیریم",
    "هر مدل گوشی بخوای دارم، لیست تو پیویه",
    "فقط امروز، ارسال رایگان برا همه سفارشا",
    "اکانت اسپاتیفای و یوتیوب پرمیوم موجوده",
    "نرخ تتر لحظه ای، معامله بالای ۱۰۰ دلار",
    "بلیط تئاتر شنبه دوتا دارم، نصف قیمت میدم",
    "کد تخفیف ۵۰ درصدی اسنپ فود دارم، تکی ۲۰",
    "ویپ و پاد سیستم اصل، لیست قیمت بگیرید",
    "جزوه های ارشد آماده ست، هر درس ۵۰",
    "ربات تلگرامی برات میسازم، نمونه کار دارم",
    "سایت وردپرسی میزنم برات، از ۲ تومن",
    "پیج اینستا ۱۰ کا فالوور واگذار میشه",
    "زمین کشاورزی دارم واگذار میکنم فوری",
    "امتیازمو میدم، فقط بیا سند بزن",
    "دوتا بلیط استادیوم دارم، کیا پایه ان؟ قیمت توافقی",
    # dealers, agents and home businesses — the professional registers
    "صرافی مجاز، حواله دلار و یورو، نرخ رقابتی",
    "طلافروشی ما اجرت پایین میگیره، تشریف بیارید",
    "املاک صداقت: چند واحد نوساز آماده فروش داریم",
    "مشاور املاکم، فایل رهن و اجاره بخواید موجوده",
    "ترشی و مربای خونگی کار میکنم، سفارش عید قبول میکنم",
    "کیف و کفش چرم دست دوز، کار خودمه، عکس بخوای میفرستم",
    "پیتزا و برگر خونگی، ارسال فقط محدوده شهرک",
    "تعمیر برد گوشی و آیسی تغذیه، نمونه کار تو پیج",
    "باطری ماشین درب منزل نصب میکنم، همه برندا",
    "پی تو پی تتر کار میکنم، احراز نمیخواد",
    "حواله وسترن یونیون میزنم، کارمزد کم",
    "دنبال خونه اجاره ای میگردم، دو خوابه، تا ۱۵ رهن",
    "چند میدی این ساعتو؟ بگو مال تو",
    "کی سکه میخواد؟ زیر قیمت بازار میدم",
    "سفارش گیرم برای شیرینی عید، لیست منو بخواید",
    "دراپ شیپینگ آموزش میدم، کارت اول رایگان",
]

SAFE = [
    # mentions commerce without being a transaction — the pole that carries the precision
    "دیروز یه گوشی نو خریدم، خیلی راضی ام",
    "بالاخره ماشینمو فروختم، راحت شدم",
    "قیمت دلار امروز چند شد؟",
    "این گوشی رو از کجا خریدی؟ چقدر خوبه",
    "میگن قیمت گوشی داره میاد پایین",
    "تو این وضعیت اقتصادی هیچی نمیشه خرید",
    "رفته بودم بازار، همه چی گرون شده",
    "دوستم مغازه لباس فروشی داره",
    "این بازی ارزش خریدن داره؟",
    "اگه جای من بودی کدوم لپ تاپ رو میخریدی؟",
    "خرید و فروش سهام امروز متوقف شد",  # news register
    "میخوام برم خرید عید، کی میاد؟",
    "پول جمع میکنم که یه دوچرخه بخرم",
    "چقدر بابت شارژ ساختمون دادی؟",
    "قیمت ها رو مقایسه کن بعد تصمیم بگیر",
    "این فیلم ارزش دیدن داره، وقتتو نمیفروشه",
    # ordinary chat
    "سلام بچه ها، خوبید؟",
    "امروز هوا خیلی قشنگه",
    "کسی جزوه جلسه قبل رو داره؟",
    "تولدت مبارک داداش، صد سال زنده باشی",
    "فردا ساعت چند کلاس داریم؟",
    "این عکس رو ببینید چقدر خنده داره",
    "دیشب بازی رو دیدید؟ چه گلی زد",
    "ممنون از راهنماییت، خیلی کمک کرد",
    "کسی هست تهران باشه؟",
    "چه خبر از بچه ها؟ کم پیدایید",
    # English
    "I finally bought a new phone yesterday, love it",
    "prices keep going up, this economy is crazy",
    "which laptop would you buy if you were me?",
    "did you watch the match last night?",
    "thanks everyone for the birthday wishes",
    "the store near my house closed down",
    "هزینه های زندگی خیلی بالا رفته این روزا",
    "کلاس زبانم شهریه شو زیاد کرد، دارم فکر میکنم ادامه بدم یا نه",
    "برا پایان نامه م استرس دارم، وقت کم آوردم",
    "دیروز ناخن هامو کاشتم، دستم اذیت میشه هنوز",
    "خونه تکونی عید مونده رو دستم، حوصله ندارم",
    "چقدر خرج عروسی بالاست، خدا به داد برسه",
    "the tutoring session yesterday actually helped a lot",
    "rent went up again, thinking of moving",
    # the short forms of NOT selling — past tense, questions, third parties — so the terse
    # listings above sharpen the register distinction instead of just the word «فروش»
    "ماشینمو فروختم",
    "گوشیمو فروختم بالاخره",
    "خونمونو فروختیم",
    "چی میفروشی؟",
    "مگه ماشینتو میفروشی؟",
    "اکانتتو فروختی آخرش؟",
    "کی ماشینشو میفروشه؟",
    "چند خریدی اینو؟",
    "از کجا خریدی؟",
    "داداشم ماشینشو فروخت",
    "همسایمون خونشو فروشی گذاشته",
    "میگن یارو اکانتشو فروخته رفته",
    "قیمتش چنده الان؟",
    "فروشگاه بسته بود",
    "sold my car last week finally",
    "did you end up selling your account?",
    # the other meanings of «طاق» — the idiom and the arch — and exchange talk that is not
    # an offer, so the barter exemplars above cannot leak onto them
    "حوصلم دیگه طاق شده از این وضع",
    "صبرم طاق شد آخرش",
    "طاقت ندارم دیگه، خسته شدم",
    "طاق بستان کرمانشاه خیلی قشنگه",
    "سقف خونه های قدیمی طاقی بود",
    "معاوضه نمیکنم، گفتم که",
    "مگه دیوونه ام معاوضه کنم؟",
    "با کی طاق زدی این خرابه رو گرفتی؟",
    # the beg-and-borrow register — «سکه بده» is a friend asking, not a market, and it was
    # a production false positive at +38: nothing on the SAFE pole knew what a request
    # between members sounds like
    "سکه بده بازی کنیم دیگه",
    "داداش جم بده، جبران میکنم برات",
    "یه شارژ بده بیام آنلاین",
    "اکانتتو بده یه دست بزنم فقط",
    "پولمو بده دیگه، چقدر صبر کنم",
    "ده تومن قرض بده تا فردا میدم",
    "سکه هاتو بده به من مگه چیه",
    "کی سکه اضافه داره بده ما؟",
    "سکه میخوام، کی میده خدایی؟",
    "یکی جم بده دعاش میکنم",
    "بده ببینم گوشیتو یه لحظه",
    "رمز وای فای رو بده لطفا",
    "جزوه رو بده کپی بگیرم",
    "توپو بده بیا وسط بازی",
    # the adversarial battery's worst offenders, one of each class: economy news with
    # numbers, buying-advice questions, scam warnings, infrastructure complaints, football
    # transfers, game losses, jokes on «فروختن», and the religious practice nouns
    "حقوق کارگر ۱۲ تومنه، اجاره خونه ۲۰",
    "دلار رسید به ۱۲۰ تومن، باورتون میشه؟",
    "اجاره ها تو تهران از ۲۰ میلیون شروع میشه",
    "قبض برقمون ۲ تومن اومده، چه خبره؟",
    "کدوم سایت برای خرید گوشی مطمئنه؟",
    "این قیمت برای پراید ۹۵ منصفانه ست؟",
    "این کانالای فروش اکانت همش کلاهبرداریه",
    "دیدید طرف اومده بود سیگنال میفروخت؟ بلاکش کردم",
    "همه سکه هامو تو بازی حروم کردم",
    "شارژ ایرانسلم نصفه شبی پرید",
    "خدمات دولت الکترونیک باز قطعه",
    "ضمانت نامه بانکی برای وام لازمه؟",
    "درگاه بانک باز خطا میده موقع پرداخت قبض",
    "شماره کارتخوان مغازه خرابه گفت نقد بیار",
    "هزینه های بیمارستان کمرشکنه",
    "استقلال دنبال خرید مدافع خارجیه",
    "این بازیکنو باید همین الان بفروشن",
    "خودمو میفروشم به یه لیوان چای",
    "رفیقمون ما رو فروخت رفت با بقیه",
    "در روایات از کم فروشی نهی شده",
    "قانون گروه: خرید و فروش ممنوع",
    "اینجا جای تبلیغ و فروش نیستا، اخطار میدم",
    # appraisal questions, job talk, charity, lost-and-found — transaction-adjacent talk
    # that is not an offer and must stay
    "این ماشین الان چند می ارزه به نظرتون؟",
    "گوشیمو کارشناسی کنید، بفروشمش یا نه؟",
    "ساعت قدیمی بابابزرگمه، ارزش عتیقه داره؟",
    "برای استخدام برنامه نویس کجا آگهی بزنم؟",
    "شرکتشون داره نیرو میگیره، رزومه بفرستید",
    "برا سیل زده ها کمک جمع میکنیم، هر چقدر تونستید",
    "کیف پول پیدا شده تو پارک، صاحبش پیام بده",
    "سوییچ ماشین گم کردم تو محوطه، کسی ندیده؟",
    "نرخ بیکاری باز رفت بالا طبق آمار",
    "صرافی ها امروز بسته بودن، دلار قفل شد",
    "طلافروشی های بازار اعتصاب کردن",
    "املاکیه میگفت بازار خوابیده کسی نمیخره",
    "همش دارن تو تلویزیون پیتزا تبلیغ میکنن، هوس کردم",
    "باطری گوشیم زود خالی میشه، چیکارش کنم؟",
    "حواله حقوقم هنوز نیومده",
    "چند میدی مگه که انقدر ادعا داری؟",
]


def escape(piece: str) -> str:
    return piece.replace("\\", "\\\\").replace("\n", "\\n").replace("\r", "\\r").replace(" ", "\\s")


def write_vocab(path: pathlib.Path, pieces: list[tuple[str, float]]) -> None:
    # One `piece score` per line, the line number *is* the id. The piece is escaped, so the
    # space before the score is the first unescaped space on the line.
    body = "\n".join(f"{escape(piece)} {score!r}" for piece, score in pieces)
    path.write_text(body + "\n", encoding="utf-8")
    print(f"vocabulary: {len(pieces)} pieces to {path}")


def write_frames(path: pathlib.Path, frames: list[str]) -> None:
    """One frame name per line; the id is the line number, like the vocabulary."""
    path.write_text("\n".join(frames) + "\n", encoding="utf-8")


def report(*paths: pathlib.Path) -> None:
    print()
    for path in paths:
        print(f"{path} is {path.stat().st_size / (1024 * 1024):.0f} MB")
    print("copy them next to the groupbot executable on the server")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--finetuned",
        required=True,
        help="the checkpoint directory finetune_intent.py wrote (out/small or out/base)",
    )
    parser.add_argument("--out", default="./out")
    parser.add_argument(
        "--int8",
        action="store_true",
        help="also quantise the matmuls; judge from the printed cosines, not from size",
    )
    parser.add_argument(
        "--table",
        choices=("int8", "fp32"),
        default="int8",
        help="how the embedding table is stored; int8 is a quarter of the size",
    )
    parser.add_argument("--check", action="store_true", help="compare the export against the library")
    parser.add_argument(
        "--vocab",
        choices=("latin-arabic", "full"),
        default="latin-arabic",
        help="which vocabulary to keep",
    )
    args = parser.parse_args()

    import json

    from transformers import AutoModel, AutoTokenizer

    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    finetuned = pathlib.Path(args.finetuned)
    meta = json.loads((finetuned / "meta.json").read_text(encoding="utf-8"))
    frames: list[str] = meta["frames"]
    sell_ids = [frames.index(f) for f in meta["sell_frames"]]
    safe_ids = [at for at in range(len(frames)) if at not in sell_ids]

    # The tokenizer is the base checkpoint's — the fine-tune froze the table and changed
    # nothing about the pieces — so the vocabulary file is byte-identical across exports.
    tokenizer = AutoTokenizer.from_pretrained(meta["model"])
    pieces = read_unigram(tokenizer)
    bos, eos, unk, pad = (
        tokenizer.bos_token_id,
        tokenizer.eos_token_id,
        tokenizer.unk_token_id,
        tokenizer.pad_token_id,
    )
    specials = {i for i in (bos, eos, unk, pad, tokenizer.mask_token_id) if i is not None}
    kept, remap = trim(pieces, specials, args.vocab == "full")
    print(f"vocabulary: {len(kept)} of {len(pieces)} pieces")
    for name, old in (("bos", bos), ("eos", eos), ("unk", unk), ("pad", pad)):
        if remap[old] != old:
            sys.exit(f"{name} moved from id {old} to {remap[old]}; the Rust side reads fixed ids")

    trimmed = Unigram(kept, remap[unk])

    def load(where):
        model = AutoModel.from_pretrained(where).eval()
        head = torch.nn.Linear(model.config.hidden_size, len(frames))
        head.load_state_dict(torch.load(finetuned / "head.pt"))
        head.eval()
        return model, head

    model, head = load(finetuned)
    # 384 is the always-on model, 768 the escalation tier — the file names keep the two from
    # ever overwriting each other.
    if model.config.hidden_size not in (384, 768):
        sys.exit(f"{finetuned} embeds in {model.config.hidden_size} dimensions, not 384 or 768")
    big = model.config.hidden_size == 768

    # Gather the table down to the kept rows before the export, so the graph is small.
    table = model.embeddings.word_embeddings
    index = torch.tensor([old for old, _ in sorted(remap.items(), key=lambda kv: kv[1])])
    rows = table.weight.detach()[index].clone()
    model.embeddings.word_embeddings = (
        Table(rows) if args.table == "int8" else torch.nn.Embedding.from_pretrained(rows, freeze=True)
    )
    model.config.vocab_size = len(kept)

    intent_path = out / ("intent_big.onnx" if big else "intent.onnx")
    raw = intent_path.with_suffix(".raw.onnx") if args.int8 else intent_path
    print(f"exporting to {raw} (temperature {meta['temperature']:.4f}, {len(frames)} frames)")
    export(Classifier(model, head, sell_ids, safe_ids, meta["temperature"]), raw)
    if args.int8:
        print(f"quantising to {intent_path}")
        quantise(raw, intent_path)

    write_vocab(out / "intent_vocab.txt", kept)
    write_frames(out / "intent_frames.txt", frames)

    if args.check:
        print("checking the export and the tokenizer against the library")
        # `model` now holds the trimmed table, so the reference comes from a clean copy.
        reference_model, reference_head = load(finetuned)
        reference = Classifier(reference_model, reference_head, sell_ids, safe_ids, meta["temperature"]).eval()
        worst = check(intent_path, reference, tokenizer, trimmed, remap, bos, eos)
        if worst < 0.999:
            print(f"WARNING: the export drifted from the library, worst cosine {worst:.6f}")

        print("\nreference tokenizations, for pinning the Rust tokenizer:")
        for case in CASES:
            print(f"  {case!r:36} {trimmed.encode(case, bos, eos)}")

    report(intent_path, out / "intent_vocab.txt", out / "intent_frames.txt")


if __name__ == "__main__":
    main()
