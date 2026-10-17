"""Export prefabs (GameObject hierarchies) from the user's install as .glb into the content pack.

Static MeshRenderer parts only for now; skinned meshes and animation come in a later pass.
Textures are written once to <pack>/models/textures/ and referenced by relative URI, so models share them.

Unity is left-handed (Y up), glTF right-handed (Y up): X is mirrored, which also flips triangle winding,
quaternion Y/Z and tangent handedness. Unity UV origin is bottom-left, glTF top-left.
"""
import argparse
import json
import pathlib
import struct
import sys

import UnityPy
from PIL import Image, ImageStat
from UnityPy.helpers.MeshHelper import MeshHandler

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

    def _view(self, data: bytes, target: int) -> int:
        while len(self.blob) % 4:
            self.blob.append(0)
        self.doc["bufferViews"].append({"buffer": 0, "byteOffset": len(self.blob), "byteLength": len(data),
                                        "target": target})
        self.blob += data
        return len(self.doc["bufferViews"]) - 1

    def accessor(self, rows: list, kind: str, minmax: bool = False) -> int:
        width = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}[kind]
        flat = [c for row in rows for c in (row if width > 1 else (row,))]
        if kind == "SCALAR":
            data, ctype, target = struct.pack(f"<{len(flat)}I", *flat), UINT32, ELEMENT_ARRAY_BUFFER
        else:
            data, ctype, target = struct.pack(f"<{len(flat)}f", *flat), FLOAT, ARRAY_BUFFER
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
            prim = {"attributes": attributes, "indices": gltf.accessor(indices, "SCALAR")}
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
        gltf.doc["nodes"].append(node)
        index = len(gltf.doc["nodes"]) - 1
        children = [self.node(gltf, child.read(), stats) for child in transform.m_Children]
        if children:
            gltf.doc["nodes"][index]["children"] = children
        return index

    def export(self, root_transform, name: str) -> dict:
        gltf = Gltf()
        stats = {"meshes": 0, "skipped_skinned": 0}
        gltf.doc["scenes"][0]["nodes"].append(self.node(gltf, root_transform, stats))
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
    ap.add_argument("names", nargs="+", help="root GameObject names to export")
    args = ap.parse_args()

    exporter = Exporter(args.pack)
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
