"""Probe: can MonoBehaviour data be decoded using type trees generated from the user's own Managed DLLs?

Prints a sample of each wanted class and the decode success rate for every class seen.
"""
import collections
import json
import pathlib
import sys

import UnityPy
from UnityPy.helpers.TypeTreeGenerator import TypeTreeGenerator

data_dir = pathlib.Path(sys.argv[1])
wanted = set(sys.argv[2].split(","))
files = sys.argv[3].split(",") if len(sys.argv) > 3 else None

gen = TypeTreeGenerator("2017.4.19f1")
gen.load_local_dll_folder(str(data_dir / "Managed"))

ok = collections.Counter()
bad = collections.Counter()
errors = {}
shown = set()
paths = [data_dir / f for f in files] if files else sorted(data_dir.glob("*.assets"))
for path in paths:
    plain = UnityPy.load(str(path))
    script_of = {}
    for obj in plain.objects:
        if obj.type.name == "MonoBehaviour":
            try:
                script_of[obj.path_id] = obj.read(check_read=False).m_Script.read().m_ClassName
            except Exception:
                pass
    env = UnityPy.load(str(path))
    env.typetree_generator = gen
    for obj in env.objects:
        name = script_of.get(obj.path_id)
        if obj.type.name != "MonoBehaviour" or name is None:
            continue
        try:
            tree = obj.read_typetree()
        except Exception as e:
            bad[name] += 1
            errors.setdefault(name, f"{type(e).__name__}: {e}")
            continue
        ok[name] += 1
        if name in wanted and name not in shown:
            shown.add(name)
            text = json.dumps(tree, default=str, indent=1)
            print(f"=== {name} in {path.name} ({len(text)} chars)")
            print(text[:3000])

print("=== decode rate (classes with failures first)")
for name in sorted(set(ok) | set(bad), key=lambda n: (-bad[n], -ok[n]))[:40]:
    print(f"{ok[name]:6} ok {bad[name]:6} bad  {name}  {errors.get(name, '')[:90]}")
print(f"TOTAL ok={sum(ok.values())} bad={sum(bad.values())}")
