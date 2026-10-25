#!/usr/bin/env python3
"""Built-in face models for the people `assets/imgfilter_subjects.tsv` names: «قفل چهره خامنه ای» recognises him
without anyone supplying a photograph, and recognises him better than any reference photograph would.

    ../bin/python tools/facelock_people.py fetch      # Wikimedia Commons photographs of each person
    ../bin/python tools/facelock_people.py portraits  # each person's Wikidata portrait, the seed of their face
    ../bin/python tools/facelock_people.py embed      # faces, in each photograph's views, as SFace prints
    ../bin/python tools/facelock_people.py eval       # held-out numbers
    ../bin/python tools/facelock_people.py write      # assets/face_people.bin
    ../bin/python tools/facelock_people.py cohort     # assets/face_cohort.bin, for locks from photographs alone

A face lock matched a picture's faces against an admin's reference photographs: the nearest one within cosine
0.55, which three photographs of Khamenei met in 64/66/72% of his held-out photographs, 320-pixel thumbnails
and sticker cut-outs. A logistic model per person, fitted on every photograph of them and those two views of
it against everyone else's faces, with its cut where one face of somebody else in ten thousand clears it, and
gated on the faces it was fitted on (`GALLERY`), meets 93/91/93% and took none of 2 764 held-out faces of
others; over nine people, none of 25 922 — the other eight people's faces among them, however alike.

Photographs: Wikimedia Commons (free licences), the person's category and its subcategories one level down,
and the person's Wikidata portrait (P18). A category is not the person: Raisi's held his family, ministers
and a series of Khamenei at a lectern, and a set built from it taught the model "an Iranian official". The
person's face is the one within 0.5 of their portrait's, nearer their centre than anyone else's, at 0.50 or
more to it, and seen again in two of their other photographs (`dataset`); every face that is far from them
is a negative for them, and so is every other person's.
"""

from __future__ import annotations

import hashlib
import io
import json
import pathlib
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent.parent
TABLE = HERE / "assets/imgfilter_subjects.tsv"
ROOT = pathlib.Path.home() / ".cache/faces/people"
API = "https://commons.wikimedia.org/w/api.php"
AGENT = {"User-Agent": "groupbot-facelock/1.0 (offline asset builder)"}
MAX_SIDE = 960
CAP = 260
# A private person who died in custody is not a public figure to recognise by default; a group that means
# her can still lock her face from photographs.
SKIP = {"mahsa amini"}


def people() -> list[str]:
    out = []
    for line in TABLE.read_text(encoding="utf-8").splitlines():
        if line.startswith("person\t"):
            name = line.split("\t")[1]
            if name not in SKIP:
                out.append(name)
    return out


def patient(url: str) -> bytes:
    for attempt in range(6):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=AGENT), timeout=30) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code not in (429, 503):
                raise
            wait = int(e.headers.get("Retry-After") or 0) or 15 * (attempt + 1)
            print(f"  {e.code} from {url[:70]}, waiting {wait}s", flush=True)
            time.sleep(wait)
        except Exception as e:  # noqa: BLE001
            print(f"  {e} from {url[:70]}", flush=True)
            time.sleep(5 * (attempt + 1))
    raise RuntimeError(f"rate limited: {url}")


def query(params: dict) -> dict:
    return json.loads(patient(API + "?" + urllib.parse.urlencode({**params, "format": "json"})))


def category_of(name: str) -> str | None:
    """The person's Commons category: Wikidata's P373 for the item the English name finds."""
    found = json.loads(patient("https://www.wikidata.org/w/api.php?" + urllib.parse.urlencode(
        {"action": "wbsearchentities", "search": name, "language": "en", "type": "item", "limit": 5, "format": "json"})))
    for hit in found.get("search", []):
        claims = json.loads(patient("https://www.wikidata.org/w/api.php?" + urllib.parse.urlencode(
            {"action": "wbgetclaims", "entity": hit["id"], "property": "P373", "format": "json"})))
        for claim in claims.get("claims", {}).get("P373", []):
            value = claim.get("mainsnak", {}).get("datavalue", {}).get("value")
            if value:
                return "Category:" + value
    return None


def portrait_url(name: str) -> str | None:
    """The person's Wikidata image (P18): a photograph of them someone chose, which is what their face in their
    category is found by. The category alone is not enough: Raisi's is mostly Khamenei and Nasrallah's mostly
    other men, looked at."""
    found = json.loads(patient("https://www.wikidata.org/w/api.php?" + urllib.parse.urlencode(
        {"action": "wbsearchentities", "search": name, "language": "en", "type": "item", "limit": 5, "format": "json"})))
    for hit in found.get("search", []):
        claims = json.loads(patient("https://www.wikidata.org/w/api.php?" + urllib.parse.urlencode(
            {"action": "wbgetclaims", "entity": hit["id"], "property": "P18", "format": "json"})))
        for claim in claims.get("claims", {}).get("P18", []):
            value = claim.get("mainsnak", {}).get("datavalue", {}).get("value")
            if value:
                print(f"{name}: {hit['id']} {hit.get('description', '')!r}, {value}", flush=True)
                return ("https://commons.wikimedia.org/wiki/Special:FilePath/"
                        + urllib.parse.quote(value.replace(" ", "_")) + "?width=1280")
    return None


