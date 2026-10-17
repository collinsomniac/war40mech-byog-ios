"""Reader for the Odin Serializer binary format (OdinSerializer is Apache-2.0; format per its BinaryDataReader).

Turns `serializationData.SerializedBytes` into plain JSON-able values. Unity object references come back as
{"$ref_unity": index} into `serializationData.ReferencedUnityObjects`.
"""
import struct
import uuid

# BinaryEntryType
NAMED_REF, UNNAMED_REF, NAMED_STRUCT, UNNAMED_STRUCT, END_NODE = 1, 2, 3, 4, 5
START_ARRAY, END_ARRAY, PRIMITIVE_ARRAY = 6, 7, 8
NAMED_INTERNAL, UNNAMED_INTERNAL = 9, 10
NAMED_EXT_INDEX, UNNAMED_EXT_INDEX = 11, 12
NAMED_EXT_GUID, UNNAMED_EXT_GUID = 13, 14
TYPE_NAME, TYPE_ID, END_STREAM = 47, 48, 49
NAMED_EXT_STRING, UNNAMED_EXT_STRING = 50, 51
NAMED_NULL, UNNAMED_NULL = 45, 46

# (named, unnamed) -> struct format for fixed-size primitives
PRIMITIVES = {
    15: "<b", 17: "<B", 19: "<h", 21: "<H", 23: "<i", 25: "<I", 27: "<q", 29: "<Q",
    31: "<f", 33: "<d", 37: "<H", 43: "<?",
}
NAMED_DECIMAL, NAMED_STRING, NAMED_GUID = 35, 39, 41


class OdinReader:
    def __init__(self, data: bytes):
        self.data = bytes(data)
        self.pos = 0
        self.types: dict[int, str] = {}

    def _take(self, n: int) -> bytes:
        if self.pos + n > len(self.data):
            raise EOFError(f"odin: need {n} bytes at {self.pos}/{len(self.data)}")
        chunk = self.data[self.pos:self.pos + n]
        self.pos += n
        return chunk

    def _unpack(self, fmt: str):
        return struct.unpack(fmt, self._take(struct.calcsize(fmt)))[0]

    def _string(self) -> str:
        wide = self._take(1)[0]
        length = self._unpack("<i")
        if wide:
            return self._take(length * 2).decode("utf-16-le")
        return self._take(length).decode("latin-1")

    def _type(self):
        tag = self._take(1)[0]
        if tag == TYPE_NAME:
            type_id = self._unpack("<i")
            self.types[type_id] = self._string()
            return self.types[type_id]
        if tag == TYPE_ID:
            return self.types.get(self._unpack("<i"), "<unknown type>")
        if tag == UNNAMED_NULL:
            return None
        raise ValueError(f"odin: bad type entry {tag} at {self.pos - 1}")

    def read_all(self) -> dict:
        """Top-level members of the serialized object, as {name: value}."""
        members = {}
        while self.pos < len(self.data):
            entry = self._entry()
            if entry is None:
                break
            name, value = entry
            members[name if name is not None else f"#{len(members)}"] = value
        return members

    def _members_until(self, end_tag: int):
        named, items = {}, []
        while True:
            if self.data[self.pos] == end_tag:
                self.pos += 1
                return named, items
            entry = self._entry()
            if entry is None:
                raise EOFError("odin: stream ended inside a node")
            name, value = entry
            if name is None:
                items.append(value)
            else:
                named[name] = value

    def _entry(self):
        tag = self._take(1)[0]
        if tag == END_STREAM:
            return None
        # Value entries come in (named, unnamed) pairs: odd tags 15..43 are named, the next tag is unnamed.
        value_tags = set(PRIMITIVES) | {NAMED_DECIMAL, NAMED_STRING, NAMED_GUID}
        tag_base = tag - 1 if tag - 1 in value_tags else tag
        named = tag in value_tags or tag in (NAMED_REF, NAMED_STRUCT, NAMED_INTERNAL, NAMED_EXT_INDEX,
                                             NAMED_EXT_GUID, NAMED_EXT_STRING, NAMED_NULL)
        name = self._string() if named else None

        if tag in (NAMED_REF, UNNAMED_REF, NAMED_STRUCT, UNNAMED_STRUCT):
            type_name = self._type()
            node = {"$type": type_name}
            if tag in (NAMED_REF, UNNAMED_REF):
                node["$id"] = self._unpack("<i")
            members, items = self._members_until(END_NODE)
            node.update(members)
            if items:
                node["$items"] = items
            return name, node
        if tag == START_ARRAY:
            self._unpack("<q")  # declared length
            _, items = self._members_until(END_ARRAY)
            return name, items
        if tag == PRIMITIVE_ARRAY:
            count = self._unpack("<i")
            size = self._unpack("<i")
            return name, {"$primitive_array": self._take(count * size).hex(), "count": count, "size": size}
        if tag in (NAMED_INTERNAL, UNNAMED_INTERNAL):
            return name, {"$ref_internal": self._unpack("<i")}
        if tag in (NAMED_EXT_INDEX, UNNAMED_EXT_INDEX):
            return name, {"$ref_unity": self._unpack("<i")}
        if tag in (NAMED_EXT_GUID, UNNAMED_EXT_GUID):
            return name, {"$ref_guid": str(uuid.UUID(bytes_le=self._take(16)))}
        if tag in (NAMED_EXT_STRING, UNNAMED_EXT_STRING):
            return name, {"$ref_string": self._string()}
        if tag in (NAMED_NULL, UNNAMED_NULL):
            return name, None
        if tag_base == NAMED_STRING:
            return name, self._string()
        if tag_base == NAMED_GUID:
            return name, str(uuid.UUID(bytes_le=self._take(16)))
        if tag_base == NAMED_DECIMAL:
            return name, {"$decimal": self._take(16).hex()}
        if tag_base in PRIMITIVES:
            value = self._unpack(PRIMITIVES[tag_base])
            if tag_base == 37:
                value = chr(value)
            return name, value
        raise ValueError(f"odin: unknown entry {tag} at {self.pos - 1}")


def decode(serialized_bytes) -> dict:
    return OdinReader(bytes(serialized_bytes)).read_all()
