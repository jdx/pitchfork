#!/usr/bin/env python3
"""Query a DNS responder directly and print what it answered.

Used by the proxy tests where `dig` is not installed. Prints the rcode name and
the answer address, if any, as `<RCODE> <address-or-->`.

Usage: dns_query.py <port> <name> [A|AAAA] [--tcp]
"""

import socket
import struct
import sys

TYPES = {"A": 1, "AAAA": 28}
RCODES = {0: "NOERROR", 1: "FORMERR", 2: "SERVFAIL", 3: "NXDOMAIN", 4: "NOTIMP", 5: "REFUSED"}


def build_query(name, qtype):
    msg = struct.pack(">HHHHHH", 0x4242, 0x0100, 1, 0, 0, 0)
    for label in name.split("."):
        msg += bytes([len(label)]) + label.encode()
    return msg + b"\0" + struct.pack(">HH", qtype, 1)


def recv_exactly(sock, count):
    """Read exactly `count` bytes, or fail.

    `recv` is free to return fewer bytes than asked for, and returns b"" once
    the peer closes; neither case should silently produce a short frame or spin.
    """
    buf = b""
    while len(buf) < count:
        chunk = sock.recv(count - len(buf))
        if not chunk:
            raise EOFError(f"connection closed after {len(buf)} of {count} bytes")
        buf += chunk
    return buf


def ask(port, msg, tcp):
    if tcp:
        sock = socket.create_connection(("127.0.0.1", port), 5)
        sock.settimeout(5)
        sock.sendall(struct.pack(">H", len(msg)) + msg)
        length = struct.unpack(">H", recv_exactly(sock, 2))[0]
        return recv_exactly(sock, length)
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(5)
    sock.sendto(msg, ("127.0.0.1", port))
    return sock.recv(512)


def first_answer(reply):
    """Address in the first answer record, or None when there is no answer."""
    if struct.unpack(">H", reply[6:8])[0] == 0:
        return None
    pos = 12
    while reply[pos]:
        pos += 1 + reply[pos]
    pos += 1 + 4  # the root label, then QTYPE and QCLASS
    # Answer: 2-byte name pointer, type, class, TTL, then the RDATA length.
    rdlen = struct.unpack(">H", reply[pos + 10 : pos + 12])[0]
    rdata = reply[pos + 12 : pos + 12 + rdlen]
    family = socket.AF_INET if rdlen == 4 else socket.AF_INET6
    return socket.inet_ntop(family, rdata)


def main():
    port = int(sys.argv[1])
    name = sys.argv[2]
    qtype = TYPES[sys.argv[3]] if len(sys.argv) > 3 and sys.argv[3] in TYPES else 1
    tcp = "--tcp" in sys.argv
    reply = ask(port, build_query(name, qtype), tcp)
    rcode = struct.unpack(">H", reply[2:4])[0] & 0x0F
    print(RCODES.get(rcode, str(rcode)), first_answer(reply) or "-")


if __name__ == "__main__":
    main()