def portraits() -> None:
    from PIL import Image

    for name in people():
        out = ROOT / name.replace(" ", "_") / "portrait.jpeg"
        if out.exists():
            continue
        url = portrait_url(name)
        if not url:
            print(f"{name}: no portrait", flush=True)
            continue
        image = Image.open(io.BytesIO(patient(url)))
        image.load()
        out.parent.mkdir(parents=True, exist_ok=True)
        image.convert("RGB").save(out, "JPEG", quality=92)
        time.sleep(3)


def members(category: str, kind: str, enough: int) -> list[str]:
    out, cont = [], {}
    while True:
        r = query({"action": "query", "list": "categorymembers", "cmtitle": category, "cmtype": kind,
                   "cmlimit": 500, **cont})
        out += [m["title"] for m in r.get("query", {}).get("categorymembers", [])]
        if "continue" not in r or len(out) >= enough:
            return out
        cont = {"cmcontinue": r["continue"]["cmcontinue"]}


def thumb_urls(titles: list[str]) -> dict[str, str]:
    urls = {}
    for at in range(0, len(titles), 15):
        r = query({"action": "query", "titles": "|".join(titles[at:at + 15]), "prop": "imageinfo",
                   "iiprop": "url|mime", "iiurlwidth": MAX_SIDE})
        for page in r.get("query", {}).get("pages", {}).values():
            info = (page.get("imageinfo") or [{}])[0]
            if info.get("mime") == "image/jpeg" and info.get("thumburl"):
                urls[page["title"]] = info["thumburl"]
        time.sleep(0.3)
    return urls


def fetch() -> None:
    from PIL import Image

    for name in people():
        out_dir = ROOT / name.replace(" ", "_")
        out_dir.mkdir(parents=True, exist_ok=True)
        if len(list(out_dir.glob("*.jpg"))) >= CAP * 0.9:
            continue
        category = category_of(name)
        if not category:
            print(f"{name}: no Commons category", flush=True)
            continue
        titles = members(category, "file", CAP * 3)
        print(f"{name}: {category}, {len(titles)} files at the top", flush=True)
        for sub in members(category, "subcat", 60)[:60]:
            if len(titles) > CAP * 3:
                break
            titles += members(sub, "file", CAP * 3)
        titles = sorted(set(t for t in titles if t.lower().endswith((".jpg", ".jpeg"))),
                        key=lambda t: hashlib.sha1(t.encode()).hexdigest())[: CAP * 2]
        got = 0
        for title, url in thumb_urls(titles).items():
            if got >= CAP:
                break
            out = out_dir / (hashlib.sha1(title.encode()).hexdigest()[:16] + ".jpg")
            if out.exists():
                got += 1
                continue
            try:
                image = Image.open(io.BytesIO(patient(url)))
                image.load()
                image = image.convert("RGB")
                if min(image.size) >= 96:
                    image.save(out, "JPEG", quality=90)
                    got += 1
            except Exception as error:  # noqa: BLE001
                print(f"  skip {title}: {error}", flush=True)
            time.sleep(0.6)
        print(f"{name}: {got} photographs from {category}", flush=True)


# Faces of nobody in particular, in what groups send: the NSFW corpus's ordinary groups (people, faces,
# memes, stickers, cartoons, anime) and Commons pictures of Iranian people and of the officials and clerics
# beside these people. Every face in them is a negative for every person.
EVERYDAY = [pathlib.Path.home() / ".cache/nsfw-train/corpus" / g for g in
            ("people", "faces", "memes", "stickers", "tg_stickers", "cartoon_faces", "anime_faces", "safe", "screenshots")]
EVERYDAY += [pathlib.Path.home() / "imgfx/commons" / g for g in
             ("ir_people", "ir_religion", "neg_clerics", "neg_mesbah", "neg_montazeri", "neg_sistani")]
EMBEDDED = pathlib.Path.home() / ".cache/faces/people_prints.pkl"
# The owner of a portrait's prints in EMBEDDED: this, then the person's name.
PORTRAIT = "portrait:"
VIEWS = ("photo", "thumb320", "sticker320")
# The drawn view: the bot's own render of the picture's traced Lottie sticker, blurred as `facelock::drawn_prints`
# blurs it. Its faces are judged by a model of their own (`drawn_model`): fitted together with photographs they
# cost photographs recall (Khomeini 100% -> 75%) and were found in a fifth of held-out drawings.
DRAWN = "vector512"


def _pipeline():
    import export_faces

    return export_faces, export_faces.Pipeline()


