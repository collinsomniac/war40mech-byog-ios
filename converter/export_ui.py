"""Export a Unity UI (uGUI + TextMesh Pro) scene from the user's own install into the content pack.

Writes pack/ui/<Scene>.json: the canvas tree with RectTransform layout, Image sprites (9-slice borders
kept), TMP text (with its I2 localization term), the persistent UnityEvent calls of every button, and
the script classes on each node, so the runtime can bind behaviour by class and method name. Sprites go
to pack/ui/sprites/. Also writes pack/loc/<code>.json from the I2 LanguageSource.

usage: export_ui.py <Mechanicus_Data> --pack <dir> --scene level5=MainMenu [--scene ...] [--loc] [--fonts]
"""
import argparse
import json
import pathlib
import sys

import UnityPy

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))  # sibling modules under python -I
from UnityPy.helpers.TypeTreeGenerator import TypeTreeGenerator

import i2loc

SKIP_FIELDS = {"m_GameObject", "m_Script", "m_Enabled", "m_Name", "m_EditorHideFlags", "m_EditorClassIdentifier"}


class Files:
    """Lazily loaded serialized files, so PPtrs into shared assets can be resolved."""

    def __init__(self, data_dir: pathlib.Path, gen: TypeTreeGenerator):
        self.dir, self.gen, self.envs, self.classes = data_dir, gen, {}, {}

    def env(self, name: str):
        if name not in self.envs:
            env = UnityPy.load(str(self.dir / name))
            env.typetree_generator = self.gen
            self.envs[name] = env
            classes = {}
            for o in UnityPy.load(str(self.dir / name)).objects:
                if o.type.name == "MonoBehaviour":
                    try:
                        classes[o.path_id] = o.read(check_read=False).m_Script.read().m_ClassName
                    except Exception:
                        pass
            self.classes[name] = classes
        return self.envs[name]

    def objects(self, name: str) -> dict:
        return {o.path_id: o for o in self.env(name).objects}

    def resolve(self, name: str, pptr: dict):
        """(file name, object) a PPtr points at, or None."""
        if not pptr or not pptr.get("m_PathID"):
            return None
        file_id = pptr["m_FileID"]
        target = name
        if file_id:
            externals = list(self.env(name).files.values())[0].externals
            target = pathlib.Path(externals[file_id - 1].path).name
            if not (self.dir / target).exists():
                return None
        obj = next((o for o in self.env(target).objects if o.path_id == pptr["m_PathID"]), None)
        return (target, obj) if obj else None


def rgba(c: dict) -> list:
    return [round(c["r"], 4), round(c["g"], 4), round(c["b"], 4), round(c["a"], 4)]


def persistent_calls(value, names) -> list:
    """Every UnityEvent persistent call inside a component: [target class or name, method, string arg]."""
    out = []
    if isinstance(value, dict):
        if "m_MethodName" in value and value.get("m_MethodName"):
            target = value.get("m_Target") or {}
            out.append([names(target), value["m_MethodName"], (value.get("m_Arguments") or {}).get("m_StringArgument", "")])
        for v in value.values():
            out += persistent_calls(v, names)
    elif isinstance(value, list):
        for v in value:
            out += persistent_calls(v, names)
    return out


