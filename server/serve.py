"""Serve a content pack read-only to the phone over the tailnet.

Binds only to the given address (use this PC's Tailscale IP), so the pack is never exposed on the LAN or internet.
    python serve.py --pack ../content/pack --bind 100.x.y.z --port 8790

/manifest.json is generated on request from the files actually in the pack (re-hashing only files whose size or
modification time changed, cached in <pack>/.manifest-cache.json), so a re-export can never leave the phone
syncing against stale checksums.
"""
import argparse
import functools
import hashlib
import http.server
import json
import pathlib
import threading

SKIP = {"manifest.json", ".manifest-cache.json", ".synced.json"}


class Manifest:
    def __init__(self, pack: pathlib.Path):
        self.pack = pack
        self.cache_path = pack / ".manifest-cache.json"
        self.lock = threading.Lock()
        try:
            self.cache = json.loads(self.cache_path.read_text())
        except (OSError, ValueError):
            self.cache = {}

    def build(self) -> bytes:
        with self.lock:
            files, changed = [], False
            seen = set()
            for path in sorted(p for p in self.pack.rglob("*") if p.is_file()):
                rel = path.relative_to(self.pack).as_posix()
                if path.name in SKIP or path.suffix == ".partial":
                    continue
                st = path.stat()
                entry = self.cache.get(rel)
                if not entry or entry["size"] != st.st_size or entry["mtime"] != st.st_mtime_ns:
                    entry = {"size": st.st_size, "mtime": st.st_mtime_ns, "sha256": hashlib.sha256(path.read_bytes()).hexdigest()}
                    self.cache[rel] = entry
                    changed = True
                seen.add(rel)
                files.append({"path": rel, "size": entry["size"], "sha256": entry["sha256"]})
            for gone in set(self.cache) - seen:
                del self.cache[gone]
                changed = True
            if changed:
                self.cache_path.write_text(json.dumps(self.cache))
            return json.dumps({"version": 1, "files": files}).encode()


class PackHandler(http.server.SimpleHTTPRequestHandler):
    manifest: Manifest = None

    def do_GET(self):
        if self.path.split("?")[0] == "/manifest.json":
            body = self.manifest.build()
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)
            return
        super().do_GET()

    def list_directory(self, path):  # manifest.json is the only index the app needs
        self.send_error(404)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pack", type=pathlib.Path, required=True)
    ap.add_argument("--bind", required=True)
    ap.add_argument("--port", type=int, default=8790)
    args = ap.parse_args()

    pack = args.pack.resolve()
    PackHandler.manifest = Manifest(pack)
    files = json.loads(PackHandler.manifest.build())["files"]
    print(f"{len(files)} files, {sum(f['size'] for f in files) / 1e6:.0f} MB")
    handler = functools.partial(PackHandler, directory=str(pack))
    with http.server.ThreadingHTTPServer((args.bind, args.port), handler) as server:
        print(f"serving {pack} on http://{args.bind}:{args.port}")
        server.serve_forever()


if __name__ == "__main__":
    main()