def _views(path: str, around):
    """The picture as the bot sees it: the photograph (longest side 1280), the whole picture as a 320-pixel
    thumbnail, and a head-and-shoulders sticker cut out around the face `around` picks."""
    import numpy as np
    from PIL import Image, ImageDraw, ImageFilter

    im = Image.open(path).convert("RGB")
    if max(im.size) > 1280:
        s = 1280 / max(im.size)
        im = im.resize((round(im.width * s), round(im.height * s)), Image.Resampling.BICUBIC)

    def webp_grey(rgba):
        buf = io.BytesIO()
        rgba.save(buf, "WEBP", quality=80)
        rgba = Image.open(io.BytesIO(buf.getvalue())).convert("RGBA")
        grey = Image.new("RGBA", rgba.size, (128, 128, 128, 255))
        grey.alpha_composite(rgba)
        return np.asarray(grey.convert("RGB"))

    s = 320 / max(im.size)
    thumb = webp_grey(im.resize((max(1, round(im.width * s)), max(1, round(im.height * s))), Image.Resampling.LANCZOS).convert("RGBA"))
    sticker = None
    if around is not None:
        x, y, w, h = around
        cx, cy, size = x + w / 2, y + h * 0.9, 3.0 * h
        crop = im.crop((int(cx - size / 2), int(cy - size / 2), int(cx + size / 2), int(cy + size / 2))).resize((512, 512), Image.Resampling.BICUBIC)
        mask = Image.new("L", (512, 512), 0)
        draw = ImageDraw.Draw(mask)
        draw.ellipse((150, 40, 362, 330), fill=255)
        draw.ellipse((40, 300, 472, 760), fill=255)
        mask = mask.filter(ImageFilter.GaussianBlur(2))
        rgba = crop.convert("RGBA")
        rgba.putalpha(mask)
        ring = mask.filter(ImageFilter.MaxFilter(17))
        white = Image.new("RGBA", (512, 512), (255, 255, 255, 0))
        white.putalpha(ring)
        white.alpha_composite(rgba)
        sticker = webp_grey(white.resize((320, 320), Image.Resampling.LANCZOS))
    return np.asarray(im), thumb, sticker


def _vector(path: str, around):
    """The same sticker as a Lottie artist traces it, as the bot looks at it: flattened to ten colours, each
    colour's regions traced to polygons and filled anti-aliased one layer over another — the hairline seams where
    shapes meet are what a rendered Lottie sticker has — over the neutral grey at 512, then blurred by two pixels
    (`facelock::DRAWN_BLUR`) before faces are looked for. Needs OpenCV (`~/.cache/nsfw-train/venv-gpu`). A plain
    flat-colour version without the tracing found Khamenei in 60% of held-out drawings and 5 of 40 real Lottie
    renders."""
    import cv2
    import numpy as np
    from PIL import Image, ImageDraw

    im = Image.open(path).convert("RGB")
    if max(im.size) > 1280:
        s = 1280 / max(im.size)
        im = im.resize((round(im.width * s), round(im.height * s)), Image.Resampling.BICUBIC)
    x, y, w, h = around
    cx, cy, size = x + w / 2, y + h * 0.9, 3.0 * h
    crop = np.asarray(im.crop((int(cx - size / 2), int(cy - size / 2), int(cx + size / 2), int(cy + size / 2)))
                      .resize((512, 512), Image.Resampling.BICUBIC))
    mask = Image.new("L", (512, 512), 0)
    draw = ImageDraw.Draw(mask)
    draw.ellipse((150, 40, 362, 330), fill=255)
    draw.ellipse((40, 300, 472, 760), fill=255)
    inside = np.asarray(mask) > 127
    cv2.setRNGSeed(0)
    _, labels, centres = cv2.kmeans(crop[inside].astype(np.float32), 10, None,
                                    (cv2.TERM_CRITERIA_EPS + cv2.TERM_CRITERIA_MAX_ITER, 20, 1.0), 3, cv2.KMEANS_PP_CENTERS)
    plan = np.full(inside.shape, -1, np.int32)
    plan[inside] = labels.ravel()
    plan = cv2.medianBlur((plan + 1).astype(np.uint8), 5).astype(np.int32) - 1
    canvas = np.full((512, 512, 3), 128, np.uint8)
    layers = [(cv2.dilate(inside.astype(np.uint8), np.ones((17, 17), np.uint8)), (255, 255, 255))]
    layers += [((plan == k).astype(np.uint8), tuple(int(c) for c in centres[k]))
               for k in sorted(range(10), key=lambda k: -(plan == k).sum())]
    for region, colour in layers:
        contours, _ = cv2.findContours(region, cv2.RETR_EXTERNAL, cv2.CHAIN_APPROX_SIMPLE)
        polygons = [cv2.approxPolyDP(c, 1.0, True) for c in contours if cv2.contourArea(c) >= 25]
        polygons = [p for p in polygons if len(p) >= 3]
        if polygons:
            cv2.fillPoly(canvas, polygons, colour, lineType=cv2.LINE_AA)
    return cv2.GaussianBlur(canvas, (0, 0), 2)


_WORKER = {}


def _prints(rgb):
    """`facelock::prints`: faces with a shorter side of 24 px or more, largest first, six at most, as
    (box, print) pairs."""
    _, pipe = _WORKER["pipe"]
    found = [f for f in pipe.faces(rgb) if min(f[1][2], f[1][3]) >= 24]
    found.sort(key=lambda f: -(f[1][2] * f[1][3]))
    return [(f[1], pipe.embed(rgb, f[2])) for f in found[:6]]


def _embed_one(job):
    """Every face's print in each view; the sticker view is cut around the largest face."""
    import os

    if "pipe" not in _WORKER:
        os.environ.setdefault("OMP_NUM_THREADS", "2")
        sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
        _WORKER["pipe"] = _pipeline()
    import numpy as np

    owner, path = job
    try:
        photo, _, _ = _views(path, None)
        faces = _prints(photo)
        if not faces:
            return owner, path, None
        _, thumb, sticker = _views(path, faces[0][0])
        out = {"photo": np.array([p for _, p in faces], np.float32),
               "boxes": np.array([b for b, _ in faces], np.float32)}
        out["thumb320"] = np.array([p for _, p in _prints(thumb)], np.float32).reshape(-1, 128)
        out["sticker320"] = np.array([p for _, p in _prints(sticker)], np.float32).reshape(-1, 128)
        out["vector512"] = np.array([p for _, p in _prints(_vector(path, faces[0][0]))], np.float32).reshape(-1, 128)
        return owner, path, out
    except Exception:  # noqa: BLE001
        return owner, path, None


