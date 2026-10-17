"""Serve a content pack read-only to the phone over the tailnet.

Binds only to the given address (use this PC's Tailscale IP), so the pack is never exposed on the LAN or internet.
    python serve.py --pack ../content/pack --bind 100.x.y.z --port 8790
"""
import argparse
import functools
import http.server
import pathlib


class PackHandler(http.server.SimpleHTTPRequestHandler):
    def list_directory(self, path):  # manifest.json is the only index the app needs
        self.send_error(404)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--pack", type=pathlib.Path, required=True)
    ap.add_argument("--bind", required=True)
    ap.add_argument("--port", type=int, default=8790)
    args = ap.parse_args()

    handler = functools.partial(PackHandler, directory=str(args.pack.resolve()))
    with http.server.ThreadingHTTPServer((args.bind, args.port), handler) as server:
        print(f"serving {args.pack.resolve()} on http://{args.bind}:{args.port}")
        server.serve_forever()


if __name__ == "__main__":
    main()
