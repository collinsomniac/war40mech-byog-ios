"""Battle rooms: each battle scene (named after its RoomData) becomes
  models/battle/<Room>.glb   the scene's static 3D; entity subtrees left out (they spawn from their prefabs),
                             template-owned nodes tagged {"template": n} so the app shows only the chosen one
  battle/<Room>.json         room cells, and per template: the walkable grid, entity placements, deploy areas.

The game builds its grid at load time (GridManager.Create1x1Grid): ten 1-unit nodes per room cell, a ray
from above finds the ground (layers Ground/Hole), and a ray up from the ground finds blockers
(NotWalkable/Hole). That scan is reproduced here against the scene's own colliders.

Coordinates in the JSON are Unity's (left-handed, metres); the app mirrors x like the glTF export does.
"""
import argparse
import json
import math
import pathlib
import sys

import numpy as np
import UnityPy

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))  # sibling modules under python -I
import export_prefab  # noqa: E402
from UnityPy.helpers.MeshHelper import MeshHandler  # noqa: E402

GROUND, OBSTACLE4, HOLE, NOT_WALKABLE = 10, 16, 17, 18
CELL_TILES = 10
BATTLE_FIRST_LEVEL = 20  # BuildSettings: Tuto_Battle_1 onwards are battle rooms


def trs(t) -> np.ndarray:
    q, s, p = t.m_LocalRotation, t.m_LocalScale, t.m_LocalPosition
    x, y, z, w = q.x, q.y, q.z, q.w
    r = np.array([
        [1 - 2 * (y * y + z * z), 2 * (x * y - z * w), 2 * (x * z + y * w)],
        [2 * (x * y + z * w), 1 - 2 * (x * x + z * z), 2 * (y * z - x * w)],
        [2 * (x * z - y * w), 2 * (y * z + x * w), 1 - 2 * (x * x + y * y)],
    ])
    m = np.eye(4)
    m[:3, :3] = r * np.array([s.x, s.y, s.z])
    m[:3, 3] = [p.x, p.y, p.z]
    return m


class Scene:
    def __init__(self, env):
        self.objects = {o.path_id: o for o in env.objects}
        self.transforms = {}
        self.go_transform = {}
        for obj in env.objects:
            if obj.type.name in ("Transform", "RectTransform"):
                t = obj.read()
                self.transforms[obj.path_id] = t
                self.go_transform[t.m_GameObject.path_id] = obj.path_id
        self.parent = {pid: t.m_Father.path_id for pid, t in self.transforms.items()}
        self.world = {}
        self.scripts = {}  # gameobject -> [(class, monobehaviour path_id)]
        for obj in env.objects:
            if obj.type.name != "MonoBehaviour":
                continue
            try:
                mb = obj.read(check_read=False)
                cls = mb.m_Script.read().m_ClassName
            except Exception:
                continue
            self.scripts.setdefault(mb.m_GameObject.path_id, []).append((cls, obj.path_id))
        self._go = {}

    def matrix(self, tid) -> np.ndarray:
        if tid not in self.world:
            parent = self.parent.get(tid) or 0
            local = trs(self.transforms[tid])
            self.world[tid] = (self.matrix(parent) @ local) if parent in self.transforms else local
        return self.world[tid]

    def go_matrix(self, go_id) -> np.ndarray:
        return self.matrix(self.go_transform[go_id])

    def ancestors(self, go_id):
        tid = self.go_transform.get(go_id)
        while tid in self.transforms:
            yield self.transforms[tid].m_GameObject.path_id
            tid = self.parent.get(tid)

    def gameobject(self, go_id):
        if go_id not in self._go:
            obj = self.objects.get(go_id)
            self._go[go_id] = obj.read() if obj is not None else None
        return self._go[go_id]

    def active(self, go_id) -> bool:
        return all((g := self.gameobject(a)) is None or g.m_IsActive for a in self.ancestors(go_id))