class SceneExporter:
    def __init__(self, files: Files, scene_file: str, pack: pathlib.Path):
        self.files, self.file, self.pack = files, scene_file, pack
        self.objs = files.objects(scene_file)
        self.classes = files.classes[scene_file]
        self.cache = {}
        self.sprites = {}
        (pack / "ui" / "sprites").mkdir(parents=True, exist_ok=True)

    def tree(self, pid):
        if pid not in self.cache:
            self.cache[pid] = self.objs[pid].read_typetree()
        return self.cache[pid]

    def target_name(self, pptr: dict) -> str:
        pid = pptr.get("m_PathID")
        if not pid or pptr.get("m_FileID") or pid not in self.objs:
            return ""
        obj = self.objs[pid]
        if obj.type.name == "MonoBehaviour":
            return self.classes.get(pid, "")
        if obj.type.name == "GameObject":
            return "GameObject:" + self.tree(pid).get("m_Name", "")
        return obj.type.name

    def sprite(self, pptr: dict):
        key = (pptr.get("m_FileID"), pptr.get("m_PathID"))
        if not key[1]:
            return None
        if key in self.sprites:
            return self.sprites[key]
        found = self.files.resolve(self.file, pptr)
        info = None
        if found:
            file_name, obj = found
            try:
                sprite = obj.read()
                name = f"{sprite.m_Name}_{file_name.split('.')[0]}_{obj.path_id}".replace(" ", "_")
                path = self.pack / "ui" / "sprites" / f"{name}.png"
                image = sprite.image
                if not path.exists():
                    image.save(path)
                border = sprite.m_Border
                info = {
                    "file": f"ui/sprites/{name}.png",
                    "size": [image.width, image.height],
                    "border": [border.x, border.y, border.z, border.w],  # left, bottom, right, top
                    "ppu": sprite.m_PixelsToUnits,
                }
            except Exception as e:
                info = {"error": f"{type(e).__name__}: {e}"}
        self.sprites[key] = info
        return info

    def font_name(self, pptr: dict) -> str:
        found = self.files.resolve(self.file, pptr)
        if not found:
            return ""
        try:
            return found[1].read(check_read=False).m_Name
        except Exception:
            return ""

    def node(self, transform_pid) -> dict:
        rt = self.tree(transform_pid)
        go = self.tree(rt["m_GameObject"]["m_PathID"])
        node = {"name": go.get("m_Name", ""), "active": bool(go.get("m_IsActive", 1))}
        if self.objs[transform_pid].type.name == "RectTransform":
            v2 = lambda k: [round(rt[k]["x"], 3), round(rt[k]["y"], 3)]
            node["rect"] = {
                "amin": v2("m_AnchorMin"), "amax": v2("m_AnchorMax"), "pos": v2("m_AnchoredPosition"),
                "size": v2("m_SizeDelta"), "pivot": v2("m_Pivot"),
                "scale": [round(rt["m_LocalScale"]["x"], 3), round(rt["m_LocalScale"]["y"], 3)],
            }
        scripts, events = [], []
        term = None
        for comp in go.get("m_Component", []):
            pid = comp["component"]["m_PathID"]
            obj = self.objs.get(pid)
            if obj is None or obj.type.name != "MonoBehaviour":
                if obj is not None and obj.type.name == "Canvas":
                    c = self.tree(pid)
                    node["canvas"] = {"sort": c.get("m_SortingOrder", 0), "override": bool(c.get("m_OverrideSorting", 0))}
                continue
            cls = self.classes.get(pid, "?")
            try:
                t = self.tree(pid)
            except Exception as e:
                scripts.append(cls)
                node.setdefault("errors", []).append(f"{cls}: {type(e).__name__}")
                continue
            enabled = bool(t.get("m_Enabled", 1))
            if cls == "Image" and enabled:
                node["image"] = {
                    "sprite": self.sprite(t.get("m_Sprite", {})), "color": rgba(t["m_Color"]),
                    "type": t.get("m_Type", 0), "preserve": bool(t.get("m_PreserveAspect", 0)),
                    "fill": t.get("m_FillAmount", 1.0), "raycast": bool(t.get("m_RaycastTarget", 1)),
                }
            elif cls == "RawImage" and enabled:
                node["image"] = {"sprite": None, "color": rgba(t["m_Color"]), "type": 0, "preserve": False, "fill": 1.0,
                                 "raycast": bool(t.get("m_RaycastTarget", 1))}
            elif cls in ("TextMeshProUGUI", "Text") and enabled:
                if cls == "TextMeshProUGUI":
                    node["text"] = {
                        "text": t.get("m_text", ""), "size": t.get("m_fontSize", 18), "color": rgba(t["m_fontColor"]),
                        "auto": [t.get("m_fontSizeMin", 10), t.get("m_fontSizeMax", 72)] if t.get("m_enableAutoSizing") else None,
                        "align": t.get("m_textAlignment", 257), "wrap": bool(t.get("m_enableWordWrapping", 1)),
                        "style": t.get("m_fontStyle", 0), "font": self.font_name(t.get("m_fontAsset", {})),
                    }
                else:
                    fd = t.get("m_FontData", {})
                    node["text"] = {"text": t.get("m_Text", ""), "size": fd.get("m_FontSize", 14), "color": rgba(t["m_Color"]),
                                    "auto": None, "align": fd.get("m_Alignment", 0), "wrap": True, "style": fd.get("m_FontStyle", 0),
                                    "font": "legacy"}
            elif cls == "Localize":
                term = t.get("mTerm") or None
            elif cls == "CanvasGroup":
                node["alpha"] = t.get("m_Alpha", 1.0)
            elif cls in ("VerticalLayoutGroup", "HorizontalLayoutGroup") and enabled:
                pad = t.get("m_Padding", {})
                node["layout"] = {
                    "axis": "v" if cls.startswith("Vertical") else "h",
                    "padding": [pad.get("m_Left", 0), pad.get("m_Right", 0), pad.get("m_Top", 0), pad.get("m_Bottom", 0)],
                    "spacing": t.get("m_Spacing", 0.0), "align": t.get("m_ChildAlignment", 0),
                    "control": [bool(t.get("m_ChildControlWidth", 1)), bool(t.get("m_ChildControlHeight", 1))],
                    "expand": [bool(t.get("m_ChildForceExpandWidth", 1)), bool(t.get("m_ChildForceExpandHeight", 1))],
                }
            elif cls == "LayoutElement" and enabled:
                node["element"] = {
                    "ignore": bool(t.get("m_IgnoreLayout", 0)),
                    "min": [t.get("m_MinWidth", -1), t.get("m_MinHeight", -1)],
                    "preferred": [t.get("m_PreferredWidth", -1), t.get("m_PreferredHeight", -1)],
                    "flexible": [t.get("m_FlexibleWidth", -1), t.get("m_FlexibleHeight", -1)],
                }
            if cls not in ("Image", "RawImage", "TextMeshProUGUI", "Text", "Localize", "LayoutElement"):
                scripts.append(cls if enabled else f"{cls}(disabled)")
            calls = persistent_calls({k: v for k, v in t.items() if k not in SKIP_FIELDS}, self.target_name)
            for target, method, arg in calls:
                events.append({"on": cls, "target": target, "method": method, "arg": arg})
        if term and "text" in node:
            node["text"]["term"] = term
        if scripts:
            node["scripts"] = scripts
        if events:
            node["events"] = events
        children = [self.node(c["m_PathID"]) for c in rt.get("m_Children", []) if c["m_PathID"] in self.objs]
        if children:
            node["children"] = children
        return node

    def export(self, scene_name: str) -> dict:
        roots = []
        for pid, obj in self.objs.items():
            if obj.type.name in ("Transform", "RectTransform") and not self.tree(pid)["m_Father"]["m_PathID"]:
                roots.append(pid)
        canvases = []
        for pid in roots:
            node = self.node(pid)
            canvases += list(find_canvases(node))
        scaler = {"reference": [1920, 1080], "match": 0.5, "mode": 1}
        for pid, cls in self.classes.items():
            if cls == "CanvasScaler" and pid in self.objs:
                s = self.tree(pid)
                scaler = {"reference": [s["m_ReferenceResolution"]["x"], s["m_ReferenceResolution"]["y"]],
                          "match": s.get("m_MatchWidthOrHeight", 0.5), "mode": s.get("m_ScreenMatchMode", 0)}
                break
        doc = {"scene": scene_name, "scaler": scaler, "canvases": canvases}
        out = self.pack / "ui" / f"{scene_name}.json"
        out.write_text(json.dumps(doc, separators=(",", ":")))
        return doc


