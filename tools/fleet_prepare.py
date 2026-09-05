#!/usr/bin/env python3
"""Prepare secret-free per-shard directories from a reviewed fleet manifest.

This command validates the manifest, routes the complete chat inventory, verifies the resulting
route set against every declared capacity, and writes only ownership files plus ``.env.example``
templates. It never writes a bot token or database password and never starts a service.
Existing files are preserved unless they are one of the generated files and ``--force`` is used.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import tempfile
from typing import Any, Mapping

from fleet_manifest import validate_manifest
from fleet_routes import validate_route_sets
from shard_route import (
    assign,
    read_counter_weights,
    read_ids,
    read_note_weights,
    read_weights,
)


class PrepareError(ValueError):
    """The manifest, inventory, or output destination is unsafe."""


def _load_document(path: Path) -> dict[str, Any]:
    try:
        document = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise PrepareError(f"could not read manifest {path}: {error}") from error
    if not isinstance(document, dict):
        raise PrepareError("manifest root must be an object")
    return document


def _check_coverage(expected: set[int], actual: Mapping[int, int], label: str) -> None:
    ids = set(actual)
    if ids != expected:
        raise PrepareError(
            f"{label} coverage mismatch: missing={len(expected - ids)}, "
            f"extra={len(ids - expected)}"
        )


def _atomic_write(path: Path, content: str, force: bool) -> None:
    if path.exists() and not force:
        raise PrepareError(f"output already exists: {path}; use --force after reviewing it")
    path.parent.mkdir(parents=True, exist_ok=True)
    handle = tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", dir=path.parent, prefix=f".{path.name}.", delete=False
    )
    temporary = Path(handle.name)
    try:
        with handle:
            handle.write(content)
            handle.flush()
            os.fsync(handle.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def prepare(
    document: dict[str, Any],
    chat_ids: list[int],
    output_dir: Path,
    *,
    weights: Mapping[int, tuple[int, int]] | None = None,
    counter_weights: Mapping[int, int] | None = None,
    note_weights: Mapping[int, int] | None = None,
    host: str | None = None,
    force: bool = False,
) -> dict[str, Any]:
    checked = validate_manifest(document)
    expected = set(chat_ids)
    if not expected or len(expected) != len(chat_ids):
        raise PrepareError("chat inventory must be non-empty and contain no duplicates")
    if len(expected) != checked["assigned_groups"]:
        raise PrepareError(
            f"chat inventory has {len(expected)} groups, manifest declares "
            f"{checked['assigned_groups']}"
        )
    if weights is not None:
        _check_coverage(expected, weights, "settings weights")
    if counter_weights is not None:
        _check_coverage(expected, counter_weights, "counter weights")
    if note_weights is not None:
        _check_coverage(expected, note_weights, "note weights")

    shard_documents = {shard["name"]: shard for shard in document["shards"]}
    if host is not None:
        selected = [name for name, shard in shard_documents.items() if shard["host"] == host]
        if not selected:
            raise PrepareError(f"host {host!r} owns no declared shards")
    else:
        selected = list(shard_documents)

    required = checked["required"]
    assignment = assign(
        chat_ids,
        shard_documents,
        checked["routing"]["seed"],
        required["groups_per_shard"],
        row_weights={chat: value[0] for chat, value in weights.items()}
        if weights is not None
        else None,
        byte_weights={chat: value[1] for chat, value in weights.items()}
        if weights is not None
        else None,
        max_rows=required["max_settings_rows_per_shard"] if weights is not None else None,
        max_bytes=required["max_settings_bytes_per_shard"] if weights is not None else None,
        counter_rows_per_group=required["counter_rows_per_group"],
        max_counter_rows=required["max_counter_rows_per_shard"],
        counter_row_weights=counter_weights,
        note_rows_per_group=required["note_rows_per_group"],
        max_note_rows=required["max_note_rows_per_shard"],
        note_row_weights=note_weights,
    )
    routes = {
        name: [chat for chat in chat_ids if assignment[chat] == name]
        for name in shard_documents
    }
    declared = {name: shard["groups"] for name, shard in shard_documents.items()}
    validate_route_sets(
        chat_ids,
        routes,
        declared,
        required["groups_per_shard"],
        weights=weights,
        max_settings_rows=required["max_settings_rows_per_shard"] if weights else None,
        max_settings_bytes=required["max_settings_bytes_per_shard"] if weights else None,
        counter_rows_per_group=required["counter_rows_per_group"],
        max_counter_rows=required["max_counter_rows_per_shard"],
        counter_weights=counter_weights,
        note_rows_per_group=required["note_rows_per_group"],
        max_note_rows=required["max_note_rows_per_shard"],
        note_weights=note_weights,
    )

    output_dir = output_dir.resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    generated: list[dict[str, Any]] = []
    for name in selected:
        shard_dir = output_dir / name
        ids = sorted(routes[name])
        route_path = shard_dir / "chat_ids.txt"
        _atomic_write(route_path, "".join(f"{chat}\n" for chat in ids), force)
        env_path = shard_dir / ".env.example"
        shard = shard_documents[name]
        env = (
            "# Fill these values from your secret manager; this template contains no secrets.\n"
            f"# token_ref={shard['token_ref']}\n"
            f"# database_ref={shard['database_ref']}\n"
            "TG_ID=\n"
            "TG_HASH=\n"
            "TG_BOT_TOKEN=\n"
            "DATABASE_URL=\n"
            f"SHARD_NAME={name}\n"
            f"SHARD_CHAT_IDS_FILE={route_path}\n"
        )
        _atomic_write(env_path, env, force)
        generated.append(
            {
                "name": name,
                "host": shard["host"],
                "groups": len(ids),
                "chat_ids": str(route_path),
                "env_template": str(env_path),
            }
        )

    report = {
        "groups": len(chat_ids),
        "declared_shards": len(shard_documents),
        "prepared_shards": len(generated),
        "host": host,
        "output_dir": str(output_dir),
        "shards": generated,
    }
    _atomic_write(
        output_dir / "fleet_prepare.json",
        json.dumps(report, indent=2, sort_keys=True) + "\n",
        force,
    )
    return report


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("manifest", type=Path)
    parser.add_argument("--input", type=Path, required=True)
    parser.add_argument("--output-dir", type=Path, required=True)
    parser.add_argument("--weights", type=Path)
    parser.add_argument("--counter-weights", type=Path)
    parser.add_argument("--note-weights", type=Path)
    parser.add_argument("--host")
    parser.add_argument("--force", action="store_true")
    parser.add_argument("--json", action="store_true")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    try:
        document = _load_document(args.manifest)
        chat_ids = read_ids(args.input)
        weights = read_weights(args.weights) if args.weights else None
        counter_weights = read_counter_weights(args.counter_weights) if args.counter_weights else None
        note_weights = read_note_weights(args.note_weights) if args.note_weights else None
        report = prepare(
            document,
            chat_ids,
            args.output_dir,
            weights=weights,
            counter_weights=counter_weights,
            note_weights=note_weights,
            host=args.host,
            force=args.force,
        )
    except (OSError, KeyError, ValueError) as error:
        print(f"fleet prepare: invalid: {error}")
        return 2
    if args.json:
        print(json.dumps(report, sort_keys=True))
    else:
        print(
            f"prepared {report['prepared_shards']} of {report['declared_shards']} shards "
            f"for {report['groups']} groups under {report['output_dir']}"
        )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
