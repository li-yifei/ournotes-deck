#!/usr/bin/env python3
"""Stage private game resources for the local app.

The repository deliberately keeps account data, deck data and extracted assets out
of git. This command copies resources into a predictable directory and writes a
content manifest consumed by the desktop shell.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
from pathlib import Path


def digest(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for block in iter(lambda: f.read(1024 * 1024), b""):
            h.update(block)
    return h.hexdigest()


def copy_file(src: Path, dst: Path, root: Path, role: str, files: list[dict[str, object]]) -> None:
    if not src.is_file():
        raise SystemExit(f"{role} is not a file: {src}")
    dst.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(src, dst)
    files.append({"role": role, "path": dst.relative_to(root).as_posix(), "bytes": dst.stat().st_size, "sha256": digest(dst)})


def copy_assets(src: Path, dst: Path, root: Path, files: list[dict[str, object]]) -> None:
    if not src.is_dir():
        raise SystemExit(f"assets is not a directory: {src}")
    for path in sorted(p for p in src.rglob("*") if p.is_file()):
        relative = path.relative_to(src)
        target = dst / relative
        target.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(path, target)
        files.append({
            "role": "asset",
            "path": target.relative_to(root).as_posix(),
            "bytes": target.stat().st_size,
            "sha256": digest(target),
        })


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, default=Path("resources/local"))
    parser.add_argument("--deck-data", type=Path)
    parser.add_argument("--roster", type=Path)
    parser.add_argument("--assets", type=Path)
    parser.add_argument("--source-label", default="local-export", help="non-sensitive label stored in the manifest")
    args = parser.parse_args()
    if not any((args.deck_data, args.roster, args.assets)):
        parser.error("provide at least one of --deck-data, --roster or --assets")

    output = args.output
    output.mkdir(parents=True, exist_ok=True)
    files: list[dict[str, object]] = []
    if args.deck_data:
        copy_file(args.deck_data, output / "nnnotes.deck-data/1", output, "deck-data", files)
    if args.roster:
        copy_file(args.roster, output / "account/roster.json", output, "roster", files)
    if args.assets:
        copy_assets(args.assets, output / "assets", output, files)

    manifest = {
        "format": "ournotes.local/1",
        "deckData": "nnnotes.deck-data/1",
        "assetsDir": "assets" if args.assets else None,
        "accountDir": "account" if args.roster else None,
        "source": args.source_label,
        "files": files,
    }
    manifest = {key: value for key, value in manifest.items() if value is not None}
    manifest_path = output / "manifest.json"
    manifest_path.write_text(json.dumps(manifest, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"manifest": str(manifest_path), "files": len(files)}, ensure_ascii=False))


if __name__ == "__main__":
    main()
