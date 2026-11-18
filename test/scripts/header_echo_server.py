#!/usr/bin/env python3
"""HTTP server that echoes the forwarding headers it received, for e2e tests.

Answers every path with one `name=value` line per header, so a test can assert
what the proxy forwarded.

Usage: header_echo_server.py <port>
"""
import sys
from http.server import HTTPServer, BaseHTTPRequestHandler

port = int(sys.argv[1])
names = ["Host", "X-Forwarded-Host", "X-Forwarded-Proto", "X-Forwarded-Port"]

print(f"Serving headers on port {port}...")


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = "".join(f"{n.lower()}={self.headers.get(n, '')}\n" for n in names).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        pass  # suppress default logging


HTTPServer(("127.0.0.1", port), Handler).serve_forever()