def _vector_one(job):
    """The drawn view alone, for a record embedded before it existed."""
    import os

    if "pipe" not in _WORKER:
        os.environ.setdefault("OMP_NUM_THREADS", "2")
        sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
        _WORKER["pipe"] = _pipeline()
    import numpy as np

    key, box = job
    try:
        return key, np.array([p for _, p in _prints(_vector(key[1], box))], np.float32).reshape(-1, 128)
    except Exception:  # noqa: BLE001
        return key, np.zeros((0, 128), np.float32)


# The bot's renders of every picture's traced sticker, one directory per drawing style (`render_lottie_stickers`,
# `lottie_trace.py`). One style found Khamenei in 15 of 40 held-out stickers of that style and 7 of 40 of another.
# Three (adding cartoon6 and poster8) found 15, 6, 14 and 1 of 40 in four styles, and cut the other drawn models
# to a sliver of their own faces (Khomeini 1 of 43, Rouhani 9 of 59): one style, and for the rest the general
# model's eye on the whole drawing (`facelock_drawn_vision.py`).
RENDERED = [pathlib.Path.home() / ".cache/faces" / name for name in ("train_tgs_png",)]


def _rendered_one(job):
    """The drawn view from the bot's own renders of a picture's traced Lottie sticker (five frames), already blurred
    by the bot as `facelock::drawn_prints` blurs them: blurred here by Pillow instead, a drawing of Khamenei's son
    scored over the cut in the bot that its twin under the cut in training did not."""
    import os

    if "pipe" not in _WORKER:
        os.environ.setdefault("OMP_NUM_THREADS", "2")
        sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))
        _WORKER["pipe"] = _pipeline()
    import numpy as np
    from PIL import Image

    key, styles = job
    prints, largest = [], []
    for frames in styles:
        for at, frame in enumerate(frames):
            found = [p for _, p in _prints(np.asarray(Image.open(frame).convert("RGB")))]
            prints += found
            if at == 0 and found:
                largest.append(found[0])
    return key, np.array(prints, np.float32).reshape(-1, 128), np.array(largest, np.float32).reshape(-1, 128)


def rendered() -> None:
    """Replaces the drawn view with the bot's own renders: traced Lottie stickers of every picture (the
    scratch `gen_train_tgs.py`), rendered by `facelock::tests::render_lottie_stickers` into `RENDERED`."""
    import hashlib
    import pickle
    from concurrent.futures import ProcessPoolExecutor

    done = pickle.load(open(EMBEDDED, "rb"))
    todo = []
    for key, out in done.items():
        if out is None:
            continue
        stem = hashlib.sha1(f"{key[0]}|{key[1]}".encode()).hexdigest()[:16]
        styles = [[str(root / f"{stem}_{at}.png") for at in range(5)] for root in RENDERED
                  if all((root / f"{stem}_{at}.png").exists() for at in range(5))]
        if styles:
            todo.append((key, styles))
    print(f"{len(todo)} rendered stickers", flush=True)
    with ProcessPoolExecutor(8) as pool:
        for key, prints, largest in pool.map(_rendered_one, todo, chunksize=8):
            done[key]["vector512"] = prints
            done[key]["vector512_largest"] = largest
    pickle.dump(done, open(EMBEDDED, "wb"))
    print("drew", len(todo), "from renders", flush=True)


def vectors() -> None:
    """Adds the drawn view to every record `embed` wrote without it."""
    import pickle
    from concurrent.futures import ProcessPoolExecutor

    done = pickle.load(open(EMBEDDED, "rb"))
    todo = [(key, out["boxes"][0]) for key, out in done.items() if out is not None and "vector512" not in out]
    print(f"{len(todo)} records to draw", flush=True)
    started = time.time()
    with ProcessPoolExecutor(8) as pool:
        for at, (key, prints) in enumerate(pool.map(_vector_one, todo, chunksize=8)):
            done[key]["vector512"] = prints
            if at % 1000 == 0:
                print(f"  {at}/{len(todo)} {time.time() - started:.0f}s", flush=True)
    pickle.dump(done, open(EMBEDDED, "wb"))
    print("drew", len(todo), flush=True)


def embed() -> None:
    import pickle
    from concurrent.futures import ProcessPoolExecutor

    jobs = [(name, str(p)) for name in people() for p in sorted((ROOT / name.replace(" ", "_")).glob("*.jpg"))]
    jobs += [(PORTRAIT + name, str(p)) for name in people() for p in [ROOT / name.replace(" ", "_") / "portrait.jpeg"]
             if p.exists()]
    for directory in EVERYDAY:
        jobs += [("", str(p)) for p in sorted(directory.glob("*.jpg"))[:2500]]
    done = pickle.load(open(EMBEDDED, "rb")) if EMBEDDED.exists() else {}
    todo = [j for j in jobs if (j[0], j[1]) not in done]
    print(f"{len(jobs)} pictures, {len(todo)} to embed", flush=True)
    started = time.time()
    with ProcessPoolExecutor(8) as pool:
        for at, (owner, path, out) in enumerate(pool.map(_embed_one, todo, chunksize=8)):
            done[(owner, path)] = out
            if at % 1000 == 0:
                print(f"  {at}/{len(todo)} {time.time() - started:.0f}s", flush=True)
                pickle.dump(done, open(EMBEDDED, "wb"))
    pickle.dump(done, open(EMBEDDED, "wb"))
    print(f"embedded {sum(v is not None for v in done.values())} pictures with faces", flush=True)


