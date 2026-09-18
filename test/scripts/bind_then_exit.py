#!/usr/bin/env python3
"""Bind a TCP port, hold it open for N seconds, then exit 0.

Used to check that a `oneshot` daemon's readiness comes from its exit status
and not from the implicit TCP port check that an expected port would otherwise
install. The socket is held by this process itself, so nothing is left running
once the task exits.

Usage: bind_then_exit.py <port> <seconds>
"""
import socket
import sys
import time

port = int(sys.argv[1])
secs = float(sys.argv[2])

sock = socket.socket()
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(("127.0.0.1", port))
sock.listen(1)
print(f"listening on {port}", flush=True)

time.sleep(secs)

sock.close()
print("task done", flush=True)
