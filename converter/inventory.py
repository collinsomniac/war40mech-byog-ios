"""Inventory a local Mechanicus install: object types per file and MonoBehaviour script usage.

Reads the user's own copy; writes only a metadata summary (no asset content) to --out.
"""
import argparse
import collections
import json
import pathlib
import sys
import time

import UnityPy


def serialized_files(data_dir: pathlib.Path):
    for path in sorted(data_dir.iterdir()):
        name = path.name
        if path.is_file() and (name.endswith(".assets") or name.startswith("level") and "." not in name
                               or name == "globalgamemanagers"):
            yield path


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("data_dir", type=pathlib.Path, help="<install>/Mechanicus_Data")
    ap.add_argument("--out", type=pathlib.Path, required=True)
    args = ap.parse_args()

    totals = collections.Counter()
    scripts = collections.Counter()
    files = {}
    started = time.time()
    for path in serialized_files(args.data_dir):
        env = UnityPy.load(str(path))
        counts = collections.Counter()
        for obj in env.objects:
            counts[obj.type.name] += 1
            if obj.type.name == "MonoBehaviour":
                try:
                    script = obj.read(check_read=False).m_Script.read()
                    scripts[f"{script.m_Namespace}.{script.m_ClassName}".lstrip(".")] += 1
                except Exception:
                    scripts["<unresolved>"] += 1
        files[path.name] = dict(counts)
        totals.update(counts)
        print(f"{path.name}: {sum(counts.values())} objects", file=sys.stderr)

    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps({
        "seconds": round(time.time() - started, 1),
        "totals": dict(totals.most_common()),
        "monobehaviour_scripts": dict(scripts.most_common()),
        "files": files,
    }, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