class Colliders:
    """Colliders on the grid layers, for vertical-ray queries over many points at once."""

    def __init__(self):
        self.boxes = []  # (layer, world matrix 4x4, inverse, center, half extents)
        self.tris = {}   # layer -> [T x 3 x 3 arrays]

    def add_box(self, layer, m, center, size):
        try:
            inv = np.linalg.inv(m)
        except np.linalg.LinAlgError:
            return
        self.boxes.append((layer, m, inv, np.array([center.x, center.y, center.z]),
                           np.abs(np.array([size.x, size.y, size.z])) / 2))

    def add_mesh(self, layer, m, tris: np.ndarray):
        if len(tris):
            self.tris.setdefault(layer, []).append(tris @ m[:3, :3].T + m[:3, 3])

    def heights(self, xs, zs, layers):
        """Heights (NaN where missed) at which vertical lines at (xs, zs) cross colliders: [(y, layer)].
        Meshes report their highest and lowest crossing per point."""
        out = []
        n = len(xs)
        top = np.stack([xs, np.full(n, 1000.0), zs, np.ones(n)], axis=1)
        bottom = np.stack([xs, np.full(n, -1000.0), zs, np.ones(n)], axis=1)
        for layer, m, inv, center, half in self.boxes:
            if layer not in layers:
                continue
            o = (top @ inv.T)[:, :3]
            d = (bottom @ inv.T)[:, :3] - o
            lo, hi = center - half, center + half
            flat = np.abs(d) < 1e-9
            safe = np.where(flat, 1.0, d)
            ta, tb = (lo - o) / safe, (hi - o) / safe
            inside = (o >= lo) & (o <= hi)
            near = np.where(flat, np.where(inside, -np.inf, np.inf), np.minimum(ta, tb))
            far = np.where(flat, np.where(inside, np.inf, -np.inf), np.maximum(ta, tb))
            t0 = np.maximum(near.max(axis=1), 0.0)
            t1 = np.minimum(far.min(axis=1), 1.0)
            hit = t0 <= t1
            if not hit.any():
                continue
            for t in (t0, t1):
                p = o + d * t[:, None]
                y = p @ m[1, :3] + m[1, 3]
                out.append((np.where(hit, y, np.nan), layer))
        for layer, chunks in self.tris.items():
            if layer not in layers:
                continue
            for arr in chunks:
                for start in range(0, len(arr), 1024):
                    t = arr[start:start + 1024]
                    a, b, c = t[:, 0], t[:, 1], t[:, 2]
                    det = (b[:, 0] - a[:, 0]) * (c[:, 2] - a[:, 2]) - (c[:, 0] - a[:, 0]) * (b[:, 2] - a[:, 2])
                    ok = np.abs(det) > 1e-12
                    if not ok.any():
                        continue
                    det = np.where(ok, det, 1.0)[:, None]
                    px = xs[None, :] - a[:, 0:1]
                    pz = zs[None, :] - a[:, 2:3]
                    u = (px * (c[:, 2:3] - a[:, 2:3]) - (c[:, 0:1] - a[:, 0:1]) * pz) / det
                    v = ((b[:, 0:1] - a[:, 0:1]) * pz - px * (b[:, 2:3] - a[:, 2:3])) / det
                    inside = ok[:, None] & (u >= -1e-6) & (v >= -1e-6) & (u + v <= 1 + 1e-6)
                    rows = np.any(inside, axis=1)
                    if not rows.any():
                        continue
                    y = a[rows, 1:2] + u[rows] * (b[rows, 1:2] - a[rows, 1:2]) + v[rows] * (c[rows, 1:2] - a[rows, 1:2])
                    y_hi = np.where(inside[rows], y, -np.inf).max(axis=0)
                    y_lo = np.where(inside[rows], y, np.inf).min(axis=0)
                    out.append((np.where(np.isfinite(y_hi), y_hi, np.nan), layer))
                    out.append((np.where(np.isfinite(y_lo), y_lo, np.nan), layer))
        return out


def mesh_triangles(mesh) -> np.ndarray:
    handler = MeshHandler(mesh)
    handler.process()
    if not handler.m_Vertices:
        return np.zeros((0, 3, 3))
    verts = np.array([v[:3] for v in handler.m_Vertices], dtype=np.float64)
    idx = [tri for sub in handler.get_triangles() for tri in sub]
    return verts[np.array(idx)] if idx else np.zeros((0, 3, 3))