OUT = HERE / "assets/face_people.bin"
# One false match in this many faces of other people, measured on faces the cut was not set on.
TARGET = 1e-4
# A face the model takes must also be this near one of the person's own fitted faces (`facelock::GALLERY`).
# Held out over nine people: the model alone took 5 of 23 948 everyday faces and 1 of 1 974 faces of the other
# people for recall 95.5/92.9/95.2% (photo/thumb320/sticker320) — other white-turbaned clerics for Rouhani, a
# meme's face, looked at; gated at 0.50, none of either at the same recall. A stricter cut instead (+0.5 logit)
# took none but lost ten points; a looser one (TARGET 1e-3) gated the same way took ten times as many.
GALLERY = 0.50
# Above the cut the negatives set, in logits. At one in ten thousand of some six thousand faces the cut is their
# highest, and the highest of faces it has not seen was over it: Khamenei's model took his son's sticker at
# +0.02 and Jannati's took Khamenei at +0.04, the only two of 1 974 held-out faces of the other people. At
# +0.05 neither, for recall 96.1/93.8/95.2% → 95.5/92.9/95.2%. Chosen on those held-out faces, so it is a
# floor, not a measurement.
MARGIN = 0.05


def _split(path: str, buckets: int) -> int:
    return int(hashlib.sha1(path.encode()).hexdigest()[:8], 16) % buckets


# A photograph's face is the person's at this cosine to their centre or above. At 0.40 their sets held the people
# photographed beside them — for Raisi, his family, ministers and other clerics were most of it, looked at — and
# a model fitted on those learns "an Iranian official", which is the mistake it then makes.
ASSIGN = 0.50
# ...and the same face (cosine 0.5) must be in this many of their other photographs.
SUPPORT = 2


def dataset():
    """Per person: their own face's print in each view of each photograph, and the centre of those; and every
    face of nobody in particular. A person's face in their own photographs is the identity common to most of
    them: seeded by the face that recurs across the most photographs, then rounds of taking each photograph's
    face nearest the centre — and nearer it than every other person's. Khamenei is the largest face in many
    photographs of the officials beside him: without the rivals, Raisi's centre settled at cosine 0.84 to
    Khamenei's, and his gallery held Khamenei."""
    import pickle

    import numpy as np

    data = pickle.load(open(EMBEDDED, "rb"))
    persons, everyday, seeds = {}, [], {}
    for (owner, path), out in data.items():
        if out is None:
            continue
        if owner.startswith(PORTRAIT):
            seeds[owner[len(PORTRAIT):]] = out["photo"][0]
        elif owner:
            persons.setdefault(owner, []).append((path, out))
        else:
            everyday.append((path, out))
    # A person is seeded by their portrait's face, and the centre is the mean of the face in each photograph
    # within 0.5 of it: a category holds the person's family, ministers and funeral crowds too. Without the
    # portrait, the face recurring across most photographs was Khamenei's for Raisi's category.
    persons = {name: records for name, records in persons.items() if len(records) >= 15 and name in seeds}
    centres = {}
    for name, records in persons.items():
        alike = [out["photo"][int(np.argmax(out["photo"] @ seeds[name]))] for _, out in records
                 if float((out["photo"] @ seeds[name]).max()) >= 0.5]
        centre = np.mean(alike or [seeds[name]], 0)
        centres[name] = centre / np.linalg.norm(centre)
    for _ in range(8):
        picks = {}
        for name, records in persons.items():
            rivals = np.array([c for other, c in centres.items() if other != name])
            chosen = []
            for path, out in records:
                s = out["photo"] @ centres[name]
                s = np.where(s > (out["photo"] @ rivals.T).max(1), s, -1.0)
                at = int(s.argmax())
                chosen.append(at if s[at] >= ASSIGN else None)
            picks[name] = chosen
        for name, records in persons.items():
            centre = np.mean([out["photo"][at] for (_, out), at in zip(records, picks[name]) if at is not None], 0)
            centres[name] = centre / np.linalg.norm(centre)
    # And a face is the person's only if the same face is in SUPPORT of their other photographs: a minister
    # photographed beside them twice is near their centre, but not near their face in every other picture.
    for name, records in persons.items():
        chosen = picks[name]
        kept = [at for at in range(len(records)) if chosen[at] is not None]
        faces = np.array([records[at][1]["photo"][chosen[at]] for at in kept])
        support = ((faces @ faces.T) >= 0.5).sum(1) - 1
        for at, count in zip(kept, support):
            if count < SUPPORT:
                chosen[at] = None
    found = {}
    for name, records in persons.items():
        centre = centres[name]
        rows = []
        for (path, out), at in zip(records, picks[name]):
            if at is None:
                continue
            mine = out["photo"][at]
            views = {"photo": mine}
            for view in ("thumb320", "sticker320"):
                if view == "sticker320" and at != 0:
                    continue    # the sticker was cut around the largest face, which is somebody else
                if len(out[view]):
                    s = out[view] @ mine
                    if s.max() >= 0.35:
                        views[view] = out[view][int(s.argmax())]
            if at == 0 and len(out.get(DRAWN + "_largest", ())):
                views[DRAWN] = out[DRAWN + "_largest"]    # the largest face of each style's first frame
            others = [p for view in VIEWS for p in out[view] if float(p @ centre) < 0.45]
            rows.append((path, views, others))
        found[name] = (centre, rows)
    return found, everyday


