"""Enum tables the game's data refers to by name (behaviour trees name skill tags, e.g. "GAUSS_FLAYER",
while skill records store the numbers), read from the player's own Assembly-CSharp.dll metadata into
pack/data/_enums.json as {"Namespace.Type+Nested": {"NAME": value}}.
"""
import argparse
import json
import pathlib
import struct
import sys

import dnfile

WANTED = ["TAG", "TYPE", "RELATION_TYPE", "DAMAGE_TYPE", "SLOT", "TILESET", "CANTICLE", "AWAKE_LEVEL", "ID", "TEMPLATE"]


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("data_dir", type=pathlib.Path)
    ap.add_argument("--pack", type=pathlib.Path, required=True)
    args = ap.parse_args()
    pe = dnfile.dnPE(str(args.data_dir / "Managed" / "Assembly-CSharp.dll"))
    md = pe.net.mdtables
    # Field row index -> constant value.
    constants = {}
    for c in md.Constant:
        parent = c.Parent
        if parent.table is not None and parent.table.name == "Field":
            raw = bytes(c.Value.value_bytes())
            size = len(raw)
            fmt = {1: "<b", 2: "<h", 4: "<i", 8: "<q"}.get(size)
            if fmt:
                constants[parent.row_index] = struct.unpack(fmt, raw)[0]
    # Nested type -> enclosing type, for qualified names.
    enclosing = {}
    if md.NestedClass:
        for n in md.NestedClass:
            enclosing[n.NestedClass.row_index] = n.EnclosingClass.row_index
    types = list(md.TypeDef)

    def qualified(i):
        t = types[i - 1]
        name = str(t.TypeName)
        if i in enclosing:
            return qualified(enclosing[i]) + "+" + name
        return f"{t.TypeNamespace}.{name}" if str(t.TypeNamespace) else name

    out = {}
    for i, t in enumerate(types, start=1):
        if str(t.TypeName) not in WANTED:
            continue
        extends = t.Extends.row
        if extends is None or str(getattr(extends, "TypeName", "")) != "Enum":
            continue
        values = {}
        for f in t.FieldList:
            row = f.row
            idx = f.row_index
            if idx in constants:
                values[str(row.Name)] = constants[idx]
        if values:
            out[qualified(i)] = values
    path = args.pack / "data" / "_enums.json"
    path.write_text(json.dumps(out, indent=0))
    print(f"{len(out)} enums -> {path}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
