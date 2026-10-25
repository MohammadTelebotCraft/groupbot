#!/usr/bin/env python3
"""The second look (`verify`): Qwen3-VL-2B-Instruct (Apache-2.0) asked «Does this image show <subject>? Answer yes or
no.» about a picture a general-model filter is about to delete, run by the bot's own ONNX Runtime on the CPU.

    ~/.cache/nsfw-train/venv-gpu/bin/python tools/export_verify.py vision --out ./out   # verify_vision.onnx
    ~/.cache/nsfw-train/venv-gpu/bin/python tools/export_verify.py text --out ./out     # verify_text.onnx
    ~/.cache/nsfw-train/venv-gpu/bin/python tools/export_verify.py prompts --out ./out  # verify_prompts.bin
    ~/.cache/nsfw-train/venv-gpu/bin/python tools/export_verify.py check --out ./out    # against transformers
    cp out/verify_vision.onnx* out/verify_text.onnx* out/verify_prompts.bin target/release/

One 384 x 384 picture (squashed, as `vision` squashes it; 144 image tokens), one prefill pass, no generation: the
answer is the first token, and P(yes) is the softmax of the yes tokens' logits against the no tokens'.

Each graph keeps its weights in a `.data` file beside it — five files in all, copied together.

- `verify_vision.onnx`: the vision tower for that one grid — pixel_values (576, 1536), the Qwen2-VL processor's
  patch layout, -> image_embeds (144, 2048) and the three DeepStack features (3, 144, 2048). Exported from
  transformers in fp32 (cosine 1.0000 to it), then every weight MatMul two 4-bit MatMulNBits summed — the weight
  and what 4 bits left of it — 501 MB, cosine 0.9993. One 4-bit layer (256 MB) reached 0.93 and turned borderline
  answers; dynamic int8 with per-tensor activations 0.87. The bot loads it without prepacking
  (`verify::model`): its weights stay mapped from the file, 0.3 GB less resident for a fifth more time.
- `verify_text.onnx`: the language model written op by op from the safetensors — one tensor in memory at a time,
  the whole fp32 model does not fit this machine's WSL — every Linear a 4-bit MatMulNBits (block 32, asymmetric).
  Inputs: inputs_embeds (1, L, 2048), cos and sin (1, L, 128) of the interleaved M-RoPE, an additive causal mask
  (1, 1, L, L), the DeepStack features (added to the image tokens after layers 0-2); output: the five yes and five
  no tokens' logits at the last position (1, 10). Two layers built in fp32 match transformers to 2e-5.
- `verify_prompts.bin`: every English a filter can ask about (both subject tables' visual rows, and the concept
  locks' questions), tokenized here once, with the token embeddings they use — the 151 936-row table is 600 MB,
  the rows these prompts touch are a few thousand. `VPR1`, u32 rows, u32 width; per row an f32 scale and `width`
  int8 values; u32 prefix length and that many row indices (the prompt before the image); u32 subjects; per
  subject a u16 key length, the key, a u16 length and that many u32 row indices (the prompt after the image).

Measured through the bot itself (`verify::tests::answers_for_a_list`) on the pictures the shipped filters delete:
at P(yes) 0.7 the trained filters' 238 deletions of 9.9 million everyday checks keep 220 of 229 right ones and none
of the 9 wrong ones; words without training data, at 0.995, 6 of 10 right and none of 14 wrong; 95% of
trained-filter and 83% of learned-filter catches of verified pictures pass. ONNX Runtime answers the same at 1, 4
and 8 threads and differently at 6 (a borderline 0.45 became 0.30): measure at the thread count that ships.
"""

from __future__ import annotations