def entity_class(scripts):
    """The entity data component (…DataMb) on a GameObject, if any."""
    for cls, pid in scripts:
        if cls.endswith("DataMb") and cls not in ("AreaSpawnDataMb", "AreaTriggerAllDataMb"):
            return cls, pid
    return None


def scan(cells, colliders) -> dict:
    """GridManager.Create1x1Grid and the A* GridGraph scan: per node, ground height and walkability."""
    xs_ = [c["x"] for c in cells]
    ys_ = [c["y"] for c in cells]
    min_x, max_x, min_y, max_y = min(xs_), max(xs_), min(ys_), max(ys_)
    width = int((max_x - min_x + 1) * CELL_TILES)
    depth = int((max_y - min_y + 1) * CELL_TILES)
    origin_x = (min_x + max_x) / 2 * CELL_TILES - width / 2
    origin_z = (min_y + max_y) / 2 * CELL_TILES - depth / 2
    jj, ii = np.meshgrid(np.arange(depth), np.arange(width), indexing="ij")
    xs = (origin_x + ii + 0.5).ravel().astype(np.float64)
    zs = (origin_z + jj + 0.5).ravel().astype(np.float64)
    ground = np.full(len(xs), -np.inf)
    hole = np.zeros(len(xs), dtype=bool)
    for c in colliders:
        for y, layer in c.heights(xs, zs, (GROUND, HOLE)):
            y = np.where(y <= 100.0, y, np.nan)
            better = np.nan_to_num(y, nan=-np.inf) > ground
            ground = np.where(better, y, ground)
            hole = np.where(better, layer == HOLE, hole)
    blocked = np.zeros(len(xs), dtype=bool)
    for c in colliders:
        for y, layer in c.heights(xs, zs, (NOT_WALKABLE, HOLE)):
            with np.errstate(invalid="ignore"):
                blocked |= (y > ground + 0.01) & (y <= ground + 100.0)
    has = np.isfinite(ground)
    kinds = np.where(~has, 0, np.where(hole, 2, np.where(blocked, 3, 1)))  # 0 none, 1 walk, 2 hole, 3 blocked
    return {"width": width, "depth": depth, "origin": [origin_x, origin_z],
            "heights": [round(float(h), 3) if np.isfinite(h) else None for h in ground],
            "kinds": [int(k) for k in kinds]}


def area_cells(scene, container_go) -> list:
    """GridManager.GetGridNodeCoordinates: node centres covered by the container's mesh renderers' bounds."""
    out = []
    stack = [container_go]
    while stack:
        g = stack.pop()
        tid = scene.go_transform.get(g)
        if tid is None:
            continue
        stack.extend(c.read().m_GameObject.path_id for c in scene.transforms[tid].m_Children)
        go = scene.gameobject(g)
        comps = {c.component.type.name: c.component for c in go.m_Component} if go else {}
        if "MeshFilter" not in comps or "MeshRenderer" not in comps:
            continue
        try:
            aabb = comps["MeshFilter"].read().m_Mesh.read().m_LocalAABB
        except Exception:
            continue
        m = scene.go_matrix(g)
        c, e = aabb.m_Center, aabb.m_Extent
        corners = np.array([[c.x + sx * e.x, c.y + sy * e.y, c.z + sz * e.z, 1.0]
                            for sx in (-1, 1) for sy in (-1, 1) for sz in (-1, 1)]) @ m.T
        lo, hi = corners.min(axis=0), corners.max(axis=0)
        x = lo[0] + 0.5
        while x < hi[0]:
            z = lo[2] + 0.5
            while z < hi[2]:
                out.append([round(float(x), 2), round(float(z), 2)])
                z += 1.0
            x += 1.0
    return out