def find_canvases(node: dict):
    """Top-level canvases (nested canvases stay inside their parent's tree)."""
    if "canvas" in node and "rect" in node:
        yield node
        return
    for child in node.get("children", []):
        yield from find_canvases(child)


def export_loc(files: Files, pack: pathlib.Path):
    raw = None
    for o in files.env("resources.assets").objects:
        if files.classes["resources.assets"].get(o.path_id) == "LanguageSource":
            raw = o.get_raw_data()
            break
    loc = i2loc.read(raw)
    out = pack / "loc"
    out.mkdir(parents=True, exist_ok=True)
    codes = []
    for i, lang in enumerate(loc["languages"]):
        if not lang["code"]:
            continue
        table = {k: v[i] for k, v in loc["terms"].items() if v[i]}
        (out / f"{lang['code']}.json").write_text(json.dumps(table, ensure_ascii=False, separators=(",", ":")), encoding="utf-8")
        codes.append({"code": lang["code"], "name": lang["name"], "terms": len(table)})
    (out / "languages.json").write_text(json.dumps(codes, ensure_ascii=False))
    return codes


# The game's TMP fonts ship only as SDF atlases. Titillium Web and Fjalla One are open (OFL) Google Fonts,
# fetched onto the user's machine; Beholder and Cone have no open source, so they map to the closest
# open font until the SDF atlases are rendered directly.
OPEN_FONTS = {
    "TitilliumWeb-Regular.ttf": "ofl/titilliumweb/TitilliumWeb-Regular.ttf",
    "TitilliumWeb-Bold.ttf": "ofl/titilliumweb/TitilliumWeb-Bold.ttf",
    "FjallaOne-Regular.ttf": "ofl/fjallaone/FjallaOne-Regular.ttf",
}
FONT_MAP = {
    "Titillium-Regular SDF": "fonts/TitilliumWeb-Regular.ttf",
    "FjallaOne-Regular SDF": "fonts/FjallaOne-Regular.ttf",
    "Beholder SDF": "fonts/FjallaOne-Regular.ttf",
    "Cone SDF": "fonts/TitilliumWeb-Bold.ttf",
    "": "fonts/TitilliumWeb-Regular.ttf",
}


