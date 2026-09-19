#!/usr/bin/env python3
"""HTTP server that echoes an environment variable, for e2e tests.

Serves the value of the named variable (empty when unset) on every path, so a
test can assert what the daemon's environment actually contained.

Usage: echo_env_server.py <port> <env_var>
"""
import os
import sys
from http.server import HTTPServer, BaseHTTPRequestHandler

port = int(sys.argv[1])
var = sys.argv[2] if len(sys.argv) > 2 else "PITCHFORK_URL"
value = os.environ.get(var, "")

print(f"Serving {var}={value!r} on port {port}...")


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        body = f"{var}={value}".encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, format, *args):
        pass  # suppress default logging


HTTPServer(("127.0.0.1", port), Handler).serve_forever()
