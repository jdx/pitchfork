#!/usr/bin/env python3
"""Binds a TCP port, listens briefly, then closes the listener and idles.

For health_port e2e tests: the daemon process must stay alive after the port
stops accepting connections so the supervisor's health check (not process
exit) triggers the crash path.

Usage: health_port_server.py <port> <listen_seconds>
"""
import socket
import sys
import time

port = int(sys.argv[1]) if len(sys.argv) > 1 else 0
listen_seconds = float(sys.argv[2]) if len(sys.argv) > 2 else 3

s = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", port))
s.listen(5)
print(f"Listening on 127.0.0.1:{port}")
time.sleep(listen_seconds)
s.close()
print("Listener closed")
time.sleep(3600)  # keep the process alive so the daemon stays "running"