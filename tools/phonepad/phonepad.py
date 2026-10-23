#!/usr/bin/env python3
# Mush8 - phonepad.py
# Turns any phone's browser into a controller for Mush8 on this Mac, no app needed
# usage: python3 tools/phonepad/phonepad.py, then open the printed address on the phone (PHONEPAD_PORT=<port> to change it)
# the launcher starts it with the game ("Browser controllers"), --exit-with-pid <game pid> then stops it when the game quits
"""Serves a touch controller page and forwards its input over a WebSocket to
Mush8's iPhone controller port (UDP 21421), with the same packets the iOS app
sends (crates/nxhost/src/remote.rs). Every phone gets its own UDP socket, so
Mush8 sees each one as a separate iPhone controller. Standard library only."""
import asyncio
import base64
import hashlib
import math
import os
import socket
import struct
import subprocess
import sys
import time

HTTP_PORT = int(os.environ.get("PHONEPAD_PORT", "8088"))
MUSH8 = ("127.0.0.1", 21421)
PAGE = os.path.join(os.path.dirname(os.path.abspath(__file__)), "pad.html")
WS_GUID = "258EAFA5-E914-47DA-95CA-C5AB0DC85B11"


def log(text):
    print("[%s] %s" % (time.strftime("%H:%M:%S"), text), flush=True)


def packet(buttons, lx, ly):
    # "M8P1", buttons u64, sticks lx ly rx ry f32, motion flag u8, accel + gyro 6x f32 (53 bytes)
    return b"M8P1" + struct.pack("<Q4fB6f", buttons, lx, ly, 0.0, 0.0, 0, *([0.0] * 6))


def axis(text):
    v = float(text)
    return max(-1.0, min(1.0, v)) if math.isfinite(v) else 0.0


def send_frame(writer, op, payload):
    n = len(payload)
    head = bytes([0x80 | op, n]) if n < 126 else bytes([0x80 | op, 126]) + struct.pack(">H", n)
    writer.write(head + payload)


async def read_frame(reader):
    b0, b1 = await reader.readexactly(2)
    n = b1 & 0x7F
    if n == 126:
        n = struct.unpack(">H", await reader.readexactly(2))[0]
    elif n == 127:
        n = struct.unpack(">Q", await reader.readexactly(8))[0]
    mask = await reader.readexactly(4) if b1 & 0x80 else b""
    data = await reader.readexactly(n)
    if mask:
        data = bytes(c ^ mask[i % 4] for i, c in enumerate(data))
    return b0 & 0x0F, data


class Mush8Link(asyncio.DatagramProtocol):
    def __init__(self, writer):
        self.writer = writer

    def datagram_received(self, data, addr):
        # Mush8 answers "M8A1" + player number (0 = no free slot) about twice a second
        if len(data) >= 5 and data[:4] == b"M8A1":
            send_frame(self.writer, 0x1, b"player %d" % data[4])

    def error_received(self, exc):
        pass


async def websocket(reader, writer, key, peer):
    accept = base64.b64encode(hashlib.sha1((key + WS_GUID).encode()).digest()).decode()
    writer.write(("HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\n"
                  "Sec-WebSocket-Accept: %s\r\n\r\n" % accept).encode())
    await writer.drain()
    transport, _ = await asyncio.get_running_loop().create_datagram_endpoint(lambda: Mush8Link(writer), remote_addr=MUSH8)
    log("phone connected: %s" % peer)
    try:
        while True:
            op, data = await read_frame(reader)
            if op == 0x8:
                break
            if op == 0x9:
                send_frame(writer, 0xA, data)
            elif op == 0x1:
                try:
                    b, lx, ly = data.decode().split(",")
                    transport.sendto(packet(int(b) & 0xFFFF, axis(lx), axis(ly)))
                except ValueError:
                    pass
    except (asyncio.IncompleteReadError, ConnectionError):
        pass
    finally:
        transport.close()
        writer.close()
        log("phone disconnected: %s" % peer)


async def handle(reader, writer):
    try:
        head = await reader.readuntil(b"\r\n\r\n")
    except (asyncio.IncompleteReadError, asyncio.LimitOverrunError, ConnectionError):
        writer.close()
        return
    lines = head.decode("latin-1").split("\r\n")
    parts = lines[0].split(" ")
    path = parts[1] if len(parts) > 1 else "/"
    headers = {}
    for line in lines[1:]:
        k, sep, v = line.partition(":")
        if sep:
            headers[k.strip().lower()] = v.strip()
    if path == "/ws" and "sec-websocket-key" in headers:
        await websocket(reader, writer, headers["sec-websocket-key"], "%s:%d" % writer.get_extra_info("peername")[:2])
        return
    if path in ("/", "/index.html"):
        with open(PAGE, "rb") as f:
            body = f.read()
        status, ctype = "200 OK", "text/html; charset=utf-8"
    else:
        body, status, ctype = b"not found", "404 Not Found", "text/plain"
    writer.write(("HTTP/1.1 %s\r\nContent-Type: %s\r\nContent-Length: %d\r\nCache-Control: no-store\r\n"
                  "Connection: close\r\n\r\n" % (status, ctype, len(body))).encode() + body)
    await writer.drain()
    writer.close()


def lan_ip():
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    try:
        s.connect(("192.0.2.1", 9))  # picks the outgoing interface, sends nothing
        return s.getsockname()[0]
    except OSError:
        return "127.0.0.1"
    finally:
        s.close()


def local_host():
    # the Bonjour name stays the same while the router hands out different IPs
    try:
        name = subprocess.run(["scutil", "--get", "LocalHostName"], capture_output=True, text=True, check=True).stdout.strip()
    except (OSError, subprocess.CalledProcessError):
        name = socket.gethostname().split(".")[0]
    return name + ".local"


def alive(pid):
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


async def exit_with(pid):
    # the launcher execs into the game, so its pid is the game's
    while alive(pid):
        await asyncio.sleep(1)
    log("Mush8 quit, stopping the phone pad")
    os._exit(0)


async def main():
    if "--exit-with-pid" in sys.argv[1:-1]:
        asyncio.ensure_future(exit_with(int(sys.argv[sys.argv.index("--exit-with-pid") + 1])))
    server = await asyncio.start_server(handle, "0.0.0.0", HTTP_PORT)
    log("Mush8 phone pad is running")
    print("open this on a phone on the same Wi-Fi:  http://%s:%d" % (local_host(), HTTP_PORT), flush=True)
    print("(it stays the same, if it doesn't load use http://%s:%d)" % (lan_ip(), HTTP_PORT), flush=True)
    print("stop with Ctrl+C", flush=True)
    async with server:
        await server.serve_forever()


if __name__ == "__main__":
    try:
        asyncio.run(main())
    except KeyboardInterrupt:
        pass
