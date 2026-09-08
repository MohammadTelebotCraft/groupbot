"""Training-time mirror of intent_normalize.rs; deployment evaluation runs Rust itself.

Spelling aliases are read from the Rust table. Runtime evaluation checks this mirror against
the canonical text returned by Rust on every evaluated row.
"""
from __future__ import annotations

import functools
import itertools
import pathlib
import re
import unicodedata

SOURCE = pathlib.Path(__file__).resolve().parents[1] / "src/handlers/intent_normalize.rs"
_table = SOURCE.read_text(encoding="utf-8").split("const WORDS:", 1)[1].split("];", 1)[0]
WORDS = dict(re.findall(r'\("([^"]+)",\s*"([^"]+)"\)', _table))
assert WORDS, "normalization aliases must be read from Rust"
LOOKALIKES = str.maketrans({"а": "a", "α": "a", "е": "e", "ε": "e", "о": "o", "ο": "o",
                          "р": "p", "ρ": "p", "с": "c", "ѕ": "s", "і": "i", "ι": "i",
                          "ӏ": "l", "у": "y", "х": "x", "χ": "x", "т": "t"})
SEPARATORS = ".·-_*/\\|~"


def runs(text):
    return [(c, len(list(group))) for c, group in itertools.groupby(text)]


PATTERNS = [(runs(word), canonical) for word, canonical in WORDS.items()]


@functools.lru_cache(maxsize=8192)
def lookup(text):
    if text in WORDS:
        return WORDS[text]
    candidate = runs(text)
    for expected, canonical in PATTERNS:
        if len(candidate) == len(expected) and all(a == b and n >= m for (a, n), (b, m) in zip(candidate, expected)):
            return canonical
    return None


def invisible(c):
    return (c in "\u00ad\u034f\u061c\u0640\u0670\ufeff"
            or "\u200b" <= c <= "\u200f" or "\u202a" <= c <= "\u202e"
            or "\u2060" <= c <= "\u206f" or "\ufe00" <= c <= "\ufe0f"
            or "\u064b" <= c <= "\u065f" or "\u06d6" <= c <= "\u06ed")


def soft(c):
    return c == " " or c in SEPARATORS or "\u2600" <= c <= "\u27bf" or "\U0001f000" <= c <= "\U0001faff"


def descramble(text: str) -> tuple[str, bool]:
    folded, tampered = [], False
    for c in unicodedata.normalize("NFKC", text):
        for c in c.lower():
            if invisible(c):
                tampered = True
                if c in "\u200b\u200c\u200d\u2060\ufeff":
                    folded.append(" ")
            else:
                folded.append(chr(ord(c) - 0x660 + 0x6f0) if "\u0660" <= c <= "\u0669" else {"ي": "ی", "ى": "ی", "ك": "ک"}.get(c, " " if c.isspace() else c))
    out, at = [], 0
    while at < len(folded):
        c = folded[at]
        if at == 0 or folded[at - 1].isspace():
            end = at
            while end < len(folded) and not folded[end].isspace():
                end += 1
            token = "".join(folded[at:end])
            if "://" in token or token.startswith(("t.me/", "www.")) or "@" in token:
                out.append(token)
                at = end
                continue
        if soft(c) and not c.isspace() and at > 0 and folded[at - 1].isalpha():
            end = at + 1
            while end < len(folded) and soft(folded[end]) and not folded[end].isspace():
                end += 1
            if end < len(folded) and folded[end].isalpha():
                out.append(" ")
                tampered = True
                at = end
                continue
        if c.isalpha() and (at == 0 or not folded[at - 1].isalnum()):
            candidate, best = "", None
            for end in range(at, min(len(folded), at + 96)):
                nxt = folded[end]
                if nxt.isalpha():
                    candidate += nxt.translate(LOOKALIKES)
                elif not soft(nxt):
                    break
                else:
                    continue
                if len(candidate) > 32:
                    break
                if end + 1 == len(folded) or not folded[end + 1].isalnum():
                    word = lookup(candidate)
                    if word is not None:
                        best = end + 1, word
            if best is not None:
                end, word = best
                tampered |= "".join(folded[at:end]) != word
                out.append(word)
                at = end
                continue
        end = at + 1
        while end < len(folded) and folded[end] == c:
            end += 1
        copies = 1 if c.isalpha() and "\u0600" <= c <= "\u06ff" and end - at >= 3 else end - at
        out.append(c * copies)
        at = end
    words = []
    for word in "".join(out).split():
        parts = [part for part in re.split("[" + re.escape(SEPARATORS) + "]", word) if part]
        if len(parts) > 1 and all(lookup(part) is not None for part in parts):
            words.append(" ".join(parts))
            tampered = True
        else:
            words.append(word)
    return " ".join(words), tampered
