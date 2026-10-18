"""Unit models: every entity data record's `prefabEntity` becomes models/units/<file id>_<path id>.glb,
named by the same (FNV-1a of the file name, path id) key the app's data layer uses for references.
Skinned bodies, rigid parts and their Animator clips are included.
"""
import argparse
import json
import pathlib
import sys

import UnityPy

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))  # sibling modules under python -I
import export_prefab  # noqa: E402


def file_id(name: str) -> int:
    h = 0x811C9DC5
    for b in name.encode():
        h = ((h ^ b) * 0x01000193) & 0xFFFFFFFF
    return h


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("data_dir", type=pathlib.Path)
    ap.add_argument("--pack", type=pathlib.Path, required=True)
    args = ap.parse_args()
    data = args.pack / "data"
    files = json.loads((data / "_files.json").read_text())
    wanted: dict[str, dict[int, str]] = {}
    for path in sorted(data.glob("*.json")):
        if path.name.startswith("_"):
            continue
        for r in json.loads(path.read_text()):
            p = r["fields"].get("prefabEntity")
            if not isinstance(p, dict) or not p.get("m_PathID"):
                continue
            file = r["file"] if p["m_FileID"] == 0 else files[r["file"]][p["m_FileID"] - 1]
            wanted.setdefault(file, {})[p["m_PathID"]] = r["fields"].get("m_Name", "")
    exporter = export_prefab.Exporter(args.pack)
    out_dir = exporter.models_dir / "units"
    out_dir.mkdir(parents=True, exist_ok=True)
    summary = {}
    for file, prefabs in wanted.items():
        env = UnityPy.load(str(args.data_dir / file))
        objects = {o.path_id: o for o in env.objects}
        for pid, name in sorted(prefabs.items()):
            obj = objects.get(pid)
            if obj is None or obj.type.name != "GameObject":
                continue
            go = obj.read()
            transform = next((c.component.read() for c in go.m_Component if c.component.type.name == "Transform"), None)
            if transform is None:
                continue
            key = f"{file_id(file)}_{pid}"
            try:
                stats = exporter.export(transform, f"units/{key}")
            except Exception as e:
                stats = f"{type(e).__name__}: {e}"[:200]
            if isinstance(stats, dict) and not stats.get("meshes") and not stats.get("skinned"):
                (out_dir / f"{key}.glb").unlink(missing_ok=True)  # empty (built at runtime, e.g. tech-priests)
                stats = "empty"
            summary[key] = {"prefab": go.m_Name, "example": name, "stats": stats}
            print(key, go.m_Name, stats, flush=True)
    (out_dir / "_summary.json").write_text(json.dumps(summary, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
