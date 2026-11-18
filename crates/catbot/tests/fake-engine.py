#!/usr/bin/env python3
"""A stand-in for Podman or Docker (stage 17.1's tests), as `CATBOT_CONTAINER="python3 fake-engine.py"`.

It keeps its containers in FAKE_ENGINE_STATE (JSON) and logs every command line to
FAKE_ENGINE_LOG (one JSON list a line). A "running" container is a small websocket server on a
free port that echoes what it's sent, standing in for the box's screen. `exec` runs a shell
command here for real (in a scratch folder), notes xdotool and the browser, and gives a few
bytes for a screenshot.
"""
import base64, hashlib, json, os, socket, subprocess, sys, tempfile

def load():
    try:
        return json.load(open(os.environ["FAKE_ENGINE_STATE"]))
    except Exception:
        return {"built": False, "boxes": {}}

def save(state):
    json.dump(state, open(os.environ["FAKE_ENGINE_STATE"], "w"))

def kill(box):
    if box.get("pid"):
        try:
            os.kill(box["pid"], 9)
        except OSError:
            pass
    box["pid"] = None

def serve_screen(box):
    s = socket.socket(); s.bind(("127.0.0.1", 0)); port = s.getsockname()[1]; s.close()
    child = subprocess.Popen([sys.executable, __file__, "serve", str(port)], stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL,
                             stderr=subprocess.DEVNULL, start_new_session=True)
    box.update(pid=child.pid, port=port, running=True)

def serve(port):
    """The box's screen: a websocket handshake, then whatever comes is sent back."""
    s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    s.bind(("127.0.0.1", port)); s.listen(8)
    while True:
        conn, _ = s.accept()
        if os.fork():
            conn.close(); continue
        head = b""
        while b"\r\n\r\n" not in head:
            got = conn.recv(4096)
            if not got: os._exit(0)
            head += got
        key = next((l.split(b":", 1)[1].strip() for l in head.split(b"\r\n") if l.lower().startswith(b"sec-websocket-key")), b"")
        accept = base64.b64encode(hashlib.sha1(key + b"258EAFA5-E914-47DA-95CA-C5AB0DC85B11").digest())
        conn.sendall(b"HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: " + accept + b"\r\n\r\n")
        while True:
            got = conn.recv(4096)
            if not got: os._exit(0)
            conn.sendall(got)

args = sys.argv[1:]
if args[:1] == ["serve"]:
    serve(int(args[1])); sys.exit(0)
if os.environ.get("FAKE_ENGINE_LOG"):
    with open(os.environ["FAKE_ENGINE_LOG"], "a") as log:
        log.write(json.dumps(args) + "\n")
state = load()
boxes = state["boxes"]

def flag_value(name):
    return args[args.index(name) + 1] if name in args else None

cmd = args[0] if args else ""
if args[:2] == ["image", "inspect"]:
    sys.exit(0 if state["built"] else 1)
elif cmd == "build":
    assert os.path.isfile(os.path.join(args[-1], "Dockerfile")) and os.path.isfile(os.path.join(args[-1], "start.sh"))
    state["built"] = True
elif cmd == "create":
    if not state["built"]: sys.exit("no such image")
elif cmd == "cp":
    dest = args[-1]
    os.makedirs(os.path.join(dest, "core"))
    open(os.path.join(dest, "vnc.html"), "w").write("<html>fake view</html>")
    open(os.path.join(dest, "core", "rfb.js"), "w").write("// fake")
elif cmd == "inspect":
    box = boxes.get(args[-1])
    if box is None: sys.exit("Error: no such container " + args[-1])
    print("true" if box["running"] else "false")
elif cmd == "run":
    name = flag_value("--name")
    if name in boxes: sys.exit("Conflict: the name is in use")
    boxes[name] = {"running": False}
    serve_screen(boxes[name])
    print("f00d" * 16)
elif cmd == "start":
    box = boxes.get(args[-1])
    if box is None: sys.exit("no such container")
    if not box["running"]: serve_screen(box)
elif cmd == "stop":
    box = boxes.get(args[-1])
    if box is None: sys.exit("no such container")
    kill(box); box["running"] = False
elif cmd == "rm":
    box = boxes.pop(args[-1], None)
    if box: kill(box)
elif cmd == "port":
    box = boxes.get(args[1])
    if not box or not box["running"]: sys.exit("no port")
    print(f"127.0.0.1:{box['port']}")
elif cmd == "exec":
    rest = args[1:]
    # Skip exec's own flags to the container's name, then the command.
    i = 0
    while rest[i].startswith("-"):
        i += 1 if rest[i] == "-d" else 2
    name, command = rest[i], rest[i + 1:]
    box = boxes.get(name)
    if not box or not box["running"]: sys.exit("container is not running")
    if command[0] == "timeout":
        script = command[command.index("-lc") + 1]
        done = subprocess.run(["bash", "-c", script], cwd=tempfile.gettempdir(), capture_output=True, timeout=int(command[3]))
        sys.stdout.buffer.write(done.stdout); sys.stderr.buffer.write(done.stderr)
        save(state); sys.exit(done.returncode)
    elif command[0] == "import":
        sys.stdout.buffer.write(b"\xff\xd8\xff\xe0fake-jpeg")
else:
    sys.exit("fake engine: unknown command " + " ".join(args))
save(state)
