#!/usr/bin/env python3
"""A stand-in for Blender with Catbot's add-on turned on (tests only).

Reads the port and key from the catbot_bridge.json it's given, prints the port, and answers the
add-on's requests the way the add-on does. Every request it does is written to the log given
second, one JSON line each. The real add-on is tested against real Blender separately.
"""
import base64
import json
import os
import socket
import sys

config = json.load(open(sys.argv[1]))
log = sys.argv[2]
PNG = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mP8z8BQDwAEhQGAhKmMIQAAAABJRU5ErkJggg==")


def note(op, args):
    with open(log, "a") as f:
        f.write(json.dumps({"op": op, "args": args}) + "\n")


def handle(op, args):
    if op == "info":
        return {"bridge": 2, "blender": "5.3.0-fake", "file": None, "objects": 1}
    if op == "scene":
        return {"info": handle("info", {}), "objects": [{"name": "Cube", "type": "MESH", "location": [0, 0, 0]}]}
    if op == "object":
        if args.get("name") != "Cube":
            raise ValueError("there's no object called %r" % args.get("name"))
        return {"name": "Cube", "mesh": {"vertices": 8}}
    if op == "look":
        return {"mimeType": "image/jpeg", "data": base64.b64encode(b"picture of " + (args.get("path") or "viewport").encode()).decode()}
    note(op, args)
    if op == "change":
        return {"done": ["did " + s["action"] for s in args["steps"]]}
    if op == "python":
        return {"printed": "hi from blender\n", "result": 42}
    if op == "render":
        os.makedirs(os.path.dirname(args["path"]), exist_ok=True)
        open(args["path"], "wb").write(PNG)
        return {"path": args["path"], "seconds": 0.5, "engine": "CYCLES"}
    if op == "save":
        return {"exported" if args["kind"] != "blend" else "saved": args.get("path", "/where/it/was.blend")}
    raise ValueError("there's no request called %r" % op)


sock = socket.socket()
sock.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
sock.bind(("127.0.0.1", config["port"]))
sock.listen(8)
print(config["port"], flush=True)
while True:
    conn, _ = sock.accept()
    with conn:
        data = b""
        while not data.endswith(b"\n"):
            part = conn.recv(65536)
            if not part:
                break
            data += part
        ask = json.loads(data)
        if ask.get("key") != config["key"]:
            reply = {"ok": False, "error": "wrong key"}
        else:
            try:
                reply = {"ok": True, "result": handle(ask["op"], ask.get("args") or {})}
            except Exception as e:
                reply = {"ok": False, "error": str(e)}
        conn.sendall(json.dumps(reply).encode() + b"\n")
