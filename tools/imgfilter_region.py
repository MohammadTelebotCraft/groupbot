#!/usr/bin/env python3
"""Pictures of the region a Persian group lives in, for «فیلتر تصویری» to be calibrated on.

    ~/.cache/nsfw-train/venv-gpu/bin/python tools/imgfilter_region.py fetch     # Wikimedia Commons
    ~/.cache/nsfw-train/venv-gpu/bin/python tools/imgfilter_region.py render    # Persian-script pictures
    ~/.cache/nsfw-train/venv-gpu/bin/python tools/imgfilter_region.py manifest
    ~/.cache/nsfw-train/venv-gpu/bin/python tools/vision_embed.py --manifest ~/.cache/concepts/region_manifest.tsv \
        --root ~/.cache --hf google/siglip2-base-patch16-384 --out ~/.cache/concepts/region_base384.npz --resume
    ../bin/python tools/imgfilter_region.py write     # assets/imgfilter_region.bin
    ../bin/python tools/imgfilter_bank.py             # the bank now includes these rows

The everyday bank every filter was calibrated on came from the NSFW corpus: stickers, memes and
screenshots in English, people and food photographed elsewhere. A Persian group's pictures are
Iranian people, streets, food, officials on the news, and screenshots and memes in Persian
script — and that is exactly where the old filters deleted. Measured on 2026-10-15, the five
live phrase filters flagged 42 of 630 held-out pictures from this set where the lexicon's English
with this set in the bank flags 1, and Persian-script pictures 17 against 2.

Three parts, all written to `~/.cache/concepts/region/`:

- `commons/<set>/` — Wikimedia Commons categories: everyday Iranian life, and public figures
  who appear in Iranian news (the clerics and officials a filter for one of them would otherwise
  take with it). Commons answers 429 to anything brisk; the fetcher waits and retries.
- `synth/chat|meme/` — Persian-script pictures rendered here: chat screenshots and captioned
  photographs, set in Vazirmatn (SIL OFL) with real shaping. They hold no real conversation.

`imgfilter_region.bin` is the same `IFB1` layout as the bank, under `IFR1`, and `imgfilter::
understood` reads it to ask whether a phrase's best matches are just «people of the region».
"""

from __future__ import annotations

import hashlib
import io
import json
import pathlib
import random
import struct
import sys
import time
import urllib.error
import urllib.parse
import urllib.request

import numpy as np

HERE = pathlib.Path(__file__).resolve().parent.parent
ROOT = pathlib.Path.home() / ".cache/concepts"
REGION = ROOT / "region"
OUT = HERE / "assets/imgfilter_region.bin"
AGENT = {"User-Agent": "groupbot-imgfilter-region/1.0 (offline calibration of a moderation model)"}
API = "https://commons.wikimedia.org/w/api.php"
MAX_SIDE = 800

# set -> (categories, depth, cap). Depth 2 only for the one public figure with a deep tree.
COMMONS = {
    "khamenei": (["Category:Ali Khamenei"], 2, 450),
    "khomeini": (["Category:Ruhollah Khomeini"], 1, 70),
    "raisi": (["Category:Ebrahim Raisi"], 1, 70),
    "rouhani": (["Category:Hassan Rouhani"], 1, 70),
    "ahmadinejad": (["Category:Mahmoud Ahmadinejad"], 1, 70),
    "rafsanjani": (["Category:Akbar Hashemi Rafsanjani"], 1, 60),
    "khatami": (["Category:Mohammad Khatami"], 1, 60),
    "nasrallah": (["Category:Hassan Nasrallah"], 1, 50),
    "soleimani": (["Category:Qasem Soleimani"], 1, 60),
    "pezeshkian": (["Category:Masoud Pezeshkian"], 1, 60),
    "mojtaba": (["Category:Mojtaba Khamenei"], 1, 40),
    "jannati": (["Category:Ahmad Jannati"], 1, 30),
    "mesbah": (["Category:Mohammad-Taqi Mesbah-Yazdi"], 1, 30),
    "montazeri": (["Category:Hussein-Ali Montazeri"], 1, 30),
    "larijani": (["Category:Ali Larijani"], 1, 40),
    "clerics": (["Category:Shia clerics", "Category:Ayatollahs"], 1, 80),
    "people": (["Category:People of Iran", "Category:Iranian women", "Category:Children of Iran",
                "Category:Men of Iran"], 1, 160),
    "streets": (["Category:Streets in Tehran", "Category:Street life in Iran", "Category:Bazaars in Iran"], 1, 120),
    "food": (["Category:Cuisine of Iran"], 1, 80),
    "religion": (["Category:Mosques in Tehran", "Category:Muharram in Iran", "Category:Friday prayer in Iran"], 1, 100),
    "misc": (["Category:Nowruz in Iran", "Category:Football in Iran", "Category:Tehran Metro",
              "Category:Universities in Iran"], 1, 120),
}