def camera_rig(scene, typetree_of):
    """The scene's CameraContainer rig (BulwarkStudios.Engine3D.CameraContainer): container position, pivot
    angles, the camera's zoom (local z) and field of view, zoom limits, and RoomCameraBounds' floor box."""
    rig = None
    for g, scripts in scene.scripts.items():
        for cls, pid in scripts:
            if cls != "CameraContainer":
                continue
            t = typetree_of(pid)
            if t is None:
                continue
            pivot_t = scene.transforms.get(t["pivot"]["m_PathID"])
            cam = typetree_of(t["mainCamera"]["m_PathID"]) or {}
            cam_go = cam.get("m_GameObject", {}).get("m_PathID")
            cam_t = scene.transforms.get(scene.go_transform.get(cam_go))
            container = scene.go_matrix(g)[:3, 3]
            rig = {
                "position": [round(float(v), 3) for v in container],
                "pivot_euler": euler(pivot_t.m_LocalRotation) if pivot_t else [45.0, 0.0, 0.0],
                "zoom": round(float(cam_t.m_LocalPosition.z), 3) if cam_t else -20.0,
                "fov": cam.get("field of view", 30.0),
                "zoom_min": t["zoomMin"], "zoom_max": t["zoomMax"], "zoom_cinematic": t["zoomCinematic"],
                "zoom_max_fov": t["zoomMaxFoV"], "x_rotation_zoom": t["xRotationZoom"],
            }
    if rig is None:
        return None
    for g, scripts in scene.scripts.items():
        for cls, pid in scripts:
            if cls != "RoomCameraBounds":
                continue
            t = typetree_of(pid) or {}
            pts = []
            for key in ("pt1", "pt2"):
                go = t.get(key, {}).get("m_PathID")
                tid = scene.go_transform.get(go, go)
                if tid in scene.transforms:
                    pts.append(scene.matrix(tid)[:3, 3])
            if len(pts) == 2:
                rig["bounds"] = [[round(float(min(a, b)), 3) for a, b in zip(*pts)],
                                 [round(float(max(a, b)), 3) for a, b in zip(*pts)]]
    return rig


def render_settings(env, scene, typetree_of) -> dict:
    """The scene's lighting environment: ambient from RenderSettings' baked ambient probe (spherical
    harmonics L0 per channel), and the SSMS/Global height fog that blackens everything below the floor."""
    out = {}
    for obj in env.objects:
        if obj.type.name == "RenderSettings":
            t = obj.read_typetree()
            sh = t.get("m_AmbientProbe", {})
            l0 = [sh.get("sh[ 0]", 0.0), sh.get("sh[ 9]", 0.0), sh.get("sh[18]", 0.0)]
            out["ambient_sh0"] = [round(float(v), 6) for v in l0]
            out["ambient_intensity"] = t.get("m_AmbientIntensity", 1.0)
    for g, scripts in scene.scripts.items():
        for cls, pid in scripts:
            if cls in ("SSMSGlobalFog", "GlobalFog") and "height_fog" not in out:
                t = typetree_of(pid) or {}
                if t.get("heightFog"):
                    tint = t.get("fogTint", {"r": 0.0, "g": 0.0, "b": 0.0})
                    out["height_fog"] = {"height": t.get("height", 0.0), "density": t.get("heightDensity", 1.0),
                                         "max_density": t.get("maxDensity", 1.0),
                                         "color": [tint.get("r", 0.0), tint.get("g", 0.0), tint.get("b", 0.0)]}
    return out


def euler(q) -> list:
    """Unity quaternion -> Unity euler degrees (ZXY: x pitch, y yaw, z roll)."""
    x, y, z, w = q.x, q.y, q.z, q.w
    pitch = math.degrees(math.asin(max(-1.0, min(1.0, 2 * (w * x - y * z)))))
    yaw = math.degrees(math.atan2(2 * (w * y + x * z), 1 - 2 * (x * x + y * y)))
    roll = math.degrees(math.atan2(2 * (w * z + x * y), 1 - 2 * (x * x + z * z)))
    return [round(pitch, 3), round(yaw, 3), round(roll, 3)]


def placement(scene, go_id) -> dict:
    m = scene.go_matrix(go_id)
    return {"pos": [round(float(p), 3) for p in m[:3, 3]], "yaw": round(math.degrees(math.atan2(m[0, 2], m[2, 2])), 1)}


