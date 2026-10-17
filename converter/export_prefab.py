"""Export prefabs (GameObject hierarchies) from the user's install as .glb into the content pack.

Static MeshRenderer parts only for now; skinned meshes and animation come in a later pass.
Textures are written once to <pack>/models/textures/ and referenced by relative URI, so models share them.

Unity is left-handed (Y up), glTF right-handed (Y up): X is mirrored, which also flips triangle winding,
quaternion Y/Z and tangent handedness. Unity UV origin is bottom-left, glTF top-left.
"""
import argparse
import math
import json
import pathlib
import struct
import sys

import UnityPy

sys.path.insert(0, str(pathlib.Path(__file__).resolve().parent))  # sibling modules under python -I
from PIL import Image, ImageStat
from UnityPy.helpers.MeshHelper import MeshHandler

import anim

FLOAT, UINT32 = 5126, 5125
ARRAY_BUFFER, ELEMENT_ARRAY_BUFFER = 34962, 34963


class Gltf:
    def __init__(self):
        self.doc = {"asset": {"version": "2.0", "generator": "war40mech-byog converter"},
                    "scene": 0, "scenes": [{"nodes": []}], "nodes": [], "meshes": [], "materials": [],
                    "textures": [], "images": [], "samplers": [{"magFilter": 9729, "minFilter": 9987}],
                    "accessors": [], "bufferViews": [], "buffers": []}
        self.blob = bytearray()
        self.material_index: dict[int, int] = {}
        self.image_index: dict[str, int] = {}

    def _view(self, data: bytes, target: int | None) -> int:
        while len(self.blob) % 4:
            self.blob.append(0)
        view = {"buffer": 0, "byteOffset": len(self.blob), "byteLength": len(data)}
        if target is not None:
            view["target"] = target
        self.doc["bufferViews"].append(view)
        self.blob += data
        return len(self.doc["bufferViews"]) - 1

    def accessor(self, rows: list, kind: str, minmax: bool = False, indices: bool = False,
                 animation: bool = False) -> int:
        """Vertex data by default; `indices` for triangle lists; `animation` for sampler input/output."""
        width = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}[kind]
        flat = [c for row in rows for c in (row if width > 1 else (row,))]
        if indices:
            data, ctype, target = struct.pack(f"<{len(flat)}I", *flat), UINT32, ELEMENT_ARRAY_BUFFER
        else:
            data, ctype = struct.pack(f"<{len(flat)}f", *flat), FLOAT
            target = None if animation else ARRAY_BUFFER
        if width == 1:
            rows = [(r,) for r in rows]
        acc = {"bufferView": self._view(data, target), "componentType": ctype, "count": len(rows), "type": kind}
        if minmax:
            acc["min"] = [min(r[i] for r in rows) for i in range(width)]
            acc["max"] = [max(r[i] for r in rows) for i in range(width)]
        self.doc["accessors"].append(acc)
        return len(self.doc["accessors"]) - 1

    def texture(self, uri: str) -> int:
        if uri not in self.image_index:
            self.doc["images"].append({"uri": uri})
            self.doc["textures"].append({"source": len(self.doc["images"]) - 1, "sampler": 0})
            self.image_index[uri] = len(self.doc["textures"]) - 1
        return self.image_index[uri]

    def write(self, path: pathlib.Path) -> None:
        self.doc["buffers"] = [{"byteLength": len(self.blob)}]
        doc = {k: v for k, v in self.doc.items() if v != []}
        body = json.dumps(doc, separators=(",", ":")).encode()
        body += b" " * (-len(body) % 4)
        while len(self.blob) % 4:
            self.blob.append(0)
        out = struct.pack("<III", 0x46546C67, 2, 12 + 8 + len(body) + 8 + len(self.blob))
        out += struct.pack("<II", len(body), 0x4E4F534A) + body
        out += struct.pack("<II", len(self.blob), 0x004E4942) + bytes(self.blob)
        path.write_bytes(out)