import argparse
import glob
import json
import os
import pathlib
import re
import struct
import sys

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent.parent
NAME = "Qwen/Qwen3-VL-2B-Instruct"
SIDE = 384
GRID = SIDE // 16
PATCHES = GRID * GRID
IMAGE_TOKENS = PATCHES // 4
QUESTION = "Does this image show {}? Answer yes or no."
H, KV, D, HID, INTER, LAYERS, EPS = 16, 8, 128, 2048, 6144, 28, 1e-6
BLOCK = 32
# The concept locks' questions (`concepts::CONCEPTS`, by key), chosen on the GPU by what each kept of the heads'
# deletions on the concept corpus's evaluation split at YES (true catches, label-negatives — most of which show the
# thing, so not a count of mistakes): «a weapon such as a gun or a knife»
# 348/400, 38/183 where «a gun or a knife» kept 202 and 18 and «a weapon» 364 and 97; «tobacco» 252/269, 16/38;
# «an alcoholic drink» 369/400, 40/61; «gambling» 276/308, 20/47; «blood or gore» 34/39, 14/81. Drugs have no
# corpus and keep the plain question.
CONCEPT_QUESTIONS = {
    "c_cig": "tobacco",
    "c_alc": "an alcoholic drink",
    "c_gun": "a weapon such as a gun or a knife",
    "c_bet": "gambling",
    "c_drg": "illegal drugs",
    "c_bld": "blood or gore",
}
# What the concept locks ask besides, to tell what they are for from what they must leave alone
# (`concepts::decide`): kept under their own text. A weapon is a gun, or a knife that is neither a sword nor
# cutlery, or a pocket knife, combat knife or dagger; gambling shows something gambling is done with (a casino or its
# tables, a betting site, a poker game); tobacco is not a no-smoking sign; blood is not a sign or a poster
# unless someone in it is hurt.
CONTRAST_QUESTIONS = ["a gun", "a sword", "cutlery or a kitchen knife", "a pocket knife, a combat knife or a dagger",
                      "a casino, poker chips, playing cards, a roulette wheel, a slot machine or a betting website",
                      "sports betting", "a poker game or a poker tournament", "a no-smoking sign",
                      "a sign or a poster", "a wound or an injured person"]


def safetensors_path() -> str:
    return glob.glob(os.path.expanduser(f"~/.cache/huggingface/hub/models--{NAME.replace('/', '--')}/snapshots/*/model.safetensors"))[0]


def thing(english: str) -> str:
    """The subject as the question names it: «compact disc (cd)» asks about «compact disc»."""
    return re.sub(r"\s*\(.*?\)", "", english).strip()


