"""Rewrite texture URIs inside existing .glb files in models/ subfolders to step back up to models/textures/."""
import json
import pathlib
import struct
import sys


def fix(path: pathlib.Path, prefix: str) -> bool:
    data = path.read_bytes()
    magic, version, _ = struct.unpack("<III", data[:12])
    json_len, json_type = struct.unpack("<II", data[12:20])
    doc = json.loads(data[20:20 + json_len])
    rest = data[20 + json_len:]
    changed = False
    for image in doc.get("images", []):
        if image.get("uri", "").startswith("textures/"):
            image["uri"] = prefix + image["uri"]
            changed = True
    if not changed:
        return False
    body = json.dumps(doc, separators=(",", ":")).encode()
    body += b" " * (-len(body) % 4)
    out = struct.pack("<III", magic, version, 12 + 8 + len(body) + len(rest)) + struct.pack("<II", len(body), json_type) + body + rest
    path.write_bytes(out)
    return True


if __name__ == "__main__":
    models = pathlib.Path(sys.argv[1])
    n = 0
    for glb in models.glob("*/*.glb"):
        n += fix(glb, "../")
    print(f"fixed {n} files")