def patient(url: str) -> bytes:
    for attempt in range(8):
        try:
            with urllib.request.urlopen(urllib.request.Request(url, headers=AGENT), timeout=60) as r:
                return r.read()
        except urllib.error.HTTPError as e:
            if e.code not in (429, 503):
                raise
            time.sleep(15 * (attempt + 1))
        except Exception:  # noqa: BLE001
            time.sleep(5 * (attempt + 1))
    raise RuntimeError(f"rate limited: {url}")


def query(params: dict) -> dict:
    return json.loads(patient(API + "?" + urllib.parse.urlencode({**params, "format": "json"})))


def members(category: str, kind: str) -> list[str]:
    out, cont = [], {}
    while True:
        r = query({"action": "query", "list": "categorymembers", "cmtitle": category, "cmtype": kind,
                   "cmlimit": 500, **cont})
        out += [m["title"] for m in r.get("query", {}).get("categorymembers", [])]
        if "continue" not in r or len(out) > 3000:
            return out
        cont = {"cmcontinue": r["continue"]["cmcontinue"]}


def files_in(category: str, depth: int, seen: set[str]) -> list[str]:
    if category in seen:
        return []
    seen.add(category)
    files = members(category, "file")
    if depth > 0:
        for sub in members(category, "subcat"):
            files += files_in(sub, depth - 1, seen)
    return files


def thumb_urls(titles: list[str]) -> dict[str, str]:
    urls = {}
    # Long titles in Persian overflow a URL at fifty per query (414); fifteen fits.
    for at in range(0, len(titles), 15):
        r = query({"action": "query", "titles": "|".join(titles[at:at + 15]), "prop": "imageinfo",
                   "iiprop": "url|mime", "iiurlwidth": MAX_SIDE})
        for page in r.get("query", {}).get("pages", {}).values():
            info = (page.get("imageinfo") or [{}])[0]
            if info.get("mime") in ("image/jpeg", "image/png") and info.get("thumburl"):
                urls[page["title"]] = info["thumburl"]
        time.sleep(0.5)
    return urls


def fetch() -> None:
    from PIL import Image

    for name, (categories, depth, cap) in COMMONS.items():
        out_dir = REGION / "commons" / name
        out_dir.mkdir(parents=True, exist_ok=True)
        titles, seen = [], set()
        for category in categories:
            try:
                titles += files_in(category, depth, seen)
            except Exception as error:  # noqa: BLE001
                print(f"  {category}: {error}", flush=True)
        titles = sorted(set(titles), key=lambda t: hashlib.sha1(t.encode()).hexdigest())[:cap]
        got = 0
        for title, url in thumb_urls(titles).items():
            out = out_dir / (hashlib.sha1(title.encode()).hexdigest()[:16] + ".jpg")
            if out.exists():
                got += 1
                continue
            try:
                image = Image.open(io.BytesIO(patient(url)))
                image.load()
                image = image.convert("RGB")
                if min(image.size) >= 64:
                    image.save(out, "JPEG", quality=90)
                    got += 1
            except Exception as error:  # noqa: BLE001
                print(f"  skip {title}: {error}", flush=True)
            time.sleep(1.0)
        print(f"{name}: {got}", flush=True)


LINES = """سلام بچه ها امروز کسی کلاس میاد
فردا ساعت چند جلسه داریم
این عکس رو دیدین خیلی باحاله
قیمت دلار امروز دوباره رفت بالا
کسی میدونه این گوشی چنده
بازی دیشب رو دیدین چه گلی زد
لطفا تو گروه تبلیغ نکنید
مرسی از همه که تبریک گفتین
امروز هوا خیلی سرد شده
کی میاد بریم کوه آخر هفته
این آهنگ جدید رو گوش بدین
تولدت مبارک رفیق
یه سوال داشتم از ادمین
کسی فیلم خوب سراغ داره
امتحان فردا خیلی سخته
من فردا نمیتونم بیام
عکس های سفر شمال رو گذاشتم
نتیجه کنکور کی میاد
شب بخیر همگی
خبر جدید رو خوندین
قیمت طلا امروز چقدره
این لینک رو باز کنید
یکی کمکم کنه لطفا
دارم میرم خرید چیزی لازم ندارین
رستوران جدید باز شده خوبه
کتاب جدید رو خریدم
مدرسه ها تعطیل شد
کسی شارژر اضافه داره
ساعت کاری فروشگاه تغییر کرد
اینترنت خیلی کنده امروز
آزمون آنلاین ساعت ده شروع میشه
لیست خرید رو فرستادم
بیاین برای عید برنامه بریزیم
عکس بچگیم رو پیدا کردم
پیشنهاد غذا برای شام دارین
ممنون از راهنماییت
دیروز تو خیابون تصادف شده بود
همه چی آماده است برای مهمونی
کلاس زبان ثبت نام شروع شد
ببخشید دیر جواب دادم""".split("\n")