def _logistic(X, y, l2=1e-2):
    import numpy as np
    from scipy.optimize import minimize

    X = np.asarray(X, np.float64)
    y = np.asarray(y, np.float64)
    w = np.where(y == 1, 0.5 / y.sum(), 0.5 / (1 - y).sum())

    def loss(t):
        z = X @ t[:-1] + t[-1]
        g = w * (1 / (1 + np.exp(-z)) - y)
        return (w * (np.logaddexp(0, z) - y * z)).sum() + 0.5 * l2 * t[:-1] @ t[:-1], np.concatenate([X.T @ g + l2 * t[:-1], [g.sum()]])

    return minimize(loss, np.zeros(X.shape[1] + 1), jac=True, method="L-BFGS-B").x


def models(evaluate: bool):
    """A model per person: their own faces, every view, against every other face — everyone else's photographs,
    the strangers beside them in their own, and everyday faces. With `evaluate`, fitted on three fifths of each
    person's photographs and two fifths of the rest, the cut set on one fifth of the rest, and measured on the
    held-out photographs and the last fifth."""
    import numpy as np

    found, everyday = dataset()
    pool = []  # (path, print) for every face of nobody in particular
    for path, out in everyday:
        pool += [(path, p) for view in VIEWS for p in out[view]]
    for name, (_, rows) in found.items():
        pool += [(path, p) for path, _, others in rows for p in others]
    built = {}
    for name, (centre, rows) in found.items():
        # A face of nobody in particular may be the person unlabelled, so one near their centre is left out; a face
        # `dataset` gave somebody else is somebody else however alike — Khamenei for his son, a fellow cleric for
        # Jannati — and those are the negatives that matter most. Filtering them too hid exactly those mistakes.
        negatives = [(path, p) for path, p in pool if float(p @ centre) < 0.45]
        for other, (_, other_rows) in found.items():
            if other != name:
                negatives += [(path, p) for path, views, _ in other_rows for v, p in views.items() if v in VIEWS]
        if evaluate:
            fit_rows = [r for r in rows if _split(r[0], 5) < 3]
            test_rows = [r for r in rows if _split(r[0], 5) >= 3]
            fit_neg = [p for path, p in negatives if _split(path, 5) < 2]
            cut_neg = np.array([p for path, p in negatives if _split(path, 5) in (2, 3)])
            test_neg = np.array([p for path, p in negatives if _split(path, 5) == 4])
        else:
            fit_rows, test_rows = rows, []
            fit_neg = [p for path, p in negatives if _split(path, 5) < 3]
            cut_neg = np.array([p for path, p in negatives if _split(path, 5) >= 3])
            test_neg = np.zeros((0, 128))
        positives = [p for _, views, _ in fit_rows for v, p in views.items() if v in VIEWS]
        if len(positives) < 20:
            continue
        weights, bias, cut, gallery, takes = _fit(positives, fit_neg, cut_neg, TARGET)
        report = None
        if evaluate:
            per_view = {}
            for view in VIEWS:
                got = takes([views[view] for _, views, _ in test_rows if view in views])
                per_view[view] = (got.mean() if len(got) else float("nan"), len(got))
            report = (per_view, int(takes(test_neg).sum()), len(test_neg))
        drawn = drawn_model(name, fit_rows, test_rows, found, everyday, evaluate)
        built[name] = (weights, bias, cut, gallery, report, drawn)
    return built


def _fit(positives, fit_neg, cut_neg, target):
    """A logistic direction, its cut where `target` of `cut_neg` clears it (+ MARGIN), its gallery as the bot reads
    it back (int8 with a scale, then unit length), and the bot's rule for a face: the cut and GALLERY both."""
    import numpy as np

    t = _logistic(list(positives) + list(fit_neg), [1] * len(positives) + [0] * len(fit_neg))
    weights, bias = t[:-1].astype(np.float32), float(t[-1])
    scores = np.sort(np.asarray(cut_neg) @ weights + bias)[::-1]
    cut = float(scores[min(int(len(scores) * target), len(scores) - 1)]) + MARGIN
    gallery = []
    for p in positives:
        q, scale = _quantize(p)
        back = q.astype(np.float32) * scale
        gallery.append((q, scale, back / np.linalg.norm(back)))
    near = np.array([g for _, _, g in gallery], np.float32)

    def takes(P):
        P = np.asarray(P, np.float32).reshape(-1, 128)
        return (P @ weights + bias >= cut) & ((P @ near.T).max(1) >= GALLERY)

    return weights, bias, cut, [(q, s) for q, s, _ in gallery], takes


