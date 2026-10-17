"""Read the I2 Localization LanguageSource (every UI and story string) straight from its serialized bytes.

The type-tree generator mis-orders this I2 version's fields, so the layout is read directly:
    MonoBehaviour header, Google_* settings, Assets[], mLanguages[] {Name, Code, Flags},
    IgnoreDeviceLanguage, mTerms[] {Term, TermType, Description, Languages[], Flags[], Languages_Touch[]?}
Strings are int32 length + UTF-8, padded to 4 bytes; byte fields are padded to 4 as well.
"""
import struct


class Reader:
    def __init__(self, data: bytes):
        self.data, self.pos = data, 0

    def align(self):
        self.pos = (self.pos + 3) & ~3

    def i32(self) -> int:
        (value,) = struct.unpack_from("<i", self.data, self.pos)
        self.pos += 4
        return value

    def i64(self) -> int:
        (value,) = struct.unpack_from("<q", self.data, self.pos)
        self.pos += 8
        return value

    def u8(self) -> int:
        value = self.data[self.pos]
        self.pos += 1
        return value

    def string(self) -> str:
        n = self.i32()
        if not 0 <= n <= len(self.data) - self.pos:
            raise ValueError(f"bad string length {n} at {self.pos - 4}")
        value = self.data[self.pos:self.pos + n].decode("utf-8", "replace")
        self.pos += n
        self.align()
        return value

    def count(self, limit=1_000_000) -> int:
        n = self.i32()
        if not 0 <= n <= limit:
            raise ValueError(f"bad array length {n} at {self.pos - 4}")
        return n

    def strings(self) -> list[str]:
        return [self.string() for _ in range(self.count())]


def _term(r: Reader, languages: int, with_touch: bool) -> dict:
    term = {"term": r.string(), "type": r.i32(), "description": r.string()}
    term["text"] = r.strings()
    flags = r.count()
    r.pos += flags
    r.align()
    if with_touch:
        r.strings()
    if len(term["text"]) != languages:
        raise ValueError(f"term {term['term']!r} has {len(term['text'])} strings for {languages} languages")
    return term


def read(raw: bytes) -> dict:
    """Returns {"languages": [{name, code}], "terms": {key: [text per language]}}."""
    r = Reader(raw)
    r.i32(), r.i64()           # m_GameObject
    r.u8(); r.align()          # m_Enabled
    r.i32(), r.i64()           # m_Script
    r.string()                 # m_Name
    for _ in range(4):
        r.string()             # Google_WebServiceURL, _SpreadsheetKey, _SpreadsheetName, _LastUpdatedVersion
    r.i32(); r.i32()           # GoogleUpdateFrequency, GoogleUpdateDelay
    for _ in range(3):
        r.u8(); r.align()      # NeverDestroy, UserAgrees...x2
    for _ in range(r.count()):
        r.i32(), r.i64()       # Assets[] (PPtr)
    languages = []
    for _ in range(r.count(1000)):
        languages.append({"name": r.string(), "code": r.string()})
        r.u8(); r.align()
    r.u8(); r.align()          # IgnoreDeviceLanguage
    n_terms = r.count()
    start = r.pos
    # Newer I2 versions also serialize Languages_Touch per term; detect which layout parses.
    for with_touch in (True, False):
        r.pos = start
        try:
            terms = {}
            for _ in range(n_terms):
                t = _term(r, len(languages), with_touch)
                terms[t["term"]] = t["text"]
            return {"languages": languages, "terms": terms}
        except (ValueError, struct.error, IndexError):
            continue
    raise ValueError("LanguageSource layout not recognised")
