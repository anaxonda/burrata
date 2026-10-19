"""Loopback-only controlled same-byte media transport with Range and faults."""
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import re
import threading
import time


class FixtureServer:
    def __init__(self, path, fault='none', stall_seconds=2):
        self.data = path.read_bytes()
        self.fault = fault
        self.stall_seconds = stall_seconds
        self.lock = threading.Lock()
        self.requests = []
        owner = self

        class Handler(BaseHTTPRequestHandler):
            def log_message(self, *_args):
                pass

            def do_GET(self):
                with owner.lock:
                    index = len(owner.requests)
                    row = dict(index=index, rangeRequested=bool(self.headers.get('Range')))
                    owner.requests.append(row)
                if self.path != '/fixture':
                    self.send_error(404)
                    return
                if owner.fault == 'deny-first' and index == 0:
                    row['status'] = 403
                    self.send_error(403)
                    return
                if owner.fault == 'stall':
                    time.sleep(owner.stall_seconds)
                length = len(owner.data)
                start, end = 0, length - 1
                header = self.headers.get('Range')
                if header:
                    match = re.fullmatch(r'bytes=(\d+)-(\d*)', header)
                    if not match:
                        self.send_error(416)
                        return
                    start = int(match[1])
                    end = min(length - 1, int(match[2]) if match[2] else length - 1)
                    if start > end:
                        self.send_response(416)
                        self.send_header('Content-Range', f'bytes */{length}')
                        self.end_headers()
                        return
                status = 206 if header else 200
                row.update(status=status, start=start, end=end, advertisedBytes=end - start + 1)
                self.send_response(status)
                self.send_header('Content-Type', 'application/octet-stream')
                self.send_header('Accept-Ranges', 'bytes')
                self.send_header('Content-Length', str(end - start + 1))
                self.send_header('Connection', 'close')
                if header:
                    self.send_header('Content-Range', f'bytes {start}-{end}/{length}')
                self.end_headers()
                body = owner.data[start:end + 1]
                if owner.fault == 'truncate':
                    body = body[:len(body) // 4]
                try:
                    self.wfile.write(body)
                    row['sentBytes'] = len(body)
                except (BrokenPipeError, ConnectionResetError):
                    row['clientDisconnected'] = True

        self.server = ThreadingHTTPServer(('127.0.0.1', 0), Handler)
        self.server.daemon_threads = True
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()
        self.url = f'http://127.0.0.1:{self.server.server_port}/fixture'

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(timeout=1)