# A drawn model needs this many drawn faces of the person to fit on; only Khamenei has them (158).
DRAWN_MIN = 40
# Drawn faces of other people are few (some six thousand), so this puts the cut above every one of the cut set's.
# At 1e-3 (the third highest) Khamenei's model took a drawing of his son through the bot's own renderer.
DRAWN_TARGET = 1e-4


def drawn_model(name, fit_rows, test_rows, found, everyday, evaluate):
    """A person's model of their face as a Lottie artist draws it: their face in the bot's renders of traced
    stickers of their photographs, against every drawn face of everybody else, its cut above every drawn sticker of
    somebody else in the cut set. Measured for Khamenei held out: 16% of single drawn faces and none of 651 drawn
    stickers of others; through the bot's own Lottie decoder, 15 of 40 stickers traced from held-out photographs
    and none of 82 of other people's."""
    import pickle

    import numpy as np

    if "data" not in _DRAWN_CACHE:
        _DRAWN_CACHE["data"] = pickle.load(open(EMBEDDED, "rb"))
    data = _DRAWN_CACHE["data"]
    positives = [p for _, views, _ in fit_rows if DRAWN in views for p in views[DRAWN]]
    if len(positives) < DRAWN_MIN:
        return None
    # Per sticker of somebody else, every face of its rendered frames: an animation is judged by its best frame,
    # so the cut is set on each sticker's best face, not on single faces. Per face it let a drawing of Khamenei's
    # son through twelve frames.
    stickers = [(path, out.get(DRAWN, ())) for path, out in everyday]
    for other, (_, other_rows) in found.items():
        if other != name:
            stickers += [(path, data[(other, path)].get(DRAWN, ())) for path, _, _ in other_rows]
    stickers = [(path, np.asarray(faces, np.float32).reshape(-1, 128)) for path, faces in stickers if len(faces)]
    if evaluate:
        fit_neg = [p for path, faces in stickers if _split(path, 5) < 2 for p in faces]
        cut_stickers = [faces for path, faces in stickers if _split(path, 5) in (2, 3)]
        test_stickers = [faces for path, faces in stickers if _split(path, 5) == 4]
    else:
        fit_neg = [p for path, faces in stickers if _split(path, 5) < 3 for p in faces]
        cut_stickers = [faces for path, faces in stickers if _split(path, 5) >= 3]
        test_stickers = []
    weights, bias, _, gallery, _ = _fit(positives, fit_neg, fit_neg[:1], DRAWN_TARGET)
    best = np.sort([float((faces @ weights + bias).max()) for faces in cut_stickers])[::-1]
    cut = float(best[min(int(len(best) * DRAWN_TARGET), len(best) - 1)]) + MARGIN
    near = np.array([(q.astype(np.float32) * s) / np.linalg.norm(q.astype(np.float32) * s) for q, s in gallery])

    def takes(P):
        P = np.asarray(P, np.float32).reshape(-1, 128)
        return (P @ weights + bias >= cut) & ((P @ near.T).max(1) >= GALLERY)

    report = None
    if evaluate:
        got = takes([p for _, views, _ in test_rows if DRAWN in views for p in views[DRAWN]])
        false = sum(bool(takes(faces).any()) for faces in test_stickers)
        report = (float(got.mean()) if len(got) else float("nan"), len(got), false, len(test_stickers))
    return weights, bias, cut, gallery, report


_DRAWN_CACHE = {}


def _quantize(v):
    """`imgfilter::quantize`."""
    import numpy as np

    v = np.asarray(v, np.float32)
    scale = np.float32(max(float(np.abs(v).max()), 1e-6)) / np.float32(127.0)
    return np.clip(np.round(v / scale), -127, 127).astype(np.int8), np.float32(scale)


def evaluate() -> None:
    built = models(evaluate=True)
    print(f"{'person':28} {'photo':>12} {'thumb320':>12} {'sticker320':>12}   false / held-out faces of others"
          f"   {'drawn':>12}   false / held-out drawn stickers of others")
    for name, (_, _, _, _, (per_view, false, n), drawn) in sorted(built.items()):
        cells = " ".join(f"{rate:6.0%} ({count:3d})" if count else f"{'-':>12}" for rate, count in per_view.values())
        tail = ""
        if drawn is not None:
            rate, count, drawn_false, drawn_n = drawn[4]
            tail = f"   {rate:6.0%} ({count:3d})   {drawn_false} / {drawn_n}"
        print(f"{name:28} {cells}   {false} / {n}{tail}", flush=True)


def write() -> None:
    """`FPM3`, u32 count, u32 width; per person a u16 name length, the table's English name, then the photograph
    model — f32 bias, f32 cut, `width` f32 weights, a u32 gallery size and per gallery face an f32 scale and `width`
    int8 values — then a u8 saying whether a drawn model follows in the same layout."""
    import struct

    def model(weights, bias, cut, gallery):
        blob = struct.pack("<ff", bias, cut) + weights.astype("<f4").tobytes() + struct.pack("<I", len(gallery))
        for q, scale in gallery:
            blob += struct.pack("<f", scale) + q.tobytes()
        return blob

    built = models(evaluate=False)
    out = bytearray(b"FPM3" + struct.pack("<II", len(built), 128))
    for name, (weights, bias, cut, gallery, _, drawn) in sorted(built.items()):
        key = name.encode("utf-8")
        out += struct.pack("<H", len(key)) + key + model(weights, bias, cut, gallery)
        out += struct.pack("<B", drawn is not None)
        if drawn is not None:
            out += model(*drawn[:4])
    OUT.write_bytes(bytes(out))
    print(f"wrote {OUT}: {len(built)} people, {sum(b[5] is not None for b in built.values())} with a drawn model, "
          f"{len(out)} bytes")