def export_room(data_dir, level, scene_name, data, exporter, glb: bool):
    env = UnityPy.load(str(data_dir / level))
    scene = Scene(env)
    templates = {g: pid for g, s in scene.scripts.items() for c, pid in s if c == "RoomTemplate"}
    entities = {g: entity_class(s) for g, s in scene.scripts.items() if entity_class(s)}

    def owner(go_id):
        return next((a for a in scene.ancestors(go_id) if a in templates), None)

    def nested(go_id):
        """Inside another entity (a part of a bigger unit)."""
        return any(a in entities for a in list(scene.ancestors(go_id))[1:])

    base = Colliders()
    per_template = {g: Colliders() for g in templates}
    for obj in env.objects:
        if obj.type.name not in ("BoxCollider", "MeshCollider"):
            continue
        try:
            col = obj.read()
        except Exception:
            continue
        go_id = col.m_GameObject.path_id
        go = scene.gameobject(go_id)
        if go is None or not getattr(col, "m_Enabled", True) or go.m_Layer not in (GROUND, HOLE, NOT_WALKABLE, OBSTACLE4):
            continue
        if any(a in entities for a in scene.ancestors(go_id)):
            continue
        tpl = owner(go_id)
        if tpl is None and not scene.active(go_id):
            continue
        target = per_template[tpl] if tpl is not None else base
        m = scene.go_matrix(go_id)
        if obj.type.name == "BoxCollider":
            target.add_box(go.m_Layer, m, col.m_Center, col.m_Size)
        elif col.m_Mesh.path_id:
            try:
                target.add_mesh(go.m_Layer, m, mesh_triangles(col.m_Mesh.read()))
            except Exception:
                pass

    room = data["RoomData"].get(scene_name)
    cells = room["fields"]["cells"] if room else [{"x": 0.0, "y": 0.0}]
    template_index = {}
    for go_id, mb in templates.items():
        rec = data["RoomTemplate"].get((level, mb))
        template_index[go_id] = rec["odin"]["data"]["template"] if rec and rec.get("odin") else 0

    out_templates = []
    for go_id in templates:
        ents = [{"class": entities[g][0], "data": entities[g][1], **placement(scene, g)}
                for g in entities if owner(g) == go_id and not nested(g)]
        spawns = []
        for a in data["AreaSpawnDataMb"].get(level, []):
            g = a["fields"]["m_GameObject"]["m_PathID"]
            if owner(g) not in (go_id, None):
                continue
            spawns.append({"start": a["fields"].get("startPosition", 0),
                           "reinforcement": bool(a["fields"].get("forReinforcement", 0)),
                           "cells": area_cells(scene, a["fields"]["boundsContainer"]["m_PathID"])})
        # AreaSpawnEnemy markers: where TaskBattleSpawnEnemies places the mission's random Necrons. A marker
        # inside an entity (a node container, e.g. a sarcophagus group) spawns from that container's budget.
        enemy_spawns = []
        for g, scripts in scene.scripts.items():
            if not any(c == "AreaSpawnEnemy" for c, _ in scripts) or owner(g) not in (go_id, None):
                continue
            container = next((a for a in list(scene.ancestors(g))[1:] if a in entities), None)
            enemy_spawns.append({**placement(scene, g), "container": container})
        out_templates.append({"template": template_index[go_id], "grid": scan(cells, [base, per_template[go_id]]),
                              "entities": ents, "spawns": spawns, "enemy_spawns": enemy_spawns})
    shared = [{"class": entities[g][0], "data": entities[g][1], **placement(scene, g)}
              for g in entities if owner(g) is None and scene.active(g) and not nested(g)]
    # A second load with the generated type trees, for MonoBehaviour fields (the rig's settings).
    typed = UnityPy.load(str(data_dir / level))
    typed.typetree_generator = data.get("generator")
    typed_objects = {o.path_id: o for o in typed.objects}

    def typetree_of(pid):
        obj = typed_objects.get(pid)
        if obj is None:
            return None
        try:
            return obj.read_typetree(check_read=False)
        except Exception:
            return None

    doc = {"room": scene_name, "level": level, "cells": cells, "shared_entities": shared,
           "templates": sorted(out_templates, key=lambda t: t["template"]), "camera": camera_rig(scene, typetree_of),
           "render": render_settings(env, scene, typetree_of)}
    stats = {"templates": len(out_templates), "entities": sum(len(t["entities"]) for t in out_templates) + len(shared)}
    if glb:
        stats.update(export_static(exporter, scene, scene_name, set(entities),
                                   {g: template_index[g] for g in templates}, owner))
    return doc, stats


