#!/usr/bin/env python3
"""HTTPS server that terminates TLS itself and requires a client certificate.

Used to prove the proxy spliced the connection instead of terminating it: the
handshake only completes if the client's certificate and the server's own
certificate reached each other untouched, and the reply names both the client
certificate and the SNI hostname the client sent.

Usage: mtls_echo_server.py <port> <server-cert> <server-key> <client-ca>
"""
import ssl
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port = int(sys.argv[1])
server_cert, server_key, client_ca = sys.argv[2], sys.argv[3], sys.argv[4]


def common_name(cert):
    """The CN of a peer certificate as returned by getpeercert()."""
    for rdn in (cert or {}).get("subject", ()):
        for key, value in rdn:
            if key == "commonName":
                return value
    return "none"


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        """Report the client certificate and the negotiated connection."""
        cert = self.connection.getpeercert()
        sni = getattr(self.connection, "pitchfork_sni", None) or "none"
        body = (
            f"client-cn={common_name(cert)}\n"
            f"sni={sni}\n"
            f"protocol={self.connection.version()}\n"
        ).encode()
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, fmt, *args):
        """Keep request logging out of the daemon's output."""


def record_sni(sock, server_name, context):
    """Remember the SNI hostname the client sent, for the reply to report."""
    sock.pitchfork_sni = server_name


ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
ctx.sni_callback = record_sni
ctx.load_cert_chain(certfile=server_cert, keyfile=server_key)
# Refuse any client that does not present a certificate signed by client_ca:
# a proxy that terminated TLS would have no certificate to present.
ctx.verify_mode = ssl.CERT_REQUIRED
ctx.load_verify_locations(cafile=client_ca)

server = HTTPServer(("127.0.0.1", port), Handler)
server.socket = ctx.wrap_socket(server.socket, server_side=True)
print(f"Server listening on https://localhost:{port}", flush=True)
server.serve_forever()
