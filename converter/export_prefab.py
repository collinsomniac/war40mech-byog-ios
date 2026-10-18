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
                 animation: bool = False, joints: bool = False) -> int:
        """Vertex data by default; `indices` for triangle lists; `animation` for sampler input/output;
        `joints` for JOINTS_0 (unsigned shorts)."""
        width = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}[kind]
        flat = [c for row in rows for c in (row if width > 1 else (row,))]
        if joints:
            data, ctype, target = struct.pack(f"<{len(flat)}H", *flat), 5123, ARRAY_BUFFER
        elif indices:
            data, ctype, target = struct.pack(f"<{len(flat)}I", *flat), UINT32, ELEMENT_ARRAY_BUFFER
        else:
            data, ctype = struct.pack(f"<{len(flat)}f", *flat), FLOAT
            target = None if animation or kind == "MAT4" else ARRAY_BUFFER
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
        self.doc["scenes"][0]["nodes"].extend(self.__dict__.pop("world_nodes", []))
        # Texture URIs are relative to models/; models in subfolders (units/, battle/) step back up.
        models = next((p for p in path.parents if p.name == "models"), path.parent)
        depth = len(path.parent.relative_to(models).parts) if path.parent != models else 0
        if depth:
            for image in self.doc["images"]:
                if image["uri"].startswith("textures/"):
                    image["uri"] = "../" * depth + image["uri"]
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
        # Optional (gameobject path_id, batched) -> extras dict, for callers that tag nodes (battle templates).
        self.node_extras = None
        # Leave out inactive GameObjects (variant parts a prefab toggles on at runtime).
        self.skip_inactive = False
        # Optional clip-name predicate: which Animator clips to export.
        self.clip_filter = None

    # --- textures -------------------------------------------------------------------------------------------

    def _have(self, filename: str) -> bool:
        """Textures persist across runs and models; convert each once."""
        if filename in self.written_textures or (self.textures_dir / filename).exists():
            self.written_textures.add(filename)
            return True
        return False

    def _save(self, name: str, image: Image.Image) -> str:
        filename = f"{name}.png"
        if not self._have(filename):
            image.save(self.textures_dir / filename, optimize=False)
            self.written_textures.add(filename)
        return f"textures/{filename}"

    def albedo(self, tex) -> str:
        if self._have(f"{tex.m_Name}.png"):
            return f"textures/{tex.m_Name}.png"
        return self._save(tex.m_Name, tex.image.convert("RGBA"))

    def normal(self, tex) -> str:
        if self._have(f"{tex.m_Name}.normal.png"):
            return f"textures/{tex.m_Name}.normal.png"
        import numpy as np
        px = np.asarray(tex.image.convert("RGBA"), dtype=np.float32)
        r, g, a = px[..., 0], px[..., 1], px[..., 3]
        # DXT5nm stores X in alpha with red saturated; rebuild a standard tangent-space normal map.
        if r.mean() > 250 and a.std() > 1:
            x, y = a / 127.5 - 1, g / 127.5 - 1
            z = np.sqrt(np.clip(1 - x * x - y * y, 0, 1))
            out = np.stack([a, g, (z + 1) * 127.5], axis=-1)
        else:
            out = px[..., :3]
        image = Image.fromarray(np.clip(out, 0, 255).astype(np.uint8), "RGB")
        return self._save(f"{tex.m_Name}.normal", image)

    def packed_orm(self, tex) -> tuple[str, str]:
        """Mechanicus packs metallic (R), AO (G), emission mask (B), smoothness (A).
        glTF wants occlusion (R), roughness (G), metallic (B). Emission mask is kept as its own texture."""
        orm_name, mask_name = f"{tex.m_Name}.orm", f"{tex.m_Name}.emission"
        if self._have(orm_name + ".png") and self._have(mask_name + ".png"):
            return f"textures/{orm_name}.png", f"textures/{mask_name}.png"
        metal, ao, emission, smooth = tex.image.convert("RGBA").split()
        rough = Image.eval(smooth, lambda v: 255 - v)
        orm = self._save(orm_name, Image.merge("RGB", (ao, rough, metal)))
        mask = self._save(mask_name, Image.merge("RGB", (emission, emission, emission)))
        return orm, mask

    # --- materials ------------------------------------------------------------------------------------------

    def material(self, gltf: Gltf, mat_ptr) -> int:
        if mat_ptr.path_id in gltf.material_index:
            return gltf.material_index[mat_ptr.path_id]
        cache = self.__dict__.setdefault("parsed_materials", {})
        key = (getattr(mat_ptr, "assets_file", None) and mat_ptr.assets_file.name, mat_ptr.file_id, mat_ptr.path_id)
        if key not in cache:
            mat = mat_ptr.read()
            shader = mat.m_Shader.read()
            shader_name = shader.m_ParsedForm.m_Name if hasattr(shader, "m_ParsedForm") else shader.m_Name
            cache[key] = (mat, shader_name, {k: v.m_Texture for k, v in mat.m_SavedProperties.m_TexEnvs if v.m_Texture.path_id},
                          {k: v for k, v in mat.m_SavedProperties.m_Colors}, {k: v for k, v in mat.m_SavedProperties.m_Floats})
        mat, shader_name, texs, colors, floats = cache[key]

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

    def mesh(self, gltf: Gltf, mesh, materials: list, first: int = 0, count: int | None = None,
             skinned: bool = False) -> int | None:
        """A glTF mesh for `mesh` (or just submeshes first..first+count of it, for statically batched
        renderers), cached so instances share one copy."""
        reader = getattr(mesh, "object_reader", None)
        key = (getattr(reader, "path_id", id(mesh)), tuple(m.path_id for m in materials), first, count, skinned)
        cache = gltf.__dict__.setdefault("mesh_index", {})
        if key in cache:
            return cache[key]
        cache[key] = self._mesh(gltf, mesh, materials, first, count, skinned)
        return cache[key]

    def _mesh(self, gltf: Gltf, mesh, materials: list, first: int, count: int | None, skinned: bool = False) -> int | None:
        handlers = gltf.__dict__.setdefault("handlers", {})
        reader = getattr(mesh, "object_reader", None)
        key = getattr(reader, "path_id", id(mesh))
        if key not in handlers:
            handler = MeshHandler(mesh)
            handler.process()
            handlers[key] = (handler, handler.get_triangles())
        handler, all_triangles = handlers[key]
        if not handler.m_Vertices:
            return None
        primitives = []
        for i, triangles in enumerate(all_triangles):
            if count is not None and not first <= i < first + count:
                continue
            if not triangles:
                continue
            # Only the vertices this submesh uses (combined static-batch meshes hold a whole scene).
            used = sorted({index for tri in triangles for index in tri})
            remap = {old: new for new, old in enumerate(used)}
            pick = lambda rows: [rows[k] for k in used]  # noqa: E731
            attributes = {"POSITION": gltf.accessor([(-x, y, z) for x, y, z, *_ in pick(handler.m_Vertices)], "VEC3", minmax=True)}
            if handler.m_Normals:
                attributes["NORMAL"] = gltf.accessor([(-x, y, z) for x, y, z, *_ in pick(handler.m_Normals)], "VEC3")
            if handler.m_Tangents:
                attributes["TANGENT"] = gltf.accessor([(-x, y, z, -w) for x, y, z, w in pick(handler.m_Tangents)], "VEC4")
            if handler.m_UV0:
                attributes["TEXCOORD_0"] = gltf.accessor([(u, 1 - v) for u, v, *_ in pick(handler.m_UV0)], "VEC2")
            if skinned and handler.m_BoneIndices and handler.m_BoneWeights:
                attributes["JOINTS_0"] = gltf.accessor([tuple(int(j) for j in (list(b) + [0, 0, 0, 0])[:4]) for b in pick(handler.m_BoneIndices)], "VEC4", joints=True)
                weights = []
                for w in pick(handler.m_BoneWeights):
                    w = (list(w) + [0.0, 0.0, 0.0, 0.0])[:4]
                    total = sum(w) or 1.0
                    weights.append(tuple(x / total for x in w))
                attributes["WEIGHTS_0"] = gltf.accessor(weights, "VEC4")
            indices = [remap[index] for a, b, c in triangles for index in (a, c, b)]
            prim = {"attributes": attributes, "indices": gltf.accessor(indices, "SCALAR", indices=True)}
            slot = i - first if count is not None else i
            if slot < len(materials):
                prim["material"] = self.material(gltf, materials[slot])
            primitives.append(prim)
        if not primitives:
            return None
        gltf.doc["meshes"].append({"name": mesh.m_Name, "primitives": primitives})
        return len(gltf.doc["meshes"]) - 1

    def node(self, gltf: Gltf, transform, stats: dict) -> int:
        go = transform.m_GameObject.read()
        if self.skip_inactive and not go.m_IsActive:
            gltf.doc["nodes"].append({"name": go.m_Name + " (inactive)"})
            return len(gltf.doc["nodes"]) - 1
        p, q, s = transform.m_LocalPosition, transform.m_LocalRotation, transform.m_LocalScale
        node = {"name": go.m_Name, "translation": [-p.x, p.y, p.z], "rotation": [q.x, -q.y, -q.z, q.w],
                "scale": [s.x, s.y, s.z]}
        if not go.m_IsActive:
            node["extras"] = {"inactive": True}
        if self.node_extras and (extra := self.node_extras(transform.m_GameObject.path_id, batched=False)):
            node.setdefault("extras", {}).update(extra)
        components = {c.component.type.name: c.component for c in go.m_Component}
        if "MeshRenderer" in components and "MeshFilter" in components:
            renderer = components["MeshRenderer"].read()
            mesh_ptr = components["MeshFilter"].read().m_Mesh
            batch = getattr(renderer, "m_StaticBatchInfo", None)
            batched = batch is not None and batch.subMeshCount > 0
            if mesh_ptr.path_id and renderer.m_Enabled:
                mesh = mesh_ptr.read()
                if batched:
                    # Static batching: the combined mesh is already in world space; draw this renderer's
                    # submesh range from a root-level node with no transform.
                    index = self.mesh(gltf, mesh, list(renderer.m_Materials), batch.firstSubMesh, batch.subMeshCount)
                    if index is not None and go.m_IsActive:
                        world = {"name": go.m_Name + " (batched)", "mesh": index}
                        if self.node_extras and (extra := self.node_extras(transform.m_GameObject.path_id, batched=True)):
                            world["extras"] = extra
                        gltf.doc["nodes"].append(world)
                        gltf.__dict__.setdefault("world_nodes", []).append(len(gltf.doc["nodes"]) - 1)
                        stats["meshes"] += 1
                else:
                    index = self.mesh(gltf, mesh, list(renderer.m_Materials))
                    if index is not None:
                        node["mesh"] = index
                        stats["meshes"] += 1
        if "SkinnedMeshRenderer" in components:
            skin = components["SkinnedMeshRenderer"].read()
            if skin.m_Mesh.path_id and skin.m_Enabled:
                smesh = skin.m_Mesh.read()
                index = self.mesh(gltf, smesh, list(skin.m_Materials), skinned=True)
                if index is not None:
                    node["mesh"] = index
                    stats["skinned"] = stats.get("skinned", 0) + 1
                    gltf.__dict__.setdefault("pending_skins", []).append(
                        (None, [b.path_id for b in skin.m_Bones], list(smesh.m_BindPose)))
            else:
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
        reader = getattr(transform, "object_reader", None)
        if reader is not None:
            gltf.__dict__.setdefault("node_of_transform", {})[reader.path_id] = index
        pending = gltf.__dict__.get("pending_skins")
        if pending and pending[-1][0] is None and "mesh" in node and "SkinnedMeshRenderer" in components:
            pending[-1] = (index, pending[-1][1], pending[-1][2])
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

    def skins(self, gltf: Gltf, stats: dict) -> None:
        """glTF skins for skinned renderers, once every bone's node exists. Bind poses are mirrored in X
        like everything else (S M S with S = diag(-1, 1, 1, 1)) and stored column-major."""
        nodes = gltf.__dict__.get("node_of_transform", {})
        for node_index, bones, bindposes in gltf.__dict__.pop("pending_skins", []):
            if node_index is None:
                continue
            joints = [nodes.get(b) for b in bones]
            if not joints or any(j is None for j in joints) or len(bindposes) < len(joints):
                gltf.doc["nodes"][node_index].pop("mesh", None)
                stats["skins_unresolved"] = stats.get("skins_unresolved", 0) + 1
                continue
            mats = []
            sign = (-1.0, 1.0, 1.0, 1.0)
            for m in bindposes[:len(joints)]:
                e = [[getattr(m, f"e{r}{c}") for c in range(4)] for r in range(4)]
                mats.append(tuple(sign[r] * e[r][c] * sign[c] for c in range(4) for r in range(4)))
            gltf.doc.setdefault("skins", []).append({"joints": joints, "inverseBindMatrices": gltf.accessor(mats, "MAT4")})
            gltf.doc["nodes"][node_index]["skin"] = len(gltf.doc["skins"]) - 1

    def animations(self, gltf: Gltf, stats: dict) -> None:
        """Every clip of every Animator's controller becomes a glTF animation targeting the exported nodes."""
        self.skins(gltf, stats)
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
            seen, seen_names = set(), set()
            for clip_ptr in animator.m_Controller.read().m_AnimationClips:
                if clip_ptr.path_id in seen:
                    continue
                seen.add(clip_ptr.path_id)
                clip = clip_ptr.read()
                if clip.m_Name in seen_names or (self.clip_filter and not self.clip_filter(clip.m_Name)):
                    continue
                seen_names.add(clip.m_Name)
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
