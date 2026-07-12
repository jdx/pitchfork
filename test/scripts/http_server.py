#!/usr/bin/env python3
"""Simple HTTP server for e2e tests.

Delays start by specified seconds, then serves /health with a configurable
status code. All other paths return 404.

Usage: http_server.py <delay_seconds> <port> [health_status]
"""
import sys
import time
from http.server import HTTPServer, BaseHTTPRequestHandler

delay = int(sys.argv[1]) if len(sys.argv) > 1 else 1
port = int(sys.argv[2]) if len(sys.argv) > 2 else 18080
health_status = int(sys.argv[3]) if len(sys.argv) > 3 else 200

print(f"Waiting {delay}s before starting server...")
time.sleep(delay)

print(f"Starting HTTP server on port {port}...")


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path == "/health":
            self.send_response(health_status)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"OK")
        else:
            self.send_response(404)
            self.send_header("Content-Type", "text/plain")
            self.end_headers()
            self.wfile.write(b"Not Found")

    def log_message(self, format, *args):
        pass  # suppress default logging


server = HTTPServer(("0.0.0.0", port), Handler)
print(f"Server listening on http://localhost:{port}")
print("Health check available at /health")
server.serve_forever()
