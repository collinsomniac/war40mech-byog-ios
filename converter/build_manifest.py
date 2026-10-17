"""Write <pack>/manifest.json: every file in the pack with its size and SHA-256, so the app can sync incrementally."""
import hashlib
import json
import pathlib
import sys


def main() -> int:
    pack = pathlib.Path(sys.argv[1])
    files = []
    for path in sorted(p for p in pack.rglob("*") if p.is_file() and p.name != "manifest.json"):
        digest = hashlib.sha256(path.read_bytes()).hexdigest()
        files.append({"path": path.relative_to(pack).as_posix(), "size": path.stat().st_size, "sha256": digest})
    (pack / "manifest.json").write_text(json.dumps({"version": 1, "files": files}, indent=1))
    print(f"{len(files)} files, {sum(f['size'] for f in files) / 1e6:.1f} MB")
    return 0


if __name__ == "__main__":
    sys.exit(main())
