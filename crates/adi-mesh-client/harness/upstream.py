#!/usr/bin/env python3
"""The node's "local service" for the probe: a page, an event stream, and a websocket.

The node's gateway resolves a service label to a local port and then splices raw bytes
(`adi-mesh/src/tunnel.rs`). So whatever this answers with is what the browser tab sees, and the
three shapes below are the three the client has to be able to carry:

    GET /            a page with a Content-Length            — the case the spike already proved
    GET /sse         text/event-stream, one event a second   — a response that never ends
    GET /ws, /api/ws an RFC 6455 upgrade, echo + one push    — what a live channel is
    GET /api/health  a JSON reply on a second path           — what a panel's own frontend calls

`/` is a miniature of what a control panel is from the browser's point of view: a page that then
calls its own `/api/*` with a **root-absolute** URL and opens a websocket at another one. Serving it
is what proves the service worker really is routing a whole app to one node rather than one page.

Deliberately hand-rolled on top of the stdlib rather than a framework: the point is to be
unambiguous about what goes on the wire and when it is flushed. A framework that buffered would
make the node look like it was buffering.
"""

import base64
import hashlib
import socketserver
import struct
import sys
import threading
import time

# How many events /sse emits before it stops, and how far apart. The probe's verdict is the
# *spread* of arrival times, so the gap is the signal: a buffered stream delivers all of them at
# once whatever this is set to.
SSE_EVENTS = 8
SSE_GAP = 1.0

GUID = b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11"

PAGE = b"""<!doctype html>
<html><head><meta charset="utf-8"><title>node-local service</title></head>
<body>
<h1 id="title">Hello from the node's local service.</h1>
<p id="health">health: pending</p>
<p id="live">live: pending</p>
<p id="where">path: <span id="path"></span></p>
<script>
// Root-absolute, exactly as a real panel's frontend does it (`docs/fleet.md` 4): the page must
// never learn its own address, so what routes this to the right machine is the service worker.
document.getElementById("path").textContent = location.pathname;
fetch("/api/health")
  .then((r) => r.json())
  .then((j) => { document.getElementById("health").textContent = "health: " + j.status; })
  .catch((e) => { document.getElementById("health").textContent = "health: failed " + e; });

const socket = new WebSocket((location.protocol === "https:" ? "wss://" : "ws://") + location.host + "/api/ws");
socket.onopen = () => socket.send("ping-1");
socket.onmessage = (e) => { document.getElementById("live").textContent = "live: " + e.data; };
socket.onerror = () => { document.getElementById("live").textContent = "live: failed"; };
</script>
</body></html>
"""

HEALTH = b'{"status":"ok","service":"node-local"}'



