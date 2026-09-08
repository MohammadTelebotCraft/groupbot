"""Check the semantic emoji registry and presentation boundaries without Telegram access."""
import collections
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
REGISTRY = ROOT / "src/handlers/premium/registry.json"


def audit():
    rows = json.loads(REGISTRY.read_text(encoding="utf-8"))
    ids = set()
    keys = set()
    for row in rows:
        assert row["key"] not in keys, f"duplicate key: {row['key']}"
        keys.add(row["key"])
        value = row["custom_emoji_id"]
        if value is not None:
            assert isinstance(value, str) and re.fullmatch(r"[1-9][0-9]*", value)
            assert 0 < int(value) <= 2**63 - 1
            assert value not in ids, f"duplicate document: {value}"
            ids.add(value)
        if row["usage"] == "ACTIVE":
            assert row["confidence"] == "HIGH" and row["semantic_tags"]
        assert row["fallback"] and row["source"]

    leaked = []
    surfaces = {}
    for path in sorted((ROOT / "src").rglob("*")):
        if path.suffix not in {".rs", ".js", ".html", ".css", ".json"}:
            continue
        source = path.read_text(encoding="utf-8")
        relative = path.relative_to(ROOT).as_posix()
        if path != REGISTRY and relative != "src/handlers/premium/tests.rs":
            for match in re.finditer(r"(?<!\w)[0-9][0-9_]{15,}(?!\w)", source):
                if match[0].replace("_", "") in ids:
                    leaked.append(f"{relative}:{source.count(chr(10), 0, match.start()) + 1}")
        if re.search(r"\.reply\(|\.respond\(|\.alert\(|send_message\(|Button::|innerHTML", source):
            surfaces[relative] = len(re.findall(r"premium::|semantic-icon|MODERATION_ICONS", source))
    assert not leaked, "custom IDs outside registry/tests: " + ", ".join(leaked)

    for path in (ROOT / "src/handlers/premium").glob("*.rs"):
        source = path.read_text(encoding="utf-8")
        assert not any(name in source for name in ("fn custom_entities", "fn entity_document", "fn button_document", "fn contains_any")), path
    counts = collections.Counter(row["usage"] for row in rows if row["custom_emoji_id"])
    result = {
        "custom_documents": len(ids),
        "usage": dict(counts),
        "source_surfaces_audited": len(surfaces),
        "raw_id_leaks": leaked,
        "presentation_references_by_file": surfaces,
    }
    print(json.dumps(result, indent=2, ensure_ascii=False))
    return result


if __name__ == "__main__":
    audit()