def export_fonts(pack: pathlib.Path):
    import urllib.request
    out = pack / "fonts"
    out.mkdir(parents=True, exist_ok=True)
    for name, path in OPEN_FONTS.items():
        if not (out / name).exists():
            with urllib.request.urlopen(f"https://raw.githubusercontent.com/google/fonts/main/{path}") as r:
                (out / name).write_bytes(r.read())
    (out / "fonts.json").write_text(json.dumps(FONT_MAP, indent=1))


def count(node: dict) -> int:
    return 1 + sum(count(c) for c in node.get("children", []))


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("data_dir", type=pathlib.Path)
    ap.add_argument("--pack", type=pathlib.Path, required=True)
    ap.add_argument("--scene", action="append", default=[], help="levelN=Name")
    ap.add_argument("--loc", action="store_true")
    ap.add_argument("--fonts", action="store_true")
    args = ap.parse_args()

    if args.fonts:
        export_fonts(args.pack)

    gen = TypeTreeGenerator("2017.4.19f1")
    gen.load_local_dll_folder(str(args.data_dir / "Managed"))
    files = Files(args.data_dir, gen)
    if args.loc:
        for lang in export_loc(files, args.pack):
            print(f"loc {lang['code']}: {lang['terms']} terms")
    for spec in args.scene:
        level, name = spec.split("=")
        doc = SceneExporter(files, level, args.pack).export(name)
        nodes = sum(count(c) for c in doc["canvases"])
        print(f"ui {name}: {len(doc['canvases'])} canvases, {nodes} nodes")
    return 0


if __name__ == "__main__":
    sys.exit(main())
