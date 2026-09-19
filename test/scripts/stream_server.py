#!/usr/bin/env python3
"""HTTP server with long-lived responses, for idle-shutdown e2e tests.

  /health          200 immediately
  /stream?secs=N   streams one line every 0.5s for N seconds (chunked)
  /ws              answers an Upgrade with 101 and holds the connection open,
                   echoing whatever arrives, until the client closes it

Usage: stream_server.py <port>
"""
import sys
import time
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from urllib.parse import parse_qs, urlparse

port = int(sys.argv[1])


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        url = urlparse(self.path)
        if url.path == "/health":
            self._plain(200, b"OK")
        elif url.path == "/stream":
            secs = float(parse_qs(url.query).get("secs", ["5"])[0])
            self.send_response(200)
            self.send_header("Content-Type", "text/event-stream")
            self.send_header("Transfer-Encoding", "chunked")
            self.end_headers()
            deadline = time.time() + secs
            while time.time() < deadline:
                self._chunk(b"data: tick\n\n")
                time.sleep(0.5)
            self._chunk(b"")
        elif url.path == "/ws" and self.headers.get("Upgrade"):
            self.send_response(101)
            self.send_header("Upgrade", self.headers["Upgrade"])
            self.send_header("Connection", "Upgrade")
            self.end_headers()
            self.wfile.flush()
            while True:
                data = self.rfile.read1(4096)
                if not data:
                    break
                self.wfile.write(data)
                self.wfile.flush()
            self.close_connection = True
        else:
            self._plain(404, b"Not Found")

    def _plain(self, status, body):
        self.send_response(status)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def _chunk(self, data):
        self.wfile.write(b"%x\r\n%s\r\n" % (len(data), data))
        self.wfile.flush()

    def log_message(self, format, *args):
        pass


print(f"Starting stream server on port {port}...", flush=True)
ThreadingHTTPServer(("127.0.0.1", port), Handler).serve_forever()
