import http.server
import json
import pathlib
import subprocess
import sys
import argparse
import tempfile
import re
import threading
import time

parser = argparse.ArgumentParser(description='Exercise the release decoder across bounded HTTP body gaps.')
parser.add_argument('--ffmpeg', default='ffmpeg')
parser.add_argument('--output', required=True, type=pathlib.Path)
args = parser.parse_args()
workspace = tempfile.TemporaryDirectory(prefix='burrata-stream-fault-')
root = pathlib.Path(workspace.name)
decoder = args.ffmpeg
policy = pathlib.Path(__file__).resolve().parents[2] / 'src-tauri/src/playback_policy.rs'
policy_us = re.search(r'NETWORK_READ_TIMEOUT_US: &str = "(\d+)"', policy.read_text())
if not policy_us:
    raise RuntimeError('Missing production network timeout policy')
policy_seconds = int(policy_us.group(1)) / 1000000
media = root / 'fixture.m4a'
subprocess.run([decoder, '-nostdin', '-hide_banner', '-loglevel', 'error', '-y',
                '-f', 'lavfi', '-i', 'anoisesrc=duration=30:sample_rate=44100:seed=17',
                '-ac', '2', '-c:a', 'aac', '-b:a', '128k', '-movflags', '+faststart', str(media)], check=True)
data = media.read_bytes()
results = []
for gap, timeout in [(0, policy_seconds), (6, 5), (6, policy_seconds), (12, policy_seconds)]:
    facts = {'gapSeconds': gap, 'readTimeoutSeconds': timeout, 'requests': []}
    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_):
            pass
        def do_GET(self):
            start = 0
            value = self.headers.get('Range', '')
            if value.startswith('bytes='):
                start = int(value[6:].split('-')[0])
            facts['requests'].append({'range': value, 'start': start})
            self.send_response(206 if value else 200)
            self.send_header('Content-Type', 'audio/mp4')
            self.send_header('Content-Length', str(len(data) - start))
            self.send_header('Accept-Ranges', 'bytes')
            if value:
                self.send_header('Content-Range', f'bytes {start}-{len(data)-1}/{len(data)}')
            self.end_headers()
            cut = max(start, len(data) // 2)
            try:
                self.wfile.write(data[start:cut])
                self.wfile.flush()
                time.sleep(gap)
                self.wfile.write(data[cut:])
            except (BrokenPipeError, ConnectionResetError):
                facts['clientClosedBeforeRemainder'] = True
    server = http.server.ThreadingHTTPServer(('127.0.0.1', 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    began = time.monotonic()
    try:
        proc = subprocess.run([decoder, '-nostdin', '-hide_banner', '-loglevel', 'error',
            '-rw_timeout', str(int(timeout * 1000000)), '-xerror', '-i',
            f'http://127.0.0.1:{server.server_port}/fixture.m4a',
            '-vn', '-f', 'f32le', '-acodec', 'pcm_f32le', '-ar', '44100', '-ac', '2', 'pipe:1'],
            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=35)
        facts.update(exitCode=proc.returncode, decodedSeconds=len(proc.stdout) / (44100 * 2 * 4),
                     elapsedSeconds=round(time.monotonic() - began, 3),
                     errorClass='decoder_failed' if proc.returncode else None)
    finally:
        server.shutdown()
        server.server_close()
        thread.join()
    results.append(facts)
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps(results, indent=2) + '\n')
assert results[1]['decodedSeconds'] < 20 and results[1]['exitCode'] != 0
assert all(r['exitCode'] == 0 and 29.9 <= r['decodedSeconds'] <= 30.1 for i, r in enumerate(results) if i != 1)
print(json.dumps(results, indent=2))
