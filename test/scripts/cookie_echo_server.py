#!/usr/bin/env python3
"""HTTP server that reports the Cookie header fields it received.

A backend behind the proxy must see one Cookie field, whatever the client sent.
Reporting the field count separately from the values is what makes a split
header visible: joined wrongly, the values still look plausible.

Usage: cookie_echo_server.py <port>
"""
import sys
from http.server import HTTPServer, BaseHTTPRequestHandler

port = int(sys.argv[1]) if len(sys.argv) > 1 else 18090


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        """Reply with the number of Cookie fields received, then each one."""
        fields = self.headers.get_all("Cookie", [])
        body = f"fields={len(fields)}\n"
        for field in fields:
            body += f"cookie={field}\n"
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.end_headers()
        self.wfile.write(body.encode())

    def log_message(self, format, *args):
        """Keep request logging out of the daemon's output."""


server = HTTPServer(("127.0.0.1", port), Handler)
print(f"Server listening on http://localhost:{port}")
server.serve_forever()
