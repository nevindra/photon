#!/usr/bin/env python3
"""Drop manifest entries whose Parquet file did not survive the copy.

A hot tier copied out of a *running* Photon is never a clean snapshot: `merge_once` consolidates
small Parquet files and unlinks the originals while the copy is in flight, so whichever of the two
(manifest, data) you read first ends up naming a file the other side no longer has. The query
engine reads the manifest first and then opens every file it names, so one stale entry fails the
whole query with "Object at location ... not found".

Pruning is the right repair for a replica, and only for a replica: the manifest is the index, not
the data, and an entry with no file behind it indexes nothing. On a real deployment this situation
does not arise (the manifest is updated in the same pass that writes the file) and a missing file
would be a genuine fault worth failing on.

    python3 scripts/prune-replica-manifest.py .photon-prod/hot
"""
import json
import os
import sys

MANIFESTS = ("manifest.json", "spans-manifest.json", "metrics-manifest.json")


def main(hot_dir: str) -> int:
    manifest_dir = os.path.join(hot_dir, "manifest")
    if not os.path.isdir(manifest_dir):
        print(f"no manifest directory under {hot_dir}", file=sys.stderr)
        return 1

    total_pruned = 0
    for name in MANIFESTS:
        path = os.path.join(manifest_dir, name)
        if not os.path.exists(path):
            continue
        with open(path) as fh:
            manifest = json.load(fh)
        entries = manifest.get("entries", [])
        kept = [e for e in entries if os.path.exists(os.path.join(hot_dir, e["path"]))]
        pruned = len(entries) - len(kept)
        total_pruned += pruned
        if pruned:
            manifest["entries"] = kept
            with open(path, "w") as fh:
                json.dump(manifest, fh, separators=(",", ":"))
        print(f"{name:24s} {len(entries):5d} entries, {pruned} pruned")

    print(f"\n{total_pruned} entr{'y' if total_pruned == 1 else 'ies'} pruned in total")
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1] if len(sys.argv) > 1 else ".photon-prod/hot"))
