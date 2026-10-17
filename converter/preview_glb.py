"""CPU-only preview of a .glb from this converter: flat-shaded orthographic front and side views to PNG.

Used to verify hierarchy transforms and handedness without touching a GPU.
"""
import json
import math
import pathlib
import struct
import sys

from PIL import Image, ImageDraw


def mat_mul(a, b):
    return [[sum(a[i][k] * b[k][j] for k in range(4)) for j in range(4)] for i in range(4)]


def trs(node):
    tx, ty, tz = node.get("translation", (0, 0, 0))
    qx, qy, qz, qw = node.get("rotation", (0, 0, 0, 1))
    sx, sy, sz = node.get("scale", (1, 1, 1))
    r = [[1 - 2 * (qy * qy + qz * qz), 2 * (qx * qy - qz * qw), 2 * (qx * qz + qy * qw)],
         [2 * (qx * qy + qz * qw), 1 - 2 * (qx * qx + qz * qz), 2 * (qy * qz - qx * qw)],
         [2 * (qx * qz - qy * qw), 2 * (qy * qz + qx * qw), 1 - 2 * (qx * qx + qy * qy)]]
    return [[r[0][0] * sx, r[0][1] * sy, r[0][2] * sz, tx],
            [r[1][0] * sx, r[1][1] * sy, r[1][2] * sz, ty],
            [r[2][0] * sx, r[2][1] * sy, r[2][2] * sz, tz],
            [0, 0, 0, 1]]


def load(path):
    data = pathlib.Path(path).read_bytes()
    json_len = struct.unpack_from("<I", data, 12)[0]
    doc = json.loads(data[20:20 + json_len])
    blob = data[20 + json_len + 8:]

    def read(acc_index):
        acc = doc["accessors"][acc_index]
        view = doc["bufferViews"][acc["bufferView"]]
        width = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}[acc["type"]]
        fmt = "I" if acc["componentType"] == 5125 else "f"
        flat = struct.unpack_from(f"<{acc['count'] * width}{fmt}", blob, view["byteOffset"])
        return [flat[i:i + width] for i in range(0, len(flat), width)] if width > 1 else list(flat)

    triangles = []

    def walk(index, parent):
        node = doc["nodes"][index]
        world = mat_mul(parent, trs(node))
        if "mesh" in node and not node.get("extras", {}).get("inactive"):
            for prim in doc["meshes"][node["mesh"]]["primitives"]:
                pos = [tuple(sum(world[r][c] * p[c] for c in range(3)) + world[r][3] for r in range(3))
                       for p in read(prim["attributes"]["POSITION"])]
                idx = read(prim["indices"])
                triangles.extend((pos[idx[i]], pos[idx[i + 1]], pos[idx[i + 2]]) for i in range(0, len(idx), 3))
        for child in node.get("children", []):
            walk(child, world)

    identity = [[1 if i == j else 0 for j in range(4)] for i in range(4)]
    for root in doc["scenes"][0]["nodes"]:
        walk(root, identity)
    return triangles


def render(triangles, axis, size=512):
    """axis 'front' looks down -Z (glTF forward), 'side' looks down -X."""
    def project(p):
        return (p[0], p[1], p[2]) if axis == "front" else (p[2], p[1], -p[0])

    projected = [tuple(project(p) for p in tri) for tri in triangles]
    xs = [p[0] for t in projected for p in t]
    ys = [p[1] for t in projected for p in t]
    span = max(max(xs) - min(xs), max(ys) - min(ys)) or 1
    scale = (size * 0.9) / span
    cx, cy = (max(xs) + min(xs)) / 2, (max(ys) + min(ys)) / 2
    image = Image.new("RGB", (size, size), (24, 26, 30))
    draw = ImageDraw.Draw(image)
    light = (0.4, 0.6, 0.7)
    for tri in sorted(projected, key=lambda t: sum(p[2] for p in t)):
        a, b, c = tri
        u = [b[i] - a[i] for i in range(3)]
        v = [c[i] - a[i] for i in range(3)]
        n = (u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0])
        length = math.sqrt(sum(x * x for x in n)) or 1
        if n[2] / length <= 0:  # back-facing: counter-clockwise front faces in a right-handed view
            continue
        shade = max(0.15, sum(n[i] / length * light[i] for i in range(3)))
        colour = tuple(int(shade * k) for k in (190, 205, 170))
        draw.polygon([(size / 2 + (p[0] - cx) * scale, size / 2 - (p[1] - cy) * scale) for p in tri], fill=colour)
    return image


if __name__ == "__main__":
    tris = load(sys.argv[1])
    sheet = Image.new("RGB", (1024, 512))
    sheet.paste(render(tris, "front"), (0, 0))
    sheet.paste(render(tris, "side"), (512, 0))
    sheet.save(sys.argv[2])
    print(f"{len(tris)} triangles -> {sys.argv[2]}")