def q4(w):
    """(N, K) float -> MatMulNBits B (N, K/BLOCK, BLOCK/2) uint8, scales (N*K/BLOCK,), zero points (N*K/BLOCK/2,)."""
    n, k = w.shape
    b = w.reshape(n, k // BLOCK, BLOCK).astype(np.float32)
    lo, hi = np.minimum(b.min(-1), 0.0), np.maximum(b.max(-1), 0.0)
    scale = (hi - lo) / 15.0
    scale[scale == 0] = 1.0
    zp = np.clip(np.round(-lo / scale), 0, 15).astype(np.uint8)
    q = np.clip(np.round(b / scale[..., None]) + zp[..., None], 0, 15).astype(np.uint8)
    packed = (q[..., 0::2] | (q[..., 1::2] << 4)).astype(np.uint8)
    assert (k // BLOCK) % 2 == 0
    zpp = (zp[:, 0::2] | (zp[:, 1::2] << 4)).astype(np.uint8)
    return packed, scale.astype(np.float32).reshape(-1), zpp.reshape(-1)


def template(proc):
    msgs = [{"role": "user", "content": [{"type": "image"}, {"type": "text", "text": QUESTION}]}]
    return proc.apply_chat_template(msgs, tokenize=False, add_generation_prompt=True)


def answer_tokens(tok):
    yes = sorted({tok.encode(w, add_special_tokens=False)[0] for w in ["Yes", "yes", " Yes", " yes", "YES"]})
    no = sorted({tok.encode(w, add_special_tokens=False)[0] for w in ["No", "no", " No", " no", "NO"]})
    return yes, no


def vision(out: pathlib.Path) -> None:
    import onnx
    import torch
    from onnx import helper, numpy_helper
    from transformers import AutoModelForImageTextToText
    from transformers.vision_utils import get_vision_interpolation_indices_and_weights, get_vision_position_ids

    model = AutoModelForImageTextToText.from_pretrained(NAME, dtype=torch.bfloat16, attn_implementation="eager")
    visual = model.model.visual.float().eval()
    del model
    grid = torch.tensor([[1, GRID, GRID]])
    ii, iw = get_vision_interpolation_indices_and_weights(grid, num_grid_per_side=visual.num_grid_per_side,
                                                           mode=visual.interpolation_mode,
                                                           align_corners=visual.interpolation_align_corners,
                                                           spatial_merge_size=2)
    pos = get_vision_position_ids(grid, 2)

    class Tower(torch.nn.Module):
        def __init__(self):
            super().__init__()
            with torch.no_grad():
                self.register_buffer("pos_embeds", (visual.pos_embed(ii) * iw[:, :, None]).sum(1))
                rot = visual.rotary_pos_emb(pos).reshape(PATCHES, -1)
                emb = torch.cat((rot, rot), -1)
                self.register_buffer("cos", emb.cos())
                self.register_buffer("sin", emb.sin())
            self.cu = torch.tensor([0, PATCHES], dtype=torch.int32)
            self.visual = visual

        def forward(self, px):
            h = self.visual.patch_embed(px) + self.pos_embeds
            feats = []
            for i, blk in enumerate(self.visual.blocks):
                h = blk(h, cu_seqlens=self.cu, position_embeddings=(self.cos, self.sin))
                if i in self.visual.deepstack_visual_indexes:
                    feats.append(self.visual.deepstack_merger_list[self.visual.deepstack_visual_indexes.index(i)](h))
            return self.visual.merger(h), torch.stack(feats)

    fp32 = out / "verify_vision.fp32.onnx"
    with torch.no_grad():
        torch.onnx.export(Tower().eval(), (torch.zeros(PATCHES, 1536),), str(fp32), input_names=["pixel_values"],
                          output_names=["image_embeds", "deepstack"], opset_version=17, dynamo=False)
    m = onnx.load(str(fp32))
    inits = {i.name: i for i in m.graph.initializer}
    nodes, added, n = [], [], 0
    for node in m.graph.node:
        weight, bias = None, None
        if node.op_type == "MatMul" and node.input[1] in inits:
            w = numpy_helper.to_array(inits[node.input[1]])
            if w.ndim == 2:
                weight = w.T
        elif node.op_type == "Gemm" and node.input[1] in inits:
            a = {x.name: helper.get_attribute_value(x) for x in node.attribute}
            if a.get("transA", 0) == 0 and a.get("alpha", 1.0) == 1.0 and a.get("beta", 1.0) == 1.0:
                w = numpy_helper.to_array(inits[node.input[1]])
                weight = w if a.get("transB", 0) == 1 else w.T
                bias = node.input[2] if len(node.input) > 2 else None
        if weight is None or weight.shape[1] % (2 * BLOCK):
            nodes.append(node)
            continue
        n += 1
        # Twice: the weight at 4 bits, then what 4 bits left of it, summed — about 8-bit precision from the only
        # width ONNX Runtime 1.20's MatMulNBits has. Once, the tower's features sat at cosine 0.93 to fp32 and the
        # weapon lock's «is it a sword» turned 0.76 into 0.02 on a butcher's knife; twice, 0.9993.
        parts, rest = [], weight.astype(np.float32)
        for level in range(2):
            b, s, z = q4(rest)
            rest = rest - _dequantized(b, s, z, *weight.shape)
            names = [f"q4_{n}_{level}_b", f"q4_{n}_{level}_s", f"q4_{n}_{level}_z"]
            added += [numpy_helper.from_array(arr, name) for name, arr in zip(names, (b, s, z))]
            parts.append(f"q4_{n}_{level}_y")
            nodes.append(helper.make_node("MatMulNBits", [node.input[0]] + names, [parts[-1]], domain="com.microsoft",
                                          K=weight.shape[1], N=weight.shape[0], bits=4, block_size=BLOCK,
                                          accuracy_level=0))
        target = node.output[0] if bias is None else f"q4_{n}_y"
        nodes.append(helper.make_node("Add", parts, [target]))
        if bias is not None:
            nodes.append(helper.make_node("Add", [target, bias], [node.output[0]]))
    used = {i for node in nodes for i in node.input}
    keep = [i for i in m.graph.initializer if i.name in used]
    del m.graph.node[:]
    m.graph.node.extend(nodes)
    del m.graph.initializer[:]
    m.graph.initializer.extend(keep + added)
    if not any(o.domain == "com.microsoft" for o in m.opset_import):
        m.opset_import.append(helper.make_opsetid("com.microsoft", 1))
    _save(m, out / "verify_vision.onnx")
    fp32.unlink()
    print(f"verify_vision.onnx: {n} weights as two 4-bit layers, {(out / 'verify_vision.onnx.data').stat().st_size >> 20} MB")


def _dequantized(b, s, z, n, k):
    """What MatMulNBits multiplies by: `q4`'s packing undone."""
    q = np.zeros((n, k // BLOCK, BLOCK), np.float32)
    q[..., 0::2], q[..., 1::2] = b & 15, b >> 4
    zz = z.reshape(n, -1)
    zp = np.zeros((n, k // BLOCK), np.float32)
    zp[:, 0::2], zp[:, 1::2] = zz & 15, zz >> 4
    return ((q - zp[..., None]) * s.reshape(n, -1)[..., None]).reshape(n, k)


def _save(model, path: pathlib.Path) -> None:
    """The graph with its weights in `<name>.data` beside it, which ONNX Runtime maps instead of reading in: loaded
    from one file the language model peaked at 2.2 GB resident, from two at 1.5 GB, and loaded in half the time."""
    import onnx

    data = path.with_name(path.name + ".data")
    if data.exists():
        data.unlink()
    onnx.save(model, str(path), save_as_external_data=True, all_tensors_to_one_file=True, location=data.name,
              size_threshold=1024)


class _Graph:
    def __init__(self):
        self.nodes, self.inits, self.n = [], [], 0

    def name(self, base):
        self.n += 1
        return f"{base}_{self.n}"

    def const(self, value, base="c", dtype=np.float32):
        from onnx import numpy_helper

        name = self.name(base)
        self.inits.append(numpy_helper.from_array(np.asarray(value, dtype=dtype), name))
        return name

    def op(self, kind, inputs, domain=None, **attrs):
        return self.ops(kind, inputs, 1, domain, **attrs)[0]

    def ops(self, kind, inputs, count, domain=None, **attrs):
        from onnx import helper

        outs = [self.name(kind.lower()) for _ in range(count)]
        self.nodes.append(helper.make_node(kind, inputs, outs, domain=domain, **attrs))
        return outs


def text(out: pathlib.Path, layers: int = LAYERS, fp32: bool = False) -> pathlib.Path:
    import onnx
    from onnx import TensorProto, helper
    from safetensors import safe_open
    from transformers import AutoProcessor

    proc = AutoProcessor.from_pretrained(NAME)
    prefix = len(proc.tokenizer(template(proc).split("<|image_pad|>")[0], add_special_tokens=False)["input_ids"])
    yes, no = answer_tokens(proc.tokenizer)
    g = _Graph()
    f = safe_open(safetensors_path(), "pt")

    def w(key):
        return f.get_tensor(key).float().numpy()

    def linear(x, key, n, k):
        weight = w(key)
        if fp32:
            return g.op("MatMul", [x, g.const(weight.T.copy(), "w")])
        b, s, z = q4(weight)
        return g.op("MatMulNBits", [x, g.const(b, "qb", np.uint8), g.const(s, "qs"), g.const(z, "qz", np.uint8)],
                    domain="com.microsoft", K=k, N=n, bits=4, block_size=BLOCK, accuracy_level=4)

    eps = g.const(np.float32(EPS), "eps")
    i64 = lambda v: g.const(np.array(v, np.int64), "i", np.int64)  # noqa: E731

    def rms(x, weight):
        m = g.op("ReduceMean", [g.op("Mul", [x, x])], axes=[-1], keepdims=1)
        return g.op("Mul", [g.op("Div", [x, g.op("Sqrt", [g.op("Add", [m, eps])])]), g.const(weight, "norm")])

    cos = g.op("Unsqueeze", ["cos", i64([1])])
    sin = g.op("Unsqueeze", ["sin", i64([1])])

    # Halves by Split and the image's rows by Pad, never by sibling Slices: ONNX Runtime 1.20's
    # GatherSliceToSplitFusion names every fusion it makes the same, and the graph then fails to load.
    def rope(x):
        x1, x2 = g.ops("Split", [x, i64([D // 2, D // 2])], 2, axis=3)
        rot = g.op("Concat", [g.op("Neg", [x2]), x1], axis=3)
        return g.op("Add", [g.op("Mul", [x, cos]), g.op("Mul", [rot, sin])])

    def heads(x, n):
        return g.op("Reshape", [x, i64([1, -1, n, D])])

    def repeat(t):
        t5 = g.op("Unsqueeze", [t, i64([2])])
        return g.op("Reshape", [g.op("Concat", [t5] * (H // KV), axis=2), i64([1, H, -1, D])])

    scale = g.const(np.float32(D ** -0.5), "scale")
    # The DeepStack features padded out to the sequence: zeros but for the image's tokens.
    length = g.op("Gather", [g.op("Shape", ["inputs_embeds"]), i64([1])], axis=0)
    after = g.op("Sub", [length, i64([prefix + IMAGE_TOKENS])])
    pads = g.op("Concat", [i64([0, prefix, 0, 0]), after, i64([0])], axis=0)
    x = "inputs_embeds"
    for i in range(layers):
        pre = f"model.language_model.layers.{i}."
        xn = rms(x, w(pre + "input_layernorm.weight"))
        q = rms(heads(linear(xn, pre + "self_attn.q_proj.weight", H * D, HID), H), w(pre + "self_attn.q_norm.weight"))
        k = rms(heads(linear(xn, pre + "self_attn.k_proj.weight", KV * D, HID), KV), w(pre + "self_attn.k_norm.weight"))
        v = heads(linear(xn, pre + "self_attn.v_proj.weight", KV * D, HID), KV)
        q = rope(g.op("Transpose", [q], perm=[0, 2, 1, 3]))
        k = repeat(rope(g.op("Transpose", [k], perm=[0, 2, 1, 3])))
        v = repeat(g.op("Transpose", [v], perm=[0, 2, 1, 3]))
        s = g.op("Add", [g.op("Mul", [g.op("MatMul", [q, g.op("Transpose", [k], perm=[0, 1, 3, 2])]), scale]), "mask"])
        o = g.op("MatMul", [g.op("Softmax", [s], axis=-1), v])
        o = g.op("Reshape", [g.op("Transpose", [o], perm=[0, 2, 1, 3]), i64([1, -1, H * D])])
        x = g.op("Add", [x, linear(o, pre + "self_attn.o_proj.weight", HID, H * D)])
        xn = rms(x, w(pre + "post_attention_layernorm.weight"))
        gate = linear(xn, pre + "mlp.gate_proj.weight", INTER, HID)
        up = linear(xn, pre + "mlp.up_proj.weight", INTER, HID)
        act = g.op("Mul", [g.op("Mul", [gate, g.op("Sigmoid", [gate])]), up])
        x = g.op("Add", [x, linear(act, pre + "mlp.down_proj.weight", HID, INTER)])
        if i < 3:
            ds = g.op("Unsqueeze", [g.op("Gather", ["deepstack", i64(i)], axis=0), i64([0])])
            x = g.op("Add", [x, g.op("Pad", [ds, pads])])
    if layers == LAYERS:
        xn = rms(x, w("model.language_model.norm.weight"))
        last = g.op("Gather", [xn, i64(-1)], axis=1)
        rows = f.get_tensor("model.language_model.embed_tokens.weight")[yes + no].float().numpy()
        result = g.op("MatMul", [last, g.const(rows.T.copy(), "answers")])
        outputs = [helper.make_tensor_value_info(result, TensorProto.FLOAT, [1, len(yes) + len(no)])]
    else:
        outputs = [helper.make_tensor_value_info(x, TensorProto.FLOAT, [1, "L", HID])]
    inputs = [helper.make_tensor_value_info("inputs_embeds", TensorProto.FLOAT, [1, "L", HID]),
              helper.make_tensor_value_info("cos", TensorProto.FLOAT, [1, "L", D]),
              helper.make_tensor_value_info("sin", TensorProto.FLOAT, [1, "L", D]),
              helper.make_tensor_value_info("mask", TensorProto.FLOAT, [1, 1, "L", "L"]),
              helper.make_tensor_value_info("deepstack", TensorProto.FLOAT, [3, IMAGE_TOKENS, HID])]
    model = helper.make_model(helper.make_graph(g.nodes, "verify_text", inputs, outputs, g.inits),
                              opset_imports=[helper.make_opsetid("", 17), helper.make_opsetid("com.microsoft", 1)],
                              producer_name="groupbot export_verify")
    model.ir_version = 9
    path = out / ("verify_text.onnx" if layers == LAYERS and not fp32 else f"verify_text_{layers}{'_fp32' if fp32 else ''}.onnx")
    _save(model, path)
    print(f"{path.name}: {layers} layers, {path.stat().st_size >> 20} MB")
    return path


def subjects() -> dict[str, str]:
    """Every key the bot may ask about, and the subject its question names."""
    out = {}
    for table in ("imgfilter_subjects.tsv", "imgfilter_lexicon.tsv"):
        for line in (HERE / "assets" / table).read_text(encoding="utf-8").splitlines():
            parts = line.split("\t")
            if len(parts) >= 2 and parts[0] == "visual":
                out[parts[1]] = thing(parts[1])
    for key, question in CONCEPT_QUESTIONS.items():
        out[key] = question
    for question in CONTRAST_QUESTIONS:
        out[question] = question
    return out


def prompts(out: pathlib.Path) -> None:
    from safetensors import safe_open
    from transformers import AutoProcessor

    proc = AutoProcessor.from_pretrained(NAME)
    tok = proc.tokenizer
    head, tail = template(proc).split("<|image_pad|>")
    prefix = tok(head, add_special_tokens=False)["input_ids"]
    table = subjects()
    after = {key: tok(tail.replace("{}", subject), add_special_tokens=False)["input_ids"] for key, subject in table.items()}
    used = sorted(set(prefix) | {t for ids in after.values() for t in ids})
    row = {t: at for at, t in enumerate(used)}
    emb = safe_open(safetensors_path(), "pt").get_tensor("model.language_model.embed_tokens.weight")
    blob = bytearray(b"VPR1" + struct.pack("<II", len(used), HID))
    for t in used:
        v = emb[t].float().numpy()
        s = max(float(np.abs(v).max()), 1e-8) / 127.0
        blob += struct.pack("<f", s) + np.clip(np.round(v / s), -127, 127).astype(np.int8).tobytes()
    blob += struct.pack("<I", len(prefix)) + b"".join(struct.pack("<I", row[t]) for t in prefix)
    blob += struct.pack("<I", len(after))
    for key, ids in sorted(after.items()):
        k = key.encode()
        blob += struct.pack("<H", len(k)) + k + struct.pack("<H", len(ids)) + b"".join(struct.pack("<I", row[t]) for t in ids)
    (out / "verify_prompts.bin").write_bytes(bytes(blob))
    print(f"verify_prompts.bin: {len(after)} subjects, {len(used)} token rows, {len(blob) >> 20} MB")


def check(out: pathlib.Path, items: list[tuple[str, str]]) -> None:
    """The exported pair on the CPU against transformers on the GPU, P(yes) per (picture, english)."""
    import onnxruntime as ort
    import torch
    from PIL import Image
    from transformers import AutoModelForImageTextToText, AutoProcessor

    proc = AutoProcessor.from_pretrained(NAME)
    tok = proc.tokenizer
    yes, no = answer_tokens(tok)
    tmpl = template(proc)
    head = tmpl.split("<|image_pad|>")[0]
    prefix = len(tok(head, add_special_tokens=False)["input_ids"])
    model = AutoModelForImageTextToText.from_pretrained(NAME, dtype=torch.bfloat16).cuda().eval()
    emb = model.model.language_model.embed_tokens.weight.float().cpu().numpy()
    vision_s = ort.InferenceSession(str(out / "verify_vision.onnx"), providers=["CPUExecutionProvider"])
    text_s = ort.InferenceSession(str(out / "verify_text.onnx"), providers=["CPUExecutionProvider"])
    diffs = []
    for path, english in items:
        px = patches(Image.open(path))
        ids = tok(tmpl.replace("{}", thing(english)).replace("<|image_pad|>", "<|image_pad|>" * IMAGE_TOKENS))["input_ids"]
        merged, ds = vision_s.run(None, {"pixel_values": px})
        x = emb[ids][None].copy()
        x[0, prefix:prefix + IMAGE_TOKENS] = merged
        cos, sin = rope(len(ids), prefix)
        lg = text_s.run(None, dict(inputs_embeds=x, cos=cos, sin=sin, mask=causal(len(ids)), deepstack=ds))[0][0]
        mine = float(1 / (1 + np.exp(-(np.logaddexp.reduce(lg[:5]) - np.logaddexp.reduce(lg[5:])))))
        t = torch.tensor([ids]).cuda()
        with torch.no_grad():
            ref = model(input_ids=t, attention_mask=torch.ones_like(t), mm_token_type_ids=(t == 151655).long(),
                        pixel_values=torch.from_numpy(px).cuda().to(model.dtype),
                        image_grid_thw=torch.tensor([[1, GRID, GRID]]).cuda()).logits[0, -1].float()
        theirs = float(torch.sigmoid(torch.logsumexp(ref[yes], 0) - torch.logsumexp(ref[no], 0)))
        diffs.append((mine, theirs))
    d = np.array(diffs)
    print(f"{len(d)} pictures: |P(yes) - transformers'| median {np.median(np.abs(d[:, 0] - d[:, 1])):.4f}, "
          f"same side of 0.7 {np.mean((d[:, 0] >= 0.7) == (d[:, 1] >= 0.7)):.3f}")


def patches(image):
    """`verify::patches`: squashed to SIDE (bilinear), x/127.5 - 1, the frame twice (Qwen's temporal patch), in the
    processor's merge-block order."""
    from PIL import Image

    im = image.convert("RGB").resize((SIDE, SIDE), Image.Resampling.BILINEAR)
    x = np.asarray(im, np.float32).transpose(2, 0, 1) / 127.5 - 1.0
    x = np.stack([x, x])
    x = x.reshape(1, 2, 3, GRID // 2, 2, 16, GRID // 2, 2, 16).transpose(0, 3, 6, 4, 7, 2, 1, 5, 8)
    return x.reshape(PATCHES, 3 * 2 * 16 * 16).astype(np.float32)


def rope(length, prefix):
    """`verify::rope`: text tokens at 0..prefix on all three axes, the image's 12 x 12 grid at (prefix, prefix + row,
    prefix + col), the rest from prefix + 12 on; interleaved M-RoPE (sections 24/20/20), theta 5e6."""
    g = GRID // 2
    t, h, w = list(range(prefix)), list(range(prefix)), list(range(prefix))
    for r in range(g):
        for c in range(g):
            t.append(prefix)
            h.append(prefix + r)
            w.append(prefix + c)
    nxt = prefix + g
    while len(t) < length:
        t.append(nxt)
        h.append(nxt)
        w.append(nxt)
        nxt += 1
    p = np.array([t, h, w], np.float64)
    inv = 1.0 / (5000000.0 ** (np.arange(0, D, 2) / D))
    f = p[:, :, None] * inv[None, None, :]
    out = f[0].copy()
    out[:, 1:60:3] = f[1][:, 1:60:3]
    out[:, 2:60:3] = f[2][:, 2:60:3]
    emb = np.concatenate([out, out], -1)
    return np.cos(emb)[None].astype(np.float32), np.sin(emb)[None].astype(np.float32)


def causal(length):
    return np.triu(np.full((length, length), -np.inf, np.float32), 1)[None, None]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("what", choices=["vision", "text", "prompts", "check"])
    parser.add_argument("--out", default="out")
    parser.add_argument("--items", help="a JSON list of {path, english} for check")
    args = parser.parse_args()
    out = pathlib.Path(args.out)
    out.mkdir(parents=True, exist_ok=True)
    if args.what == "vision":
        vision(out)
    elif args.what == "text":
        text(out)
    elif args.what == "prompts":
        prompts(out)
    else:
        items = [(i["path"], i["english"]) for i in json.load(open(args.items))]
        check(out, items)


if __name__ == "__main__":
    sys.exit(main())