def render() -> None:
    """Chat screenshots and captioned photographs in Persian script. Vazirmatn is fetched into
    `~/.cache/concepts/fonts`; Pillow needs libraqm for the shaping (`features.check('raqm')`)."""
    from PIL import Image, ImageDraw, ImageFont, features

    if not features.check("raqm"):
        sys.exit("Pillow has no libraqm here, and unshaped Persian is not what groups send")
    fonts = ROOT / "fonts"
    fonts.mkdir(parents=True, exist_ok=True)
    for weight in ("Regular", "Bold"):
        path = fonts / f"Vazirmatn-{weight}.ttf"
        if not path.exists():
            path.write_bytes(patient("https://raw.githubusercontent.com/rastikerdar/vazirmatn/master/"
                                     f"fonts/ttf/Vazirmatn-{weight}.ttf"))
    regular, bold = str(fonts / "Vazirmatn-Regular.ttf"), str(fonts / "Vazirmatn-Bold.ttf")
    rng = random.Random(5)
    photos = sorted((REGION / "commons/people").glob("*.jpg")) + sorted((ROOT / "img/pool").glob("*.jpg"))[:400]
    for sub in ("chat", "meme"):
        (REGION / "synth" / sub).mkdir(parents=True, exist_ok=True)

    def chat() -> Image.Image:
        width, height = 720, rng.choice([900, 1100, 1280])
        background = rng.choice([(255, 255, 255), (239, 239, 244), (230, 240, 230), (24, 24, 32)])
        dark = sum(background) < 200
        image = Image.new("RGB", (width, height), background)
        draw = ImageDraw.Draw(image)
        font = ImageFont.truetype(regular, rng.choice([26, 30, 34]))
        y = 30
        while y < height - 120:
            lines = rng.sample(LINES, rng.randint(1, 3))
            bubble = (60, 60, 75) if dark else rng.choice([(255, 255, 255), (220, 248, 198), (210, 230, 255)])
            bubble_width = rng.randint(380, 660)
            bubble_height = len(lines) * 46 + 30
            x0 = rng.choice([20, width - bubble_width - 20])
            draw.rounded_rectangle([x0, y, x0 + bubble_width, y + bubble_height], 18, fill=bubble)
            line_y = y + 14
            for line in lines:
                draw.text((x0 + bubble_width - 20, line_y), line, font=font,
                          fill=(235, 235, 235) if dark else (20, 20, 20), anchor="ra", direction="rtl", language="fa")
                line_y += 46
            y += bubble_height + rng.randint(14, 40)
        return image

    def meme() -> Image.Image:
        photo = Image.open(rng.choice(photos)).convert("RGB")
        photo.thumbnail((720, 720))
        width, height = photo.size
        caption = rng.choice(LINES)
        font = ImageFont.truetype(bold, max(24, width // 16))
        top = rng.random() < 0.5
        if rng.random() < 0.5:
            image = Image.new("RGB", (width, height + 110), (255, 255, 255))
            image.paste(photo, (0, 110 if top else 0))
            ImageDraw.Draw(image).text((width // 2, 55 if top else height + 55), caption, font=font,
                                       fill=(0, 0, 0), anchor="mm", direction="rtl", language="fa")
        else:
            image = photo.copy()
            ImageDraw.Draw(image).text((width // 2, 50 if top else height - 50), caption, font=font,
                                       fill=(255, 255, 255), anchor="mm", direction="rtl", language="fa",
                                       stroke_width=3, stroke_fill=(0, 0, 0))
        return image

    for at in range(150):
        chat().save(REGION / "synth/chat" / f"{at:03d}.jpg", quality=88)
        meme().save(REGION / "synth/meme" / f"{at:03d}.jpg", quality=88)
    print("rendered 300")


def manifest() -> None:
    rows = ["path\tgroup\tsource\tsplit"]
    for path in sorted(REGION.rglob("*.jpg")):
        relative = path.relative_to(ROOT.parent)
        group = path.parent.name
        rows.append(f"{relative}\t{group}\tregion\ttrain")
    (ROOT / "region_manifest.tsv").write_text("\n".join(rows) + "\n", encoding="utf-8")
    print(f"{len(rows) - 1} rows")


def write() -> None:
    """The commons sets only: what `understood` asks about is people and places of the region.
    The rendered pictures go to the bank alone (`imgfilter_bank.py`)."""
    emb = np.load(ROOT / "region_base384.npz", allow_pickle=False)
    keep = np.char.find(emb["path"].astype(str), "/commons/") >= 0
    vectors = emb["embeddings"][keep].astype(np.float32)
    vectors /= np.linalg.norm(vectors, axis=1, keepdims=True)
    out = bytearray(b"IFR1" + struct.pack("<II", len(vectors), vectors.shape[1]))
    for vector in vectors:
        scale = float(np.abs(vector).max()) / 127.0
        out += struct.pack("<f", scale) + np.clip(np.round(vector / scale), -127, 127).astype(np.int8).tobytes()
    OUT.write_bytes(bytes(out))
    print(f"wrote {OUT}: {len(vectors)} pictures, {len(out) / 1e6:.1f} MB")


if __name__ == "__main__":
    {"fetch": fetch, "render": render, "manifest": manifest, "write": write}[sys.argv[1]]()
