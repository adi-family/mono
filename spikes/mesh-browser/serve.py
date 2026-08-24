#!/usr/bin/env python3
"""Serve the spike page and collect its verdict.

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


class Handler(http.server.SimpleHTTPRequestHandler):
    extensions_map = {
        **http.server.SimpleHTTPRequestHandler.extensions_map,
        ".wasm": "application/wasm",
        ".js": "text/javascript",
    }

    def __init__(self, *args, **kwargs):
        super().__init__(*args, directory=str(HERE), **kwargs)

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
    REPORT.unlink(missing_ok=True)
    socketserver.TCPServer.allow_reuse_address = True
    with socketserver.TCPServer(("127.0.0.1", port), Handler) as httpd:
        print(f"spike page on http://127.0.0.1:{port}/", flush=True)
        httpd.serve_forever()