class Handler(socketserver.StreamRequestHandler):
    # Long enough that a websocket sitting idle between messages is not mistaken for a dead one.
    timeout = 120

    def handle(self):
        # A loop, not one exchange: the mesh gateway splices one QUIC bi-stream to one TCP
        # connection, and a panel sends its page request and then its `/api` calls down that same
        # connection. Answering once and hanging up would make every asset after the first fail.
        while True:
            if not self.exchange():
                return

    def exchange(self):
        try:
            head = self.read_head()
        except (OSError, ValueError):
            return False
        if not head:
            return False
        target = head.splitlines()[0].split(" ")[1] if head.splitlines() else "/"
        headers = self.parse_headers(head)
        say(f"{head.splitlines()[0] if head.splitlines() else '?'}  upgrade={headers.get('upgrade')}")

        if target.startswith("/ws") or target.startswith("/api/ws"):
            self.websocket(headers)
            return False
        if target.startswith("/sse"):
            self.event_stream()
            return False
        if target.startswith("/api/health"):
            self.json(HEALTH)
        else:
            self.page()
        # `Connection: close` is honoured, which is what the probe's first case asks for.
        return "close" not in headers.get("connection", "").lower()

    # --- reading -------------------------------------------------------------------------

    def read_head(self):
        """Everything up to the blank line. Raw, because the body (if any) is never read here."""
        data = b""
        while b"\r\n\r\n" not in data:
            chunk = self.connection.recv(4096)
            if not chunk:
                return ""
            data += chunk
            if len(data) > 64 * 1024:
                raise ValueError("request head too long")
        return data.split(b"\r\n\r\n", 1)[0].decode("latin-1")

    @staticmethod
    def parse_headers(head):
        out = {}
        for line in head.splitlines()[1:]:
            if ":" in line:
                name, value = line.split(":", 1)
                out[name.strip().lower()] = value.strip()
        return out

    # --- the three shapes ----------------------------------------------------------------

    def page(self):
        self.connection.sendall(
            b"HTTP/1.1 200 OK\r\n"
            b"Content-Type: text/html; charset=utf-8\r\n"
            b"Content-Length: " + str(len(PAGE)).encode() + b"\r\n\r\n" + PAGE
        )

    def json(self, payload):
        self.connection.sendall(
            b"HTTP/1.1 200 OK\r\n"
            b"Content-Type: application/json\r\n"
            b"Content-Length: " + str(len(payload)).encode() + b"\r\n\r\n" + payload
        )

    def event_stream(self):
        """A response with no length that does not end — and one flush per event.

        No Content-Length and no chunked encoding, which is what makes it `Framing::UntilClose` on
        the client. `Cache-Control: no-cache` because a proxy that buffered this would be
        indistinguishable from a node that did, and this probe is about telling those apart.
        """
        self.connection.sendall(
            b"HTTP/1.1 200 OK\r\n"
            b"Content-Type: text/event-stream\r\n"
            b"Cache-Control: no-cache\r\n"
            b"Connection: keep-alive\r\n\r\n"
        )
        for n in range(1, SSE_EVENTS + 1):
            time.sleep(SSE_GAP)
            try:
                self.connection.sendall(f"data: tick-{n}\n\n".encode())
            except OSError:
                say(f"sse: the reader went away after {n - 1} events")
                return
        say(f"sse: sent all {SSE_EVENTS} events")

    def websocket(self, headers):
        key = headers.get("sec-websocket-key", "")
        if not key:
            self.connection.sendall(b"HTTP/1.1 400 Bad Request\r\nContent-Length: 0\r\n\r\n")
            return
        accept = base64.b64encode(hashlib.sha1(key.encode() + GUID).digest()).decode()
        self.connection.sendall(
            b"HTTP/1.1 101 Switching Protocols\r\n"
            b"Upgrade: websocket\r\n"
            b"Connection: Upgrade\r\n"
            b"Sec-WebSocket-Accept: " + accept.encode() + b"\r\n\r\n"
        )
        say("ws: upgraded")

        while True:
            frame = self.read_frame()
            if frame is None:
                say("ws: closed")
                return
            opcode, payload = frame
            if opcode == 0x8:
                self.send_frame(0x8, payload[:2])
                say("ws: peer closed")
                return
            if opcode == 0x9:
                self.send_frame(0xA, payload)
                continue
            text = payload.decode("utf-8", "replace")
            self.send_frame(opcode, payload)
            say(f"ws: echoed {text!r}")
            # The direction a live channel actually needs: the server speaking with nothing
            # outstanding from the client. Sent after the last expected echo so the probe reads
            # them in a fixed order rather than racing.
            if text == "ping-3":
                time.sleep(0.3)
                self.send_frame(0x1, b"push-1")
                say("ws: pushed unprompted")

    # --- RFC 6455 framing, server side ---------------------------------------------------

    def read_frame(self):
        head = self.recv_exact(2)
        if head is None:
            return None
        opcode = head[0] & 0x0F
        masked = head[1] & 0x80
        length = head[1] & 0x7F
        if length == 126:
            length = struct.unpack("!H", self.recv_exact(2))[0]
        elif length == 127:
            length = struct.unpack("!Q", self.recv_exact(8))[0]
        mask = self.recv_exact(4) if masked else b"\0\0\0\0"
        payload = self.recv_exact(length) if length else b""
        if payload is None:
            return None
        if masked:
            payload = bytes(b ^ mask[i % 4] for i, b in enumerate(payload))
        return opcode, payload

    def send_frame(self, opcode, payload):
        """Server frames are never masked (§5.1) — the client must refuse one that is."""
        out = bytes([0x80 | opcode])
        if len(payload) < 126:
            out += bytes([len(payload)])
        elif len(payload) <= 0xFFFF:
            out += bytes([126]) + struct.pack("!H", len(payload))
        else:
            out += bytes([127]) + struct.pack("!Q", len(payload))
        self.connection.sendall(out + payload)

    def recv_exact(self, n):
        out = b""
        while len(out) < n:
            chunk = self.connection.recv(n - len(out))
            if not chunk:
                return None
            out += chunk
        return out


class Server(socketserver.ThreadingTCPServer):
    allow_reuse_address = True
    daemon_threads = True


def say(message):
    print(f"[{time.strftime('%H:%M:%S')}] {message}", flush=True)


if __name__ == "__main__":
    port = int(sys.argv[1]) if len(sys.argv) > 1 else 45081
    with Server(("127.0.0.1", port), Handler) as server:
        say(f"upstream on 127.0.0.1:{port} — / /sse /ws")
        threading.Thread(target=server.serve_forever, daemon=True).start()
        while True:
            time.sleep(3600)