class Exporter:
    def __init__(self, pack: pathlib.Path):
        self.textures_dir = pack / "models" / "textures"
        self.models_dir = pack / "models"
        self.textures_dir.mkdir(parents=True, exist_ok=True)
        self.models_dir.mkdir(parents=True, exist_ok=True)
        self.written_textures: set[str] = set()

    # --- textures -------------------------------------------------------------------------------------------

    def _save(self, name: str, image: Image.Image) -> str:
        filename = f"{name}.png"
        if filename not in self.written_textures:
            image.save(self.textures_dir / filename, optimize=False)
            self.written_textures.add(filename)
        return f"textures/{filename}"

    def albedo(self, tex) -> str:
        return self._save(tex.m_Name, tex.image.convert("RGBA"))

    def normal(self, tex) -> str:
        image = tex.image.convert("RGBA")
        r, g, b, a = image.split()
        # DXT5nm stores X in alpha with red saturated; rebuild a standard tangent-space normal map.
        if ImageStat.Stat(r.resize((32, 32))).mean[0] > 250 and ImageStat.Stat(a.resize((32, 32))).stddev[0] > 1:
            x, y = a, g
            z = Image.new("L", image.size)
            z.putdata([round((max(0.0, 1 - (xv / 127.5 - 1) ** 2 - (yv / 127.5 - 1) ** 2) ** 0.5 + 1) * 127.5)
                       for xv, yv in zip(x.getdata(), y.getdata())])
            image = Image.merge("RGB", (x, y, z))
        else:
            image = Image.merge("RGB", (r, g, b))
        return self._save(f"{tex.m_Name}.normal", image)

    def packed_orm(self, tex) -> tuple[str, str]:
        """Mechanicus packs metallic (R), AO (G), emission mask (B), smoothness (A).
        glTF wants occlusion (R), roughness (G), metallic (B). Emission mask is kept as its own texture."""
        metal, ao, emission, smooth = tex.image.convert("RGBA").split()
        rough = Image.eval(smooth, lambda v: 255 - v)
        orm = self._save(f"{tex.m_Name}.orm", Image.merge("RGB", (ao, rough, metal)))
        mask = self._save(f"{tex.m_Name}.emission", Image.merge("RGB", (emission, emission, emission)))
        return orm, mask

    # --- materials ------------------------------------------------------------------------------------------

    def material(self, gltf: Gltf, mat_ptr) -> int:
        if mat_ptr.path_id in gltf.material_index:
            return gltf.material_index[mat_ptr.path_id]
        mat = mat_ptr.read()
        shader = mat.m_Shader.read()
        shader_name = shader.m_ParsedForm.m_Name if hasattr(shader, "m_ParsedForm") else shader.m_Name
        texs = {k: v.m_Texture for k, v in mat.m_SavedProperties.m_TexEnvs if v.m_Texture.path_id}
        colors = {k: v for k, v in mat.m_SavedProperties.m_Colors}
        floats = {k: v for k, v in mat.m_SavedProperties.m_Floats}

        pbr = {"metallicFactor": 1.0, "roughnessFactor": 1.0}
        entry = {"name": mat.m_Name, "pbrMetallicRoughness": pbr,
                 "extras": {"unity_shader": shader_name, "floats": floats,
                            "colors": {k: [c.r, c.g, c.b, c.a] for k, c in colors.items()}}}
        if albedo := texs.get("_Albedo") or texs.get("_MainTex"):
            pbr["baseColorTexture"] = {"index": gltf.texture(self.albedo(albedo.read()))}
        if normal := texs.get("_Normal") or texs.get("_BumpMap"):
            entry["normalTexture"] = {"index": gltf.texture(self.normal(normal.read()))}
        if packed := texs.get("_MetallicAOEmiSmooth"):
            orm, mask = self.packed_orm(packed.read())
            pbr["metallicRoughnessTexture"] = {"index": gltf.texture(orm)}
            entry["occlusionTexture"] = {"index": gltf.texture(orm)}
            entry["extras"]["emission_mask"] = mask
        else:
            pbr["metallicFactor"] = 0.0
        if "_Color" in colors:
            c = colors["_Color"]
            pbr["baseColorFactor"] = [c.r, c.g, c.b, c.a]
        gltf.doc["materials"].append(entry)
        gltf.material_index[mat_ptr.path_id] = len(gltf.doc["materials"]) - 1
        return gltf.material_index[mat_ptr.path_id]

    # --- meshes and hierarchy -------------------------------------------------------------------------------

    def mesh(self, gltf: Gltf, mesh, materials: list) -> int | None:
        handler = MeshHandler(mesh)
        handler.process()
        if not handler.m_Vertices:
            return None
        positions = [(-x, y, z) for x, y, z, *_ in handler.m_Vertices]
        attributes = {"POSITION": gltf.accessor(positions, "VEC3", minmax=True)}
        if handler.m_Normals:
            attributes["NORMAL"] = gltf.accessor([(-x, y, z) for x, y, z, *_ in handler.m_Normals], "VEC3")
        if handler.m_Tangents:
            attributes["TANGENT"] = gltf.accessor([(-x, y, z, -w) for x, y, z, w in handler.m_Tangents], "VEC4")
        if handler.m_UV0:
            attributes["TEXCOORD_0"] = gltf.accessor([(u, 1 - v) for u, v, *_ in handler.m_UV0], "VEC2")
        primitives = []
        for i, triangles in enumerate(handler.get_triangles()):
            if not triangles:
                continue
            indices = [index for a, b, c in triangles for index in (a, c, b)]
            prim = {"attributes": attributes, "indices": gltf.accessor(indices, "SCALAR", indices=True)}
            if i < len(materials):
                prim["material"] = self.material(gltf, materials[i])
            primitives.append(prim)
        gltf.doc["meshes"].append({"name": mesh.m_Name, "primitives": primitives})
        return len(gltf.doc["meshes"]) - 1

    def node(self, gltf: Gltf, transform, stats: dict) -> int:
        go = transform.m_GameObject.read()
        p, q, s = transform.m_LocalPosition, transform.m_LocalRotation, transform.m_LocalScale
        node = {"name": go.m_Name, "translation": [-p.x, p.y, p.z], "rotation": [q.x, -q.y, -q.z, q.w],
                "scale": [s.x, s.y, s.z]}
        if not go.m_IsActive:
            node["extras"] = {"inactive": True}
        components = {c.component.type.name: c.component for c in go.m_Component}
        if "MeshRenderer" in components and "MeshFilter" in components:
            renderer = components["MeshRenderer"].read()
            mesh_ptr = components["MeshFilter"].read().m_Mesh
            if mesh_ptr.path_id:
                index = self.mesh(gltf, mesh_ptr.read(), list(renderer.m_Materials))
                if index is not None:
                    node["mesh"] = index
                    stats["meshes"] += 1
        if "SkinnedMeshRenderer" in components:
            stats["skipped_skinned"] += 1
        if "Camera" in components:
            cam = components["Camera"].read_typetree()
            if cam.get("m_Enabled"):
                node.setdefault("extras", {})["camera"] = {
                    "fov": cam.get("field of view", 60.0), "near": cam.get("near clip plane", 0.3),
                    "far": cam.get("far clip plane", 1000.0), "ortho": bool(cam.get("orthographic", 0)),
                    "depth": cam.get("m_Depth", 0)}
                stats["cameras"] = stats.get("cameras", 0) + 1
        light_child = None
        if "Light" in components:
            light = components["Light"].read()
            if light.m_Enabled and go.m_IsActive:
                light_child = self.light(gltf, light)
                stats["lights"] = stats.get("lights", 0) + 1
        gltf.doc["nodes"].append(node)
        index = len(gltf.doc["nodes"]) - 1
        if "Animator" in components:
            self.animators.append((index, components["Animator"].read()))
        children = [self.node(gltf, child.read(), stats) for child in transform.m_Children]
        if light_child is not None:
            children.append(light_child)
        if children:
            gltf.doc["nodes"][index]["children"] = children
        return index

    def light(self, gltf: Gltf, light) -> int:
        """A KHR_lights_punctual light on a child node turned 180 degrees: Unity lights shine along +Z,
        glTF lights along -Z. Unity intensities have no physical unit; the scale is tuned by eye."""
        kind = {0: "spot", 1: "directional", 2: "point"}.get(light.m_Type, "point")
        c = light.m_Color
        entry = {"type": kind, "color": [c.r, c.g, c.b], "intensity": light.m_Intensity * (3.0 if kind == "directional" else 60.0)}
        if kind != "directional":
            entry["range"] = light.m_Range
        if kind == "spot":
            half = math.radians(light.m_SpotAngle) / 2
            entry["spot"] = {"innerConeAngle": half * 0.8, "outerConeAngle": half}
        ext = gltf.doc.setdefault("extensions", {}).setdefault("KHR_lights_punctual", {"lights": []})
        ext["lights"].append(entry)
        used = gltf.doc.setdefault("extensionsUsed", [])
        if "KHR_lights_punctual" not in used:
            used.append("KHR_lights_punctual")
        gltf.doc["nodes"].append({"name": "light", "rotation": [0.0, 1.0, 0.0, 0.0],
                                  "extensions": {"KHR_lights_punctual": {"light": len(ext["lights"]) - 1}}})
        return len(gltf.doc["nodes"]) - 1

    def export_scene(self, env, name: str) -> dict:
        """Every non-UI root of a scene file into one glTF (UI is exported separately by export_ui)."""
        gltf = Gltf()
        stats = {"meshes": 0, "skipped_skinned": 0, "clips": 0, "roots": 0}
        self.animators = []
        for obj in env.objects:
            if obj.type.name != "Transform":
                continue
            transform = obj.read()
            if transform.m_Father.path_id:
                continue
            try:
                gltf.doc["scenes"][0]["nodes"].append(self.node(gltf, transform, stats))
                stats["roots"] += 1
            except Exception as e:
                stats.setdefault("errors", []).append(f"{transform.m_GameObject.read().m_Name}: {type(e).__name__}: {e}")
        self.animations(gltf, stats)
        out = self.models_dir / f"{name}.glb"
        gltf.write(out)
        stats["bytes"] = out.stat().st_size
        return stats

    def animations(self, gltf: Gltf, stats: dict) -> None:
        """Every clip of every Animator's controller becomes a glTF animation targeting the exported nodes."""
        for animator_node, animator in self.animators:
            if not animator.m_Avatar.path_id or not animator.m_Controller.path_id:
                continue
            path_of_hash = {h: p for h, p in animator.m_Avatar.read().m_TOS}
            node_of_path = {}

            def walk(node_index: int, path: str) -> None:
                node_of_path[path] = node_index
                for child in gltf.doc["nodes"][node_index].get("children", []):
                    name = gltf.doc["nodes"][child]["name"]
                    walk(child, f"{path}/{name}" if path else name)

            walk(animator_node, "")
            seen = set()
            for clip_ptr in animator.m_Controller.read().m_AnimationClips:
                if clip_ptr.path_id in seen:
                    continue
                seen.add(clip_ptr.path_id)
                clip = clip_ptr.read()
                channels, samplers = [], []
                for path, prop, times, values in anim.sample_clip(clip, path_of_hash):
                    if path not in node_of_path:
                        continue
                    samplers.append({"input": gltf.accessor(times, "SCALAR", minmax=True, animation=True),
                                     "output": gltf.accessor(values, "VEC4" if prop == "rotation" else "VEC3",
                                                             animation=True),
                                     "interpolation": "LINEAR"})
                    channels.append({"sampler": len(samplers) - 1,
                                     "target": {"node": node_of_path[path], "path": prop}})
                if channels:
                    gltf.doc.setdefault("animations", []).append(
                        {"name": clip.m_Name, "channels": channels, "samplers": samplers})
                    stats["clips"] += 1

    def export(self, root_transform, name: str) -> dict:
        gltf = Gltf()
        stats = {"meshes": 0, "skipped_skinned": 0, "clips": 0}
        self.animators = []
        gltf.doc["scenes"][0]["nodes"].append(self.node(gltf, root_transform, stats))
        self.animations(gltf, stats)
        out = self.models_dir / f"{name}.glb"
        gltf.write(out)
        stats["bytes"] = out.stat().st_size
        return stats


