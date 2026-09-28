#!/usr/bin/env python3
"""Three running apps for the dev panel: crm, inbox and ledger, each on the port the dev store
leases it (`.adi-dev/mono/ports/registry.json`), so the dev API reports them running.

The dev store's own dashboards are scaffolds nothing serves, so every app on the new UI's home
screen is stopped and no favicon is ever asked for. These answer in their place:

    crm     :45201   a favicon, and a page
    inbox   :45202   a favicon, and a page
    ledger  :45203   a page and no favicon — how a running app with none looks (its letter)

They are reached as `crm.adi`, `inbox.adi` and `ledger.adi` through the front door, by the routes
the dev services' hive file declares (`~/.adi/mono/projects/adi/.adi/hive.yaml`, `dev-apps`),
which is where the panel links them. The ports sit outside the live allocator's 8000–9999 so the
live store can never lease one of them to something else.

    python3 scripts/dev-apps-fixture.py
"""

import sys
import threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

# Stand-ins for what a real app would publish as its icon. Colour here is the app's own, not
# the panel's: a favicon is the one thing on the screen the panel does not draw.
CRM_ICON = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
<rect width="64" height="64" rx="14" fill="#2F6FEB"/>
<circle cx="32" cy="25" r="10" fill="#fff"/>
<path d="M14 52c2-10 10-15 18-15s16 5 18 15z" fill="#fff"/></svg>"""

INBOX_ICON = """<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 64 64">
<rect width="64" height="64" rx="14" fill="#E5A50A"/>
<path d="M12 20h40v26H12z" fill="#fff"/>
<path d="M12 20l20 15 20-15" fill="none" stroke="#E5A50A" stroke-width="4"/></svg>"""

APPS = {
    45201: ("crm", CRM_ICON),
    45202: ("inbox", INBOX_ICON),
    45203: ("ledger", None),
}


def handler(name, icon):
    class App(BaseHTTPRequestHandler):
        def do_GET(self):
            path = self.path.split("?")[0]
            if path == "/favicon.ico":
                if icon is None:
                    return self.answer(404, "text/plain", "no favicon")
                return self.answer(200, "image/svg+xml", icon)
            page = f"""<!doctype html><meta charset="utf-8"><title>{name}</title>
<body style="margin:0;height:100vh;display:grid;place-items:center;
font:15px/1.6 system-ui;color-scheme:dark light"><div style="text-align:center">
<h1 style="margin:0 0 8px">{name}</h1>
<p style="margin:0;opacity:.7">A test app from scripts/dev-apps-fixture.py.</p></div>"""
            return self.answer(200, "text/html; charset=utf-8", page)

        def answer(self, status, ctype, body):
            data = body.encode()
            self.send_response(status)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        def log_message(self, fmt, *args):
            sys.stderr.write(f"{name} {fmt % args}\n")

    return App


if __name__ == "__main__":
    servers = [ThreadingHTTPServer(("127.0.0.1", port), handler(*app)) for port, app in APPS.items()]
    for s in servers[1:]:
        threading.Thread(target=s.serve_forever, daemon=True).start()
    servers[0].serve_forever()
