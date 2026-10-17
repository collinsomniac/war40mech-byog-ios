"""Export game-rules data from the user's own install into a local content pack (never committed).

For each wanted MonoBehaviour class, writes pack/data/<Class>.json: a list of objects holding the plain serialized
fields plus the decoded Odin payload. Asset references stay as {file, path_id} for later passes to resolve.
"""
import argparse
import collections
import json
import pathlib
import sys
import time

import UnityPy

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))  # sibling modules under python -I
from UnityPy.helpers.TypeTreeGenerator import TypeTreeGenerator

import odin

DEFAULT_CLASSES = [
    "Skill", "Equipment", "TechpriestDataBase", "DungeonEvent", "RoomData", "Blueprint", "StoryDialog",
    "SarcophagusDataMb", "NecronPropsDataMb", "FlayedOneSpawnDataMb", "NecronTacticalPointCollectableArchiveDataMb",
]


def class_names(path: pathlib.Path) -> dict[int, str]:
    """path_id -> script class name, read without generated type trees (cheap and never fails on layout)."""
    names = {}
    for obj in UnityPy.load(str(path)).objects:
        if obj.type.name == "MonoBehaviour":
            try:
                names[obj.path_id] = obj.read(check_read=False).m_Script.read().m_ClassName
            except Exception:
                pass
    return names


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("data_dir", type=pathlib.Path, help="<install>/Mechanicus_Data")
    ap.add_argument("--pack", type=pathlib.Path, required=True, help="content pack output directory")
    ap.add_argument("--classes", default=",".join(DEFAULT_CLASSES))
    args = ap.parse_args()

    wanted = set(args.classes.split(","))
    gen = TypeTreeGenerator("2017.4.19f1")
    gen.load_local_dll_folder(str(args.data_dir / "Managed"))

    out = collections.defaultdict(list)
    failures = collections.Counter()
    started = time.time()
    for path in sorted(args.data_dir.glob("*.assets")):
        names = class_names(path)
        if not wanted & set(names.values()):
            continue
        env = UnityPy.load(str(path))
        env.typetree_generator = gen
        for obj in env.objects:
            cls = names.get(obj.path_id)
            if cls not in wanted:
                continue
            try:
                tree = obj.read_typetree()
                serialization = tree.pop("serializationData", None) or {}
                blob = serialization.get("SerializedBytes")
                record = {"file": path.name, "path_id": obj.path_id, "fields": tree}
                if blob:
                    record["odin"] = odin.decode(blob)
                    record["odin_unity_refs"] = serialization.get("ReferencedUnityObjects", [])
                out[cls].append(record)
            except Exception as e:
                failures[f"{cls}: {type(e).__name__}"] += 1
        print(f"{path.name}: done", file=sys.stderr)

    data_dir = args.pack / "data"
    data_dir.mkdir(parents=True, exist_ok=True)
    for cls, records in out.items():
        records.sort(key=lambda r: r["fields"].get("m_Name", ""))
        (data_dir / f"{cls}.json").write_text(json.dumps(records, default=str))
    summary = {
        "seconds": round(time.time() - started, 1),
        "counts": {cls: len(r) for cls, r in sorted(out.items())},
        "failures": dict(failures),
    }
    (data_dir / "_summary.json").write_text(json.dumps(summary, indent=1))
    print(json.dumps(summary, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
