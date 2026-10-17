"""Probe: decode every Odin blob in the given serialized files; report success per class and print one sample."""
import collections
import json
import pathlib
import sys

import UnityPy

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))  # sibling modules under python -I
from UnityPy.helpers.TypeTreeGenerator import TypeTreeGenerator

import odin

data_dir = pathlib.Path(sys.argv[1])
files = sys.argv[2].split(",")
sample = sys.argv[3] if len(sys.argv) > 3 else "Skill"

gen = TypeTreeGenerator("2017.4.19f1")
gen.load_local_dll_folder(str(data_dir / "Managed"))

ok, bad, errors = collections.Counter(), collections.Counter(), {}
for name in files:
    env = UnityPy.load(str(data_dir / name))
    env.typetree_generator = gen
    for obj in env.objects:
        if obj.type.name != "MonoBehaviour":
            continue
        try:
            tree = obj.read_typetree()
        except Exception:
            continue
        blob = (tree.get("serializationData") or {}).get("SerializedBytes")
        if not blob:
            continue
        cls = tree.get("m_Name", "?").split(" ")[0]
        try:
            value = odin.decode(blob)
        except Exception as e:
            bad[cls] += 1
            errors.setdefault(cls, f"{type(e).__name__}: {e}")
            continue
        ok[cls] += 1
        if sample and tree.get("m_Name", "").startswith(sample):
            print(f"=== {tree['m_Name']} ({len(blob)} bytes)")
            print(json.dumps(value, indent=1)[:4000])
            sample = None

print(f"TOTAL blobs ok={sum(ok.values())} bad={sum(bad.values())}")
for cls, n in bad.most_common(15):
    print(f"  bad {n:5} {cls}: {errors[cls][:100]}")
