#!/usr/bin/env python3
"""Open an upgraded (WebSocket-style) connection through the proxy and hold it
open, sending nothing, for a number of seconds.

Prints the status line of the handshake response, then closes when the time is
up.

Usage: hold_upgrade.py <proxy_port> <host> <path> <seconds>
"""
import socket
import sys
import time

proxy_port, host, path, secs = int(sys.argv[1]), sys.argv[2], sys.argv[3], float(sys.argv[4])

sock = socket.create_connection(("127.0.0.1", proxy_port))
sock.sendall(
    (
        f"GET {path} HTTP/1.1\r\n"
        f"Host: {host}\r\n"
        "Connection: Upgrade\r\n"
        "Upgrade: websocket\r\n"
        "\r\n"
    ).encode()
)
response = b""
while b"\r\n\r\n" not in response:
    chunk = sock.recv(4096)
    if not chunk:
        break
    response += chunk
print(response.split(b"\r\n", 1)[0].decode(), flush=True)
time.sleep(secs)
sock.close()
