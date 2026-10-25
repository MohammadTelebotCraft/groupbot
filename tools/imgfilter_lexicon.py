#!/usr/bin/env python3
"""Persian names for every Open Images class, as the `assets/imgfilter_lexicon.tsv` rows `imgsubject` reads
after its own table.

    ../bin/python tools/imgfilter_lexicon.py fetch   # Wikidata: Persian labels and aliases, by Freebase id
    ../bin/python tools/imgfilter_lexicon.py write   # assets/imgfilter_lexicon.tsv

`assets/imgfilter_subjects.tsv` is written by hand and says what a phrase *is* — a lock, a person, a game, a
thing the general model knows. It held 351 subjects, and a group that typed anything else got the text
tower's own reading of the Persian, which for most words is a weaker filter and for names and slang is none.
Open Images names twenty thousand classes, each with a Freebase id, and Wikidata maps Freebase ids (P646) to
items with Persian labels and aliases (CC0). So every class Wikidata can name in Persian becomes a row:
the Persian names, and the class's English name for the text tower.

A Persian name two classes share goes to the one Open Images shows most often, counted on its human-verified
train labels. A name the hand-written table already has is left to the table, which `imgsubject` reads first
anyway. Names are folded the way `imgsubject::key` folds them, so the file holds one spelling of each.
"""

from __future__ import annotations

import collections
import csv
import json
import pathlib
import re
import sys
import time
import urllib.parse
import urllib.request

HERE = pathlib.Path(__file__).resolve().parent.parent
OI = pathlib.Path.home() / ".cache/concepts/oi"
WIKIDATA = OI / "wikidata_fa.json"
TRAIN_LABELS = OI / "oidv7-train-annotations-human-imagelabels.csv"
TABLE = HERE / "assets/imgfilter_subjects.tsv"
OUT = HERE / "assets/imgfilter_lexicon.tsv"
MAX_NAME = 32

SEPARATORS = {"​", "‌"}
IGNORED = {chr(c) for c in [0x640, 0x670, 0x61C, 0x200E, 0x200F, 0xFEFF, *range(0x64B, 0x660),
                             *range(0x202A, 0x202F), *range(0x2066, 0x206A)]}
EXTRA = str.maketrans({"آ": "ا", "أ": "ا", "إ": "ا", "ٱ": "ا", "ة": "ه", "ۀ": "ه", "ؤ": "و", "ئ": "ی"})


def fold(c: str) -> str:
    if "۰" <= c <= "۹":
        return chr(ord("0") + ord(c) - ord("۰"))
    if "٠" <= c <= "٩":
        return chr(ord("0") + ord(c) - ord("٠"))
    if c in "يى":
        return "ی"
    if c == "ك":
        return "ک"
    return c.lower() if c.isascii() else c


def key(text: str) -> str:
    """`commands::normalize`, then `imgsubject`'s own letter folds."""
    out, started, spaced = [], False, False
    for c in text:
        if c in IGNORED:
            continue
        if c.isspace() or c in SEPARATORS:
            if started and not spaced:
                out.append(" ")
                spaced = True
            continue
        started, spaced = True, False
        out.append(fold(c))
    return "".join(out).strip().translate(EXTRA)


def fetch() -> None:
    classes = list(csv.reader(open(OI / "oidv7-class-descriptions.csv", encoding="utf-8")))[1:]
    done = json.loads(WIKIDATA.read_text(encoding="utf-8")) if WIKIDATA.exists() else {}
    todo = [mid for mid, _ in classes if mid not in done]
    agent = "groupbot-lexicon/1.0 (offline asset builder)"
    for at in range(0, len(todo), 900):
        batch = todo[at:at + 900]
        values = " ".join(f'"{m}"' for m in batch)
        query = f"""SELECT ?mid ?item ?fa ?alias ?en WHERE {{
          VALUES ?mid {{ {values} }}
          ?item wdt:P646 ?mid .
          OPTIONAL {{ ?item rdfs:label ?fa . FILTER(LANG(?fa) = "fa") }}
          OPTIONAL {{ ?item skos:altLabel ?alias . FILTER(LANG(?alias) = "fa") }}
          OPTIONAL {{ ?item rdfs:label ?en . FILTER(LANG(?en) = "en") }}
        }}"""
        body = urllib.parse.urlencode({"query": query, "format": "json"}).encode()
        for attempt in range(6):
            try:
                request = urllib.request.Request(
                    "https://query.wikidata.org/sparql", data=body,
                    headers={"User-Agent": agent, "Accept": "application/sparql-results+json",
                             "Content-Type": "application/x-www-form-urlencoded"})
                with urllib.request.urlopen(request, timeout=300) as response:
                    rows = json.load(response)["results"]["bindings"]
                break
            except Exception as error:  # noqa: BLE001
                print(f"retry {at}: {error}", flush=True)
                time.sleep(65)
        else:
            sys.exit("Wikidata did not answer")
        for mid in batch:
            done.setdefault(mid, {"items": [], "fa": [], "alias": [], "en": []})
        for row in rows:
            entry = done[row["mid"]["value"]]
            item = row["item"]["value"].rsplit("/", 1)[-1]
            if item not in entry["items"]:
                entry["items"].append(item)
            for field in ("fa", "alias", "en"):
                if field in row and row[field]["value"] not in entry[field]:
                    entry[field].append(row[field]["value"])
        WIKIDATA.write_text(json.dumps(done, ensure_ascii=False), encoding="utf-8")
        print(f"{at + len(batch)} / {len(todo)}", flush=True)
        time.sleep(62)


