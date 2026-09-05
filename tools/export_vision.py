#!/usr/bin/env python3
"""Exports the general vision-language model the picture features run on.

One backbone answers three questions in this bot — «قفل سیگار» and its five siblings, the
group's own «فیلتر تصویری», and (through `nsfw::arbiter`) whether the small NSFW classifier
is about to delete a family photo. It replaced CLIP ViT-B/32, and the reason is one number:
ViT-B/32 is 63.3% zero-shot on ImageNet and *this* is around 75-80%. The margins those
features threshold are all differences of CLIP-style cosines, so an encoder that separates
concepts better is the entire accuracy budget of every one of them at once.

    google/siglip2-base-patch16-384      the deployed default since 2026-09-05 (576 patches,
                                         2.5x the cost of 224, same 768-wide space and file
                                         size). Measured on the NSFW corpus with the fitted
                                         head: false deletions of ordinary drawings 49 -> 20
                                         at equal recall against patch16-224; large-256 and
                                         base-512 bought nothing more for 2-3x the cost.
    google/siglip2-base-patch16-224      the previous default, still a fine cheaper tower
    google/siglip2-base-patch32-256      smaller/faster fallback

    NONE of them is drop-in for another: a different checkpoint is a different space, so
    switching means regenerating the text tower AND tools/export_concepts.py AND
    tools/train_nsfw_head.py AND re-measuring nsfw's constants — all together.

Both are SigLIP 2, which matters three ways beyond the score:

* **It is multilingual by training, not by distillation.** The old text half was
  `clip-ViT-B-32-multilingual-v1`, a student taught to imitate an English tower. This one saw
  Persian during contrastive training, so «سیگار» is a direction the model learned rather
  than one a student approximated.
* **Its scores are calibrated.** SigLIP is trained with a sigmoid loss and ships the two
  numbers that turn a cosine into a probability, `logit_scale` and `logit_bias`. That is what
  lets `nsfw::arbiter` answer "how sure are you this is pornography" on an absolute scale
  instead of a margin that means something different for every phrase.
* **One space for everything.** The concepts, the group's filters and the NSFW second opinion
  are dot products against the same embedding, so a picture is still encoded exactly once.

    pip install torch transformers onnx onnxruntime pillow numpy
    python3 tools/export_vision.py --out ./out --int8 --check

Four files come out, and all four go beside the executable on the server:

    vision.onnx                the image tower
    vision_text.onnx           the text tower, for «فیلتر متنی»
    vision_text_vocab.txt      the BPE vocabulary the Rust tokenizer reads
    vision_text_merges.txt     its merge ranks, in order

Only `vision.onnx` is needed for the shipped concepts and for example-based filters; the
other three are what lets an admin *name* a filter in Persian.

**The text tower is trimmed, and that is not an optimisation.** Gemma's vocabulary is 256 000
pieces, and its embedding table alone is 256000x768 floats — 786 MB that `quantize_dynamic`
does not touch, because a table is a Gather and not a MatMul. Restricted to the scripts this
bot's admins actually type, the table is a fifth of that. The restriction is *lossless* for
input in those scripts: BPE only ever concatenates, so a piece reachable from Latin and
Arabic text is itself Latin and Arabic text and is therefore kept. Anything outside them
still encodes, through the byte-fallback pieces, which are all kept — it simply encodes the
way the model was taught to handle unseen script. `--vocab full` keeps everything.

**The table is stored as int8 and the matmuls are not, and that split is measured.** Quantising
the table costs nothing worth having — the direction of a phrase moves by a cosine of 0.9989 at
worst across Persian and English — and it is what takes the file from 950 MB to 490. Quantising
the *matmuls* on top takes it to 227 MB and costs far more than it looks: measured on the same
phrases, «سیگار» came back at cosine **0.915** from the unquantised tower, and a direction that
far off is a filter pointed somewhere else. `--int8` still offers it, for a worker that cannot
spare the memory, but it is not the default and the number above is why.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import sys

import torch


# The Rust side pins these. They are asserted rather than written to a metadata file: a file
# is a fourth thing to keep in step, and every one of these is a constant of the architecture.
DIM = 768
MAX_TOKENS = 64

VISION_INPUT = "pixels"
TEXT_INPUT = "input_ids"
OUTPUT_NAME = "embedding"

# Printed at the end so `vision::tests` can pin the Rust tokenizer against the reference one.
CASES = [
    "a photo of a cat",
    "خودرو لوکس",
    "سیگار",
    "cigarette",
    "Hello World!",
    "عکس سگ",
    "中文",
]

# The scripts an admin of a Persian-speaking group types, plus what any caption carries.
# A piece survives the trim when every character in it is in one of these.
KEEP_RANGES = (
    (0x0021, 0x007E),   # ASCII, minus the space that normalisation has already replaced
    (0x00A1, 0x024F),   # Latin-1 Supplement, Latin Extended-A and -B
    (0x0300, 0x036F),   # combining marks, which Latin Extended pieces are built from
    (0x0600, 0x06FF),   # Arabic — Persian lives here
    (0x0750, 0x077F),   # Arabic Supplement
    (0x08A0, 0x08FF),   # Arabic Extended-A
    (0x200C, 0x200F),   # ZWNJ first of all: Persian is unreadable without it
    (0x2010, 0x205E),   # General Punctuation, including « » and the Persian quotes
    (0x20A0, 0x20BF),   # currency signs
    (0x2190, 0x21FF),   # arrows
    (0x2600, 0x27BF),   # Miscellaneous Symbols and Dingbats
    (0xFB50, 0xFDFF),   # Arabic Presentation Forms-A
    (0xFE00, 0xFE0F),   # variation selectors, which emoji pieces carry
    (0xFE70, 0xFEFF),   # Arabic Presentation Forms-B
    (0x1F000, 0x1FAFF), # emoji
)

# SentencePiece's stand-in for a space. Every piece that starts a word carries it.
SPACE_MARK = "\u2581"


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
# Not for the export's sake — the library would do it — but because the Rust side has to, and
# this is where that implementation is proved. `--check` runs a phrase through both and
# compares the ids, so a disagreement is caught here rather than as a filter that quietly
# deletes the wrong pictures.


class Bpe:
    """Gemma's byte-fallback BPE, in the order `tokenizer.json` specifies it.

    The pipeline is short and every step is load-bearing:

    * the normaliser replaces every space with `▁`, so by the time the pre-tokeniser tries to
      split on spaces there are none — the whole phrase is one word;
    * `ignore_merges` is **false** here, so a phrase that happens to be a vocabulary entry is
      still built by merging — taking the whole-string shortcut agrees with the reference on
      most phrases and silently disagrees on the ones whose merge ranks reach a different
      split, which is the worst possible failure shape for a tokenizer;
    * a character the vocabulary does not have becomes its UTF-8 bytes as `<0xXX>` pieces;
    * merges are applied lowest rank first, all occurrences of the winning pair at a time.
    """

    def __init__(self, vocab: dict[str, int], merges: list[tuple[str, str]], eos: int, unk: int):
        self.vocab = vocab
        self.ranks = {pair: at for at, pair in enumerate(merges)}
        self.eos = eos
        self.unk = unk

    def pieces(self, text: str) -> list[str]:
        text = text.replace(" ", SPACE_MARK)
        out: list[str] = []
        for char in text:
            if char in self.vocab:
                out.append(char)
                continue
            for byte in char.encode("utf-8"):
                out.append(f"<0x{byte:02X}>")
        while len(out) > 1:
            best, at = None, None
            for index in range(len(out) - 1):
                rank = self.ranks.get((out[index], out[index + 1]))
                if rank is not None and (best is None or rank < best):
                    best, at = rank, index
            if at is None:
                break
            left, right = out[at], out[at + 1]
            merged, index = [], 0
            while index < len(out):
                if index < len(out) - 1 and out[index] == left and out[index + 1] == right:
                    merged.append(left + right)
                    index += 2
                    continue
                merged.append(out[index])
                index += 1
            out = merged
        return out

    def encode(self, text: str) -> list[int]:
        ids = [self.vocab.get(piece, self.unk) for piece in self.pieces(text)]
        ids = ids[: MAX_TOKENS - 1]
        ids.append(self.eos)
        return ids


def spec_of(tokenizer) -> dict:
    return json.loads(tokenizer.backend_tokenizer.to_str())


def read_bpe(tokenizer) -> Bpe:
    spec = spec_of(tokenizer)
    model = spec["model"]
    if model["type"] != "BPE":
        sys.exit(f"the tokenizer is {model['type']}, not BPE; the Rust side implements BPE")
    if not model.get("byte_fallback"):
        sys.exit("the tokenizer has no byte fallback; the Rust side assumes it")
    merges = [tuple(m) if isinstance(m, list) else tuple(m.split(" ")) for m in model["merges"]]
    return Bpe(dict(model["vocab"]), merges, tokenizer.eos_token_id, tokenizer.unk_token_id)


def trim(bpe: Bpe, keep_all: bool) -> tuple[list[str], list[tuple[str, str]], dict[int, int]]:
    """The vocabulary and merges an admin's phrase can actually reach, and the id remapping.

    Kept in the original id order so the embedding rows can be gathered with one index list,
    and so the specials — pad, eos, unk — keep the low ids the graph was trained with.
    """
    order = sorted(bpe.vocab.items(), key=lambda item: item[1])
    if keep_all:
        kept = [piece for piece, _ in order]
        remap = {old: new for new, (_, old) in enumerate(order)}
        return kept, list(bpe.ranks), remap

    keep = [
        (piece, old)
        for piece, old in order
        if keeps(piece) or piece.startswith("<0x") or piece.startswith("<") and piece.endswith(">")
    ]
    alive = {piece for piece, _ in keep}
    remap = {old: new for new, (_, old) in enumerate(keep)}
    merges = [
        pair
        for pair in bpe.ranks
        if pair[0] in alive and pair[1] in alive and (pair[0] + pair[1]) in alive
    ]
    return [piece for piece, _ in keep], merges, remap


# ---------------------------------------------------------------------------------------
# the graphs
# ---------------------------------------------------------------------------------------


def pooled(out):
    """The pooled embedding, whichever shape the library hands back.

    `get_image_features` returned a tensor in transformers 4 and returns the whole output
    object in 5. Taking `[0]` of the wrong one is a graph that exports and checks out
    structurally while emitting 64 patch vectors instead of one embedding, which is exactly
    how this was first written.
    """
    return out.pooler_output if hasattr(out, "pooler_output") else out


class Vision(torch.nn.Module):
    """Pixels to an embedding. Unnormalised — the Rust side owns `unit()` for every vector it
    handles, and a graph that normalised would make one of them silently different."""

    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, pixels):
        return pooled(self.model.vision_model(pixel_values=pixels))


class Text(torch.nn.Module):
    """Token ids to an embedding, with no attention mask.

    SigLIP is trained with every sequence padded to exactly `MAX_TOKENS` and attends over the
    padding, which is why its own tokenizer reports `input_ids` alone. Passing a mask here
    would be a different model from the one that produced the weights.
    """

    def __init__(self, model):
        super().__init__()
        self.model = model

    def forward(self, input_ids):
        return pooled(self.model.text_model(input_ids=input_ids))


def export(module, args, path, names, dynamic):
    torch.onnx.export(
        module,
        args,
        str(path),
        input_names=names,
        output_names=[OUTPUT_NAME],
        dynamic_axes=dynamic,
        opset_version=17,
        do_constant_folding=True,
        # The TorchScript exporter, pinned, for the reason `export_clip_text.py` records: the
        # torch.export path spills weights into a sibling `.onnx.data` the Rust side does not
        # know to ship, and leaves value_info that the quantiser rejects.
        dynamo=False,
        external_data=False,
    )


def quantise(source: pathlib.Path, target: pathlib.Path) -> None:
    from onnxruntime.quantization import QuantType, quantize_dynamic

    quantize_dynamic(str(source), str(target), weight_type=QuantType.QInt8)
    source.unlink()


class Table(torch.nn.Module):
    """The token embedding as int8 rows with one scale each.

    `quantize_dynamic` does not touch this table, because a lookup is a Gather and not a
    MatMul — so a tower quantised the ordinary way is still 600 MB of fp32 vocabulary with a
    90 MB model attached. Quantising it here instead is what takes the file from 950 MB to
    about 250 MB, and it is nearly free in accuracy: the scale is per row, so the error is
    0.4% of a single embedding row and the tower is twelve layers of averaging after it. The
    `--check` cosine is what proves that rather than this paragraph.
    """

    def __init__(self, weight: torch.Tensor):
        super().__init__()
        scale = weight.abs().amax(dim=1, keepdim=True).clamp(min=1e-8) / 127.0
        self.register_buffer("table", (weight / scale).round().clamp(-127, 127).to(torch.int8))
        self.register_buffer("scale", scale.to(torch.float32))

    def forward(self, ids):
        return self.table[ids].to(torch.float32) * self.scale[ids]


# ---------------------------------------------------------------------------------------
# checking
# ---------------------------------------------------------------------------------------


def cosine(a, b) -> float:
    import numpy as np

    return float(np.dot(a, b) / (np.linalg.norm(a) * np.linalg.norm(b) + 1e-12))


def check_text(path, model, tokenizer, trimmed: Bpe, remap: dict[int, int]) -> float:
    """The export and the reimplemented tokenizer against the library, on the same phrases.

    Two things are being proved at once and they fail differently. A tokenizer that disagrees
    prints its ids, because a wrong id is not an error — it is a phrase the model reads as a
    different phrase. A tower that drifted shows up as a cosine below one.

    A case the trim cannot reach — Chinese, once the vocabulary is Latin and Arabic — is
    reported and excluded rather than counted as drift. It encodes through the byte pieces,
    which is a *different* and worse encoding of the same phrase, and that is the documented
    cost of the trim rather than a defect in it.
    """
    import numpy as np
    import onnxruntime as ort

    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    worst = 1.0
    for case in CASES:
        mine = trimmed.encode(case)
        theirs = tokenizer(case)["input_ids"]
        reachable = all(i in remap for i in theirs)
        if reachable and mine != [remap[i] for i in theirs]:
            print(f"  TOKENIZER MISMATCH {case!r}\n    mine   {mine}\n    theirs {[remap[i] for i in theirs]}")
        ids = mine + [0] * (MAX_TOKENS - len(mine))
        got = session.run(None, {TEXT_INPUT: np.asarray([ids], dtype=np.int64)})[0][0]
        with torch.no_grad():
            want = pooled(
                model.text_model(
                    input_ids=torch.tensor([theirs + [0] * (MAX_TOKENS - len(theirs))])
                )
            )[0].numpy()
        value = cosine(got, want)
        note = "" if reachable else "   (outside the trimmed vocabulary, byte fallback)"
        print(f"  {case!r:24} cosine {value:.6f}{note}")
        if reachable:
            worst = min(worst, value)
    return worst


def check_vision(path, model, side: int) -> float:
    import numpy as np
    import onnxruntime as ort

    session = ort.InferenceSession(str(path), providers=["CPUExecutionProvider"])
    torch.manual_seed(0)
    pixels = torch.rand(1, 3, side, side) * 2.0 - 1.0
    got = session.run(None, {VISION_INPUT: pixels.numpy()})[0][0]
    with torch.no_grad():
        want = pooled(model.vision_model(pixel_values=pixels))[0].numpy()
    value = cosine(got, want)
    print(f"  vision cosine {value:.6f}")
    return value


# ---------------------------------------------------------------------------------------


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--model", default="google/siglip2-base-patch16-384")
    parser.add_argument("--out", default="./out")
    parser.add_argument(
        "--int8",
        action="store_true",
        help="also quantise the text tower's matmuls: 227 MB instead of 490, at a cost",
    )
    parser.add_argument(
        "--table",
        choices=("int8", "fp32"),
        default="int8",
        help="how the text tower stores its vocabulary; int8 is a quarter of the size",
    )
    parser.add_argument("--check", action="store_true", help="compare the export against the library")
    parser.add_argument(
        "--vocab",
        choices=("latin-arabic", "full"),
        default="latin-arabic",
        help="which vocabulary the text tower keeps",
    )
    parser.add_argument("--no-text", action="store_true", help="image tower only")
    args = parser.parse_args()

    from transformers import AutoModel, AutoTokenizer

    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)

    model = AutoModel.from_pretrained(args.model).eval()
    side = model.config.vision_config.image_size
    if model.config.vision_config.hidden_size != DIM:
        sys.exit(
            f"{args.model} embeds in {model.config.vision_config.hidden_size} dimensions, "
            f"and the Rust side is built for {DIM}"
        )
    if model.config.text_config.max_position_embeddings != MAX_TOKENS:
        sys.exit(
            f"{args.model} takes {model.config.text_config.max_position_embeddings} tokens, "
            f"and the Rust side pads to {MAX_TOKENS}"
        )

    print(f"model {args.model}")
    print(f"  side {side}  dim {DIM}  tokens {MAX_TOKENS}")
    print(f"  logit_scale {float(model.logit_scale.exp()):.6f}  logit_bias {float(model.logit_bias):.6f}")

    # The image tower stays fp32 whatever `--int8` says, and that is deliberate. It runs on
    # every picture, so it is where quantisation would be worth most — and it is also the one
    # whose output every threshold in the bot is a difference of. A tenth of a percent moved
    # in a cosine is a tenth of a percent moved in a margin whose whole usable band is 0.05
    # wide. The text tower is quantised because it runs once, when an admin types.
    vision_path = out / "vision.onnx"
    print(f"exporting the image tower to {vision_path}")
    export(
        Vision(model),
        (torch.zeros(1, 3, side, side),),
        vision_path,
        [VISION_INPUT],
        {VISION_INPUT: {0: "batch"}, OUTPUT_NAME: {0: "batch"}},
    )
    if args.check:
        check_vision(vision_path, model, side)

    if args.no_text:
        report(vision_path)
        return

    tokenizer = AutoTokenizer.from_pretrained(args.model)
    bpe = read_bpe(tokenizer)
    kept, merges, remap = trim(bpe, args.vocab == "full")
    print(f"vocabulary: {len(kept)} of {len(bpe.vocab)} pieces, {len(merges)} of {len(bpe.ranks)} merges")

    # The trimmed BPE — the one the Rust side will run — built from the kept pieces and their
    # new ids. Everything below checks against *this*, not against the untrimmed one.
    trimmed = Bpe(
        {piece: at for at, piece in enumerate(kept)},
        merges,
        remap[bpe.eos],
        remap[bpe.unk],
    )

    # The embedding table is gathered down to the kept rows *before* the export, so the graph
    # itself is small, and then stored as int8 rows because nothing downstream would.
    # Everything else in the tower is untouched.
    table = model.text_model.embeddings.token_embedding
    index = torch.tensor([old for old, _ in sorted(remap.items(), key=lambda kv: kv[1])])
    rows = table.weight.detach()[index].clone()
    model.text_model.embeddings.token_embedding = (
        Table(rows) if args.table == "int8" else torch.nn.Embedding.from_pretrained(rows, freeze=True)
    )
    model.config.text_config.vocab_size = len(kept)

    text_path = out / "vision_text.onnx"
    raw = out / "vision_text.raw.onnx" if args.int8 else text_path
    print(f"exporting the text tower to {raw}")
    export(
        Text(model),
        (torch.zeros(1, MAX_TOKENS, dtype=torch.long),),
        raw,
        [TEXT_INPUT],
        {TEXT_INPUT: {0: "batch"}, OUTPUT_NAME: {0: "batch"}},
    )
    if args.int8:
        print(f"quantising to {text_path}")
        quantise(raw, text_path)

    write_vocab(out / "vision_text_vocab.txt", kept)
    write_merges(out / "vision_text_merges.txt", merges)

    if args.check:
        print("checking the text tower and the tokenizer against the library")
        # `model` now holds the trimmed table, so the reference has to come from a clean copy.
        reference = AutoModel.from_pretrained(args.model).eval()
        worst = check_text(text_path, reference, tokenizer, trimmed, remap)
        if worst < 0.999:
            print(f"WARNING: the export drifted from the library, worst cosine {worst:.6f}")

    print("\nreference tokenizations, for pinning the Rust tokenizer:")
    for case in CASES:
        print(f"  {case!r:24} {trimmed.encode(case)}")

    report(vision_path, text_path, out / "vision_text_vocab.txt", out / "vision_text_merges.txt")


def escape(piece: str) -> str:
    return piece.replace("\\", "\\\\").replace("\n", "\\n").replace("\r", "\\r").replace(" ", "\\s")


def write_vocab(path: pathlib.Path, pieces: list[str]) -> None:
    # One piece per line, the line number *is* the id — the same shape the WordPiece vocabulary
    # had. Escaped, because a BPE vocabulary legitimately contains newlines and spaces.
    path.write_text("\n".join(escape(piece) for piece in pieces) + "\n", encoding="utf-8")
    print(f"vocabulary: {len(pieces)} pieces to {path}")


def write_merges(path: pathlib.Path, merges: list[tuple[str, str]]) -> None:
    # `left right`, and the line number is the rank. Both halves escaped, so the single space
    # between them is unambiguous.
    body = "\n".join(f"{escape(left)} {escape(right)}" for left, right in merges)
    path.write_text(body + "\n", encoding="utf-8")
    print(f"merges: {len(merges)} to {path}")


def report(*paths: pathlib.Path) -> None:
    print()
    for path in paths:
        print(f"{path} is {path.stat().st_size / (1024 * 1024):.0f} MB")
    print("copy them next to the groupbot executable on the server")


if __name__ == "__main__":
    main()
