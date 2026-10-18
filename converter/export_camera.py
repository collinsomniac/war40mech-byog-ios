"""Camera rigs of non-battle scenes (the tomb table): writes pack/camera/<Scene>.json with the
CameraContainer settings, as export_battle does for battle rooms."""
import argparse
import json
import pathlib
import sys

import UnityPy

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))  # sibling modules under python -I
import export_battle  # noqa: E402
from UnityPy.helpers.TypeTreeGenerator import TypeTreeGenerator  # noqa: E402


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("data_dir", type=pathlib.Path)
    ap.add_argument("--pack", type=pathlib.Path, required=True)
    ap.add_argument("--scene", action="append", default=[], help="levelN=Name")
    args = ap.parse_args()
    gen = TypeTreeGenerator("2017.4.19f1")
    gen.load_local_dll_folder(str(args.data_dir / "Managed"))
    out = args.pack / "camera"
    out.mkdir(parents=True, exist_ok=True)
    for spec in args.scene:
        level, name = spec.split("=")
        scene = export_battle.Scene(UnityPy.load(str(args.data_dir / level)))
        typed = UnityPy.load(str(args.data_dir / level))
        typed.typetree_generator = gen
        objs = {o.path_id: o for o in typed.objects}

        def typetree_of(pid):
            try:
                return objs[pid].read_typetree(check_read=False)
            except Exception:
                return None

        rig = export_battle.camera_rig(scene, typetree_of)
        env = UnityPy.load(str(args.data_dir / level))
        if rig is not None:
            rig["render"] = export_battle.render_settings(env, scene, typetree_of)
        (out / f"{name}.json").write_text(json.dumps(rig))
        print(name, rig)
    return 0


if __name__ == "__main__":
    sys.exit(main())