def find_roots(env, names: set[str]):
    """Root transforms (no parent) whose GameObject name is in `names`."""
    for obj in env.objects:
        if obj.type.name != "Transform":
            continue
        transform = obj.read()
        if transform.m_Father.path_id:
            continue
        go = transform.m_GameObject.read()
        if go.m_Name in names:
            yield go.m_Name, transform


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("data_dir", type=pathlib.Path)
    ap.add_argument("--pack", type=pathlib.Path, required=True)
    ap.add_argument("--file", default="resources.assets")
    ap.add_argument("names", nargs="*", help="root GameObject names to export")
    ap.add_argument("--scene", action="append", default=[], help="levelN=Name: export a whole scene")
    args = ap.parse_args()

    exporter = Exporter(args.pack)
    for spec in args.scene:
        level, name = spec.split("=")
        print(name, exporter.export_scene(UnityPy.load(str(args.data_dir / level)), name))
    if not args.names:
        return 0
    env = UnityPy.load(str(args.data_dir / args.file))
    found = set()
    for name, transform in find_roots(env, set(args.names)):
        if name in found:
            continue
        found.add(name)
        print(name, exporter.export(transform, name))
    for missing in set(args.names) - found:
        print(f"{missing}: no root GameObject with that name in {args.file}", file=sys.stderr)
    return 0 if found else 1


if __name__ == "__main__":
    sys.exit(main())
