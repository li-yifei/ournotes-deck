#!/usr/bin/env python3
"""Extract game card textures and build a local asset index (requires UnityPy).

Usage: python extract_card_textures.py DECODED_BUNDLES ASSETS_ROOT INDEX_JSON
The ASSETS_ROOT is the manifest's assetsDir, so index keys are relative to it.
"""
import argparse
import json
import re
from pathlib import Path

import UnityPy

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("bundles", type=Path)
parser.add_argument("assets", type=Path)
parser.add_argument("index", type=Path)
args = parser.parse_args()
pattern = re.compile(r"(membercard|supportcard)_assets_\1_(\d+)_(member|snap)_(full|thumbnail|background|character)_")
count = failures = 0
for bundle in sorted(args.bundles.glob("*.bundle")):
    match = pattern.search(bundle.name.lower())
    if not match:
        continue
    kind, card_id, prefix, variant = match.groups()
    folder = "MemberCard" if kind == "membercard" else "SupportCard"
    try:
        textures = [obj.read() for obj in UnityPy.load(str(bundle)).objects if obj.type.name == "Texture2D"]
        # The card bundle has one texture; select largest if auxiliary textures exist.
        textures.sort(key=lambda t: t.m_Width * t.m_Height, reverse=True)
        if not textures:
            raise ValueError("no Texture2D")
        image = textures[0].image
        if image is None:
            raise ValueError("texture decoder returned no image")
        output = args.assets / "Assets/AddressableResources" / folder / card_id / f"{prefix}_{variant}.png"
        output.parent.mkdir(parents=True, exist_ok=True)
        image.save(output)
        count += 1
    except Exception as exc:
        failures += 1
        print(f"ERROR {bundle.name}: {exc}")

index = {"format": "ournotes.card-assets/1", "source": "android-game-cache", "members": [], "snaps": []}
for folder, group, prefix in [("MemberCard", "members", "member"), ("SupportCard", "snaps", "snap")]:
    base = args.assets / "Assets/AddressableResources" / folder
    for card in sorted(base.glob("*"), key=lambda p: int(p.name) if p.name.isdigit() else 10**12):
        if not card.is_dir() or not card.name.isdigit():
            continue
        full, thumb = card / f"{prefix}_full.png", card / f"{prefix}_thumbnail.png"
        entry = {"id": int(card.name)}
        if full.is_file(): entry["assetKey"] = full.relative_to(args.assets).as_posix()
        if thumb.is_file(): entry["thumbnailKey"] = thumb.relative_to(args.assets).as_posix()
        if len(entry) > 1: index[group].append(entry)
args.index.parent.mkdir(parents=True, exist_ok=True)
args.index.write_text(json.dumps(index, ensure_ascii=False, indent=2) + "\n")
print(f"textures {count} failures {failures}; members {len(index['members'])} snaps {len(index['snaps'])}")
if failures:
    raise SystemExit(1)
