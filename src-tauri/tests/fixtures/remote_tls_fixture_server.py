"""Synthetic TLS fixture, only in an isolated no-egress network namespace.

Range GETs are real preflight bytes. Initial no-Range requests are real Native
reader requests. No request is proxied or sent to an outside service.
"""
import http.server
import json
import pathlib
import ssl
import sys
import threading
import time

root = pathlib.Path(sys.argv[1])
data = (root / "fixture.mp3").read_bytes()
log_lock = threading.Lock()


class Handler(http.server.BaseHTTPRequestHandler):
    def log_message(self, *_):
        pass

    def record(self, status, stage):
        with log_lock:
            with (root / "requests.jsonl").open("a") as output:
                output.write(json.dumps({"method": self.command, "path": self.path,
                                         "range": self.headers.get("Range"),
                                         "status": status, "stage": stage}) + "\n")

    def do_GET(self):
        try:
            self.respond_get()
        finally:
            self.record(None, "handler_finished")

    def respond_get(self):
        mode = (root / "mode").read_text() if (root / "mode").exists() else "refresh"
        value = self.headers.get("Range")
        data = ((root / "fixture.webm").read_bytes()
                if self.path.endswith(".webm") and (root / "fixture.webm").exists()
                else (root / "fixture.mp3").read_bytes())
        if mode.startswith("pinned-"):
            denied = (self.path == "/preflight-1.mp3"
                      or (mode != "pinned-non403" and self.path == "/prepared-alt.webm")
                      or (mode == "pinned-second-denied" and self.path == "/preflight-2.mp3"))
            # Large valid-ID3 MP3: head206/tail403. The genuine short
            # prepared Opus alternative is below the tail threshold, so deny
            # its head instead; both are actual preflight access failures.
            if denied and (self.path.endswith(".webm") or value is None
                           or not value.startswith("bytes=0-")):
                status = 503 if mode == "pinned-non403" else 403
                self.record(status, "response")
                self.send_response(status)
                self.send_header("Content-Length", "0")
                self.send_header("Connection", "close")
                self.end_headers()
                return
        if value is None:
            if mode in ("cancel", "deadline"):
                self.record(None, "entered")
                until = time.monotonic() + 8
                while not (root / "release").exists() and time.monotonic() < until:
                    time.sleep(.01)
            if self.path == "/a-1.mp3" or (self.path == "/a-2.mp3" and mode == "fallback"):
                self.record(403, "response")
                self.send_response(403)
                self.send_header("Content-Length", "0")
                self.send_header("Connection", "close")
                self.end_headers()
                return
            start, end, status = 0, len(data) - 1, 200
        else:
            assert value.startswith("bytes=")
            start, end = value[6:].split("-")
            start = int(start)
            end = min(int(end) if end else len(data) - 1, len(data) - 1)
            status = 206
        self.record(status, "response")
        try:
            self.send_response(status)
            self.send_header("Content-Length", str(end - start + 1))
            self.send_header("Content-Type", "audio/webm" if self.path.endswith(".webm") else "audio/mpeg")
            self.send_header("Accept-Ranges", "bytes")
            self.send_header("ETag", '"synthetic-stereo-fixture-v1"')
            self.send_header("Connection", "close")
            if status == 206:
                self.send_header("Content-Range", f"bytes {start}-{end}/{len(data)}")
            self.end_headers()
            self.wfile.write(data[start:end + 1])
        except (BrokenPipeError, ConnectionResetError, ssl.SSLError):
            pass


class Server(http.server.ThreadingHTTPServer):
    daemon_threads = True


server = Server(("8.8.8.8", 443), Handler)
context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
context.load_cert_chain(root / "leaf.pem", root / "leaf.key")
server.socket = context.wrap_socket(server.socket, server_side=True)
(root / "ready").touch()
server.serve_forever()
