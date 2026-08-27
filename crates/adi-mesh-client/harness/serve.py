#!/usr/bin/env python3
"""Serve the probe page and collect its verdict.

Two jobs, one process:

* GET  — the page, its wasm bundle, and the JS glue, out of this directory.
* POST /report — the JSON the page produces, written to `report.json` so a shell can wait on a
  file instead of on a headless browser's console.

The wasm MIME type matters: a browser refuses `WebAssembly.instantiateStreaming` on anything but
`application/wasm`, and Python's own table does not know the extension.
"""

import http.server
import socketserver
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPORT = HERE / "report.json"
# What to serve. The probe runs against this directory (the page plus wasm-pack's `pkg/`); the
# end-to-end run passes `dist/`, so the browser sees exactly the bytes that get deployed.
ROOT = HERE


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
    }

    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(ROOT), **kwargs)

    def end_headers(self):
        # A service worker is only allowed to control a scope its own file's path covers, and a
        # cached `sw.js` is how a stale worker outlives a rebuild. Both are one header each.
        if self.path.endswith("sw.js"):
            self.send_header("Service-Worker-Allowed", "/")
            self.send_header("Cache-Control", "no-cache")
        super().end_headers()

    def do_POST(self):
        if self.path != "/report":
            self.send_error(404)
            return
        length = int(self.headers.get("Content-Length", 0))
        REPORT.write_bytes(self.rfile.read(length))
        self.send_response(204)
        self.end_headers()
        print(f"report written to {REPORT}", flush=True)

    def log_message(self, fmt, *args):
        print(f"{self.address_string()} {fmt % args}", flush=True)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 45080
    if len(sys.argv) > 2:
        ROOT = Path(sys.argv[2]).resolve()
    REPORT.unlink(missing_ok=True)
    socketserver.TCPServer.allow_reuse_address = True
    with socketserver.TCPServer(("127.0.0.1", port), Handler) as httpd:
        print(f"serving {ROOT} on http://127.0.0.1:{port}/", flush=True)
        httpd.serve_forever()
