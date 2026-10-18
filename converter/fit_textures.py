"""Mobile texture budget: downscale pack textures larger than --max pixels on a side (in place)."""
import argparse
import pathlib
import sys
from concurrent.futures import ProcessPoolExecutor

from PIL import Image


def fit(path_max):
    path, limit = path_max
    try:
        with Image.open(path) as im:
            if max(im.size) <= limit:
                return 0
            scale = limit / max(im.size)
            out = im.resize((max(1, round(im.width * scale)), max(1, round(im.height * scale))), Image.LANCZOS)
            before = path.stat().st_size
        out.save(path, optimize=False)
        return before - path.stat().st_size
    except Exception as e:
        print(f"{path.name}: {e}", file=sys.stderr)
        return 0


def main() -> int:
    ap = argparse.ArgumentParser()
    ap.add_argument("textures", type=pathlib.Path)
    ap.add_argument("--max", type=int, default=1024)
    args = ap.parse_args()
    files = [(p, args.max) for p in sorted(args.textures.glob("*.png"))]
    with ProcessPoolExecutor(4) as pool:
        saved = sum(pool.map(fit, files, chunksize=8))
    print(f"{len(files)} textures, {saved / 1e6:.0f} MB saved")
    return 0


if __name__ == "__main__":
    sys.exit(main())
