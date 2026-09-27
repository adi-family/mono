#!/usr/bin/env python3
"""A stand-in mesh gateway for the dev panel: two paired machines that answer, with apps on them.

The dev store's paired devices (`.adi-dev/mono/mesh/fleet.toml`) are hand-written records with
made-up keys, so none of them ever answers and every screen that lists a remote machine's apps
has nothing to show. A real second node needs a scratch store, a mesh daemon and a relay. This
needs none of that: `adi-app` reaches a node by sending plain HTTP to its local mesh gateway with
`Host: app.<node>.n.adi` (`crates/adi-app/src/node.rs`), and `ADI_MESH_GATEWAY_ADDR` says where
that gateway is. Point the dev API at this server and `laptop` and `nas` answer as nodes would —
so the dev API's own listing code (`viewer.rs`) runs unchanged against them.

What the machines run, and what they let the dev machine open, covers each state a tile can be in:

    laptop  notes      running, granted          opens
            budget     running, not granted      asks for access
            timesheet  stopped, granted          opens, and starting it is the node's job
    nas     photos     running, granted          opens
            media      stopped, no host          nothing to open

Any password unlocks them. The links a granted app gets (`http://<app>.<node>.n.adi/`) go through
the real front door and the live mesh, which knows neither machine — so they do not open. This
serves the listings, not the apps.

    python3 scripts/dev-fleet-fixture.py [PORT]      # default 45190
"""

import json
import sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT = int(sys.argv[1]) if len(sys.argv) > 1 else 45190

# The dev store's own mesh key (`GET dev.adi/api/mesh` → `id`). The nodes find the dev machine in
# their fleet by it, which is how the dev API learns what it has been granted there.
DEV_KEY = "93bd264b1f2c4dd2d9c1fd702ed3bbb85fb48796de49621f6048f1bb0c923a2e"
PAIRED_AT = 1789000000


def app(name, running, host=True, description=None):
    return {
        "id": name,
        "dir": f"/home/adi/.adi/mono/dashboards/{name}",
        "name": name,
        "description": description,
        "project": None,
        "host": f"{name}.adi" if host else None,
        "frontend_port": 40100 if running else None,
        "backend_port": None,
        "frontend_running": running,
        "backend_running": running,
        "modules": [],
        "routes": [],
        "archived_at": None,
        "moved_to": None,
        "never_started": False,
    }


NODES = {
    "laptop": {
        "apps": [
            app("notes", True, description="Meeting notes, searchable"),
            app("budget", True, description="Monthly spend by category"),
            app("timesheet", False),
        ],
        "grants": ["http:app", "http:notes", "http:timesheet"],
    },
    "nas": {
        "apps": [
            app("photos", True, description="Family photo library"),
            app("media", False, host=False),
        ],
        "grants": ["http:app", "http:photos"],
    },
}


class Gateway(BaseHTTPRequestHandler):
    def do_GET(self):
        host = (self.headers.get("Host") or "").split(":")[0]
        node = host.removeprefix("app.").removesuffix(".n.adi")
        spec = NODES.get(node)
        # Any other machine is one nobody answers for: drop the connection, as a dial that
        # reached no one would, so the dev panel still shows it unreachable.
        if spec is None or not host.startswith("app."):
            self.close_connection = True
            return
        if not self.headers.get("Authorization"):
            return self.answer(401, {"error": "password required"})
        path = self.path.split("?")[0]
        if path == "/api/health":
            return self.answer(200, {"ok": True})
        if path == "/api/dashboards":
            return self.answer(200, {"dashboards": spec["apps"]})
        if path == "/api/fleet":
            me = {
                "petname": "studio",
                "key": DEV_KEY,
                "nickname": "studio",
                "paired_at": PAIRED_AT,
                "grants": spec["grants"],
                "has_password": True,
            }
            return self.answer(200, {"nodes": [me]})
        return self.answer(404, {"error": f"{path} is not served by the fixture"})

    # What "ask for access" sends (`viewer::grant_self`): `{"petname", "grant"}`. Granted at once,
    # in memory — a restart takes the machines back to the grants above.
    def do_POST(self):
        host = (self.headers.get("Host") or "").split(":")[0]
        spec = NODES.get(host.removeprefix("app.").removesuffix(".n.adi"))
        if spec is None or self.path.split("?")[0] != "/api/fleet/grants/add":
            return self.answer(404, {"error": f"{self.path} is not served by the fixture"})
        body = json.loads(self.rfile.read(int(self.headers.get("Content-Length") or 0)) or b"{}")
        if body.get("grant") and body["grant"] not in spec["grants"]:
            spec["grants"].append(body["grant"])
        return self.answer(200, {"ok": True})

    def answer(self, status, body):
        data = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        sys.stderr.write(f"{self.headers.get('Host')} {fmt % args}\n")


if __name__ == "__main__":
    ThreadingHTTPServer(("127.0.0.1", PORT), Gateway).serve_forever()