def frequencies() -> collections.Counter:
    count: collections.Counter = collections.Counter()
    if not TRAIN_LABELS.exists():
        print(f"no {TRAIN_LABELS.name}: shared names go to the first class", file=sys.stderr)
        return count
    with open(TRAIN_LABELS, encoding="utf-8") as handle:
        reader = csv.reader(handle)
        next(reader)
        for _, _, label, confidence in reader:
            if confidence == "1":
                count[label] += 1
    return count


PERSIAN = re.compile(r"^[؀-ۿ ]+$")

# Names Wikidata gives a class that, typed alone, mean something else first: function words («به» is a
# quince there, «وی» the Wii), words whose class would delete everything («عکس» is a photograph, «رنگ» a
# colour), and ones whose everyday sense is not the class's («کپی» an ape, «خرج» ammunition, «کر» a horn).
STOP = {key(word) for word in """
عکس تصویر رنگ فام چیز شکل صفت حس محل سطح نفر کار شغل روز مهر گو کر وی به می دل تیر خط حرف نور پست متر مدل
ترش طعم دما صدا شعر منو کپی ادا ایل خرج حمل بین باب دو باز کره تخم ایران عدد سنت بیع
""".split()}


def usable(name: str) -> bool:
    """A Persian name an admin could type: Persian letters and spaces only, no digits (Wikidata names car
    models and race distances), at most four words and `MAX_NAME` letters."""
    return (2 <= len(name) <= MAX_NAME and PERSIAN.match(name) is not None
            and not re.search(r"[0-9۰-۹٠-٩]", name) and len(name.split()) <= 4)


def write() -> None:
    names = {mid: name for mid, name in list(csv.reader(open(OI / "oidv7-class-descriptions.csv", encoding="utf-8")))[1:]}
    wikidata = json.loads(WIKIDATA.read_text(encoding="utf-8"))
    taken = set()
    for line in TABLE.read_text(encoding="utf-8").splitlines():
        if line.strip() and not line.startswith("#"):
            taken.update(key(name) for name in line.split("\t")[2].split("|") if name)
    count = frequencies()
    claims: dict[str, list[tuple[int, str]]] = collections.defaultdict(list)
    for mid, entry in wikidata.items():
        if mid not in names:
            continue
        for raw in entry["fa"] + entry["alias"]:
            name = key(re.sub(r"\s*\([^)]*\)", "", raw))
            if usable(name) and name not in taken and name not in STOP:
                claims[name].append((count[mid], mid))
    rows: dict[str, list[str]] = collections.defaultdict(list)
    for name, owners in claims.items():
        owners.sort(key=lambda owner: (-owner[0], len(names[owner[1]]), owner[1]))
        rows[owners[0][1]].append(name)
    # Two classes can share a display name; their names then share its row.
    by_english: dict[str, set[str]] = collections.defaultdict(set)
    for mid, owned in rows.items():
        by_english[" ".join(names[mid].lower().split())].update(owned)
    lines = [
        "# Persian names for Open Images classes, from Wikidata (CC0) — written by tools/imgfilter_lexicon.py.",
        "# Same format as imgfilter_subjects.tsv, which imgsubject reads first and which wins every shared name.",
        "",
    ]
    for english in sorted(by_english):
        lines.append(f"visual\t{english}\t{'|'.join(sorted(by_english[english]))}")
    OUT.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(f"{len(by_english)} classes, {sum(len(v) for v in by_english.values())} names -> {OUT}")


if __name__ == "__main__":
    {"fetch": fetch, "write": write}[sys.argv[1]]()