COHORT_OUT = HERE / "assets/face_cohort.bin"
# The ridge on the cohort's covariance, as a share of its mean variance. 0.1, 0.3 and 1 measured within a point of
# each other; 1 caught most.
LAMBDA = 1.0
# The z is where this share of the everyday faces and the other people's, over three-photograph locks on each
# built-in person, gets through: a tenth of the models' TARGET, because nothing here is fitted to the person.
Z_TARGET = 1e-5
# Three-photograph locks drawn per person for that: at one in a hundred thousand the z is a handful of the highest
# scores, and four draws put it anywhere from 4.4 to 4.7.
Z_TRIALS = 24


def cohort() -> None:
    """`assets/face_cohort.bin` for a lock on somebody with no built-in model, from an admin's photographs alone:
    the mean and covariance of faces of nobody in particular — and of the built-in people, the look-alikes that
    matter — so the bot can draw each lock's exemplar direction `w = (S + λ·tr(S)/128·I)⁻¹ (mean(references) − μ)`
    in closed form, and the z every lock's direction must reach.

    Measured as shipped (z 4.71) with each built-in person standing in for anybody (left out of the cohort),
    references from their fitted faces, statistics from splits 0-1, z set on splits 2-3 of everyday faces and the
    other people's, and recall on held-out photographs (photo/thumb320/sticker320) against 11 844 held-out faces
    of the other people and 143 688 everyday faces:

        references   nearest within 0.55 (before)               z AND nearest within 0.50
        1            43.8/40.3/44.3%, 1 of the people           42.0/39.1/40.3%, none
        3            62.6/60.1/64.0%, 11 of the people          65.6/63.5/66.4%, none
        8            86.0/81.5/87.3%, 18 and 3 everyday         87.1/83.6/86.3%, none

    Everyday faces were never the problem; the other officials and clerics were. A cohort of everyday faces
    alone drew directions that point at "a cleric", and the same rule on it took 1 to 13 of the people's faces —
    no better than the nearest reference. A logistic model fitted per lock did no better and needs a fit.

    `FCO1`, u32 width, f32 z, then μ, A = (S + λ·tr(S)/128·I)⁻¹ and B = A·S·A as f32, the two matrices row-major:
    `w = A d` and its spread over the cohort `sqrt(dᵀ B d)` for `d = mean(references) − μ`."""
    import struct

    import numpy as np

    found, everyday = dataset()
    ev = {s: [] for s in range(5)}
    for path, out in everyday:
        ev[_split(path, 5)] += [p for view in VIEWS for p in out[view]]
    for name, (_, rows) in found.items():
        for path, _, others in rows:
            ev[_split(path, 5)] += list(others)
    names = sorted(found)
    faces_of = {n: {s: [p for path, views, _ in found[n][1] if _split(path, 5) == s for v, p in views.items() if v in VIEWS]
                    for s in range(5)} for n in names}

    def statistics(faces):
        faces = np.array(faces, np.float64)
        s = np.cov(faces.T, bias=True)
        a = np.linalg.inv(s + LAMBDA * np.trace(s) / 128 * np.eye(128))
        return faces.mean(0), a, a @ s @ a, len(faces)

    def everyone_but(name, splits):
        return [p for n in names if n != name for s in splits for p in faces_of[n][s]]

    cut_set = np.array(ev[2] + ev[3], np.float64)
    rng = np.random.default_rng(21)
    scores = []
    for name in names:
        centre, rows = found[name]
        fitted = [r for r in rows if _split(r[0], 5) < 3]
        if len(fitted) < 8:
            continue
        mu, a, b, _ = statistics(ev[0] + ev[1] + everyone_but(name, (0, 1)))
        negatives = np.concatenate([cut_set[(cut_set @ centre) < 0.45], np.array(everyone_but(name, (2, 3)))])
        for _ in range(Z_TRIALS):
            refs = np.array([fitted[at][1]["photo"] for at in rng.choice(len(fitted), 3, replace=False)])
            d = refs.mean(0) - mu
            w = a @ d / np.sqrt(d @ b @ d)
            scores.append(negatives @ w - w @ mu)
    scores = np.sort(np.concatenate(scores))[::-1]
    z = float(scores[int(len(scores) * Z_TARGET)])
    mu, a, b, count = statistics(ev[0] + ev[1] + everyone_but("", (0, 1)))
    out = b"FCO1" + struct.pack("<If", 128, z) + mu.astype("<f4").tobytes() + a.astype("<f4").tobytes() + b.astype("<f4").tobytes()
    COHORT_OUT.write_bytes(out)
    print(f"wrote {COHORT_OUT}: {count} faces, z {z:.3f} over {len(scores)} faces of others, {len(out)} bytes")


if __name__ == "__main__":
    {"fetch": fetch, "portraits": portraits, "embed": embed, "vectors": vectors, "rendered": rendered,
     "eval": evaluate, "write": write,
     "cohort": cohort}[sys.argv[1]]()