def export_static(exporter, scene, name, skip, template_of, owner) -> dict:
    gltf = export_prefab.Gltf()
    stats = {"meshes": 0, "skipped_skinned": 0, "clips": 0, "roots": 0}
    exporter.animators = []
    original = exporter.node

    def node(gltf_, transform, stats_):
        if transform.m_GameObject.path_id in skip:
            gltf_.doc["nodes"].append({"name": "entity"})
            return len(gltf_.doc["nodes"]) - 1
        return original(gltf_, transform, stats_)

    def extras(go_id, batched):
        if batched:
            tpl = owner(go_id)
            return {"template": template_of[tpl]} if tpl is not None else None
        return {"template": template_of[go_id]} if go_id in template_of else None

    exporter.node, exporter.node_extras = node, extras
    try:
        for t in scene.transforms.values():
            if t.m_Father.path_id:
                continue
            try:
                gltf.doc["scenes"][0]["nodes"].append(exporter.node(gltf, t, stats))
                stats["roots"] += 1
            except Exception as e:
                stats.setdefault("errors", []).append(f"{type(e).__name__}: {e}"[:200])
    finally:
        exporter.node, exporter.node_extras = original, None
    exporter.animations(gltf, stats)
    out = exporter.models_dir / "battle" / f"{name}.glb"
    gltf.write(out)
    stats["bytes"] = out.stat().st_size
    return stats


def scene_names(data_dir) -> dict:
    env = UnityPy.load(str(data_dir / "globalgamemanagers"))
    for obj in env.objects:
        if obj.type.name == "BuildSettings":
            return {f"level{i}": s.split("/")[-1].rsplit(".", 1)[0] for i, s in enumerate(obj.read_typetree()["scenes"])}
    return {}


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("data_dir", type=pathlib.Path)
    ap.add_argument("--pack", type=pathlib.Path, required=True)
    ap.add_argument("--scene", action="append", default=[], help="levelN (default: every battle scene)")
    ap.add_argument("--no-glb", action="store_true")
    args = ap.parse_args()

    raw = lambda cls: json.loads((args.pack / "data" / f"{cls}.json").read_text())  # noqa: E731
    data = {"RoomData": {r["fields"]["m_Name"]: r for r in raw("RoomData")},
            "RoomTemplate": {(r["file"], r["path_id"]): r for r in raw("RoomTemplate")},
            "AreaSpawnDataMb": {}}
    for a in raw("AreaSpawnDataMb"):
        data["AreaSpawnDataMb"].setdefault(a["file"], []).append(a)
    from UnityPy.helpers.TypeTreeGenerator import TypeTreeGenerator
    gen = TypeTreeGenerator("2017.4.19f1")
    gen.load_local_dll_folder(str(args.data_dir / "Managed"))
    data["generator"] = gen
    names = scene_names(args.data_dir)
    levels = args.scene or [lv for lv in names if int(lv[5:]) >= BATTLE_FIRST_LEVEL]
    out_dir = args.pack / "battle"
    out_dir.mkdir(parents=True, exist_ok=True)
    exporter = export_prefab.Exporter(args.pack)
    (exporter.models_dir / "battle").mkdir(parents=True, exist_ok=True)
    summary_path = out_dir / "_summary.json"
    summary = json.loads(summary_path.read_text()) if summary_path.exists() else {}
    for level in levels:
        name = names.get(level, level)
        try:
            doc, stats = export_room(args.data_dir, level, name, data, exporter, not args.no_glb)
        except Exception as e:
            summary[name] = f"{type(e).__name__}: {e}"
            print(name, summary[name], file=sys.stderr, flush=True)
            continue
        (out_dir / f"{name}.json").write_text(json.dumps(doc))
        summary[name] = stats
        print(name, stats, flush=True)
        summary_path.write_text(json.dumps(summary, indent=1))
    return 0


if __name__ == "__main__":
    sys.exit(main())
