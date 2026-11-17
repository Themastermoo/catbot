"""A tiny MCP server for tests (stage 16.1). Run plain, it speaks over its input and output; with
--http it serves Streamable HTTP on 127.0.0.1 (printing its port) and wants
"Authorization: Bearer good". Its tools:
  lookup     (read-only) answers "found <q>"
  make_note  (no read-only mark) appends its text to the file FAKE_MCP_LOG names, and says so
With --http, tools/list answers as server-sent events, and the session id is checked.
"""
import json, os, sys, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

TOOLS = [
    {"name": "lookup", "description": "Look something up.", "inputSchema": {"type": "object", "properties": {"q": {"type": "string"}}, "required": ["q"]}, "annotations": {"readOnlyHint": True}},
    {"name": "make_note", "description": "Write a note down.", "inputSchema": {"type": "object", "properties": {"text": {"type": "string"}}, "required": ["text"]}},
]

def handle(msg):
    method, mid = msg.get("method"), msg.get("id")
    if mid is None:
        return None
    if method == "initialize":
        result = {"protocolVersion": msg["params"].get("protocolVersion"), "capabilities": {"tools": {}}, "serverInfo": {"name": "fake", "version": "1"}}
    elif method == "tools/list":
        # Two pages, to follow the cursor.
        if (msg.get("params") or {}).get("cursor") == "page2":
            result = {"tools": TOOLS[1:]}
        else:
            result = {"tools": TOOLS[:1], "nextCursor": "page2"}
    elif method == "tools/call":
        name, args = msg["params"]["name"], msg["params"].get("arguments") or {}
        if name == "lookup":
            result = {"content": [{"type": "text", "text": f"found {args.get('q')}"}], "isError": False}
        elif name == "make_note":
            with open(os.environ["FAKE_MCP_LOG"], "a") as f:
                f.write(args.get("text", "") + "\n")
            result = {"content": [{"type": "text", "text": "noted"}], "isError": False}
        else:
            return {"jsonrpc": "2.0", "id": mid, "error": {"code": -32602, "message": f"no tool {name}"}}
    else:
        return {"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": "no such method"}}
    return {"jsonrpc": "2.0", "id": mid, "result": result}

if "--http" not in sys.argv:
    for line in sys.stdin:
        if not line.strip():
            continue
        out = handle(json.loads(line))
        if out is not None:
            # A ping from the server first, which Catbot should answer and skip past.
            if out.get("result", {}).get("tools") is not None:
                print(json.dumps({"jsonrpc": "2.0", "id": "srv-1", "method": "ping"}), flush=True)
            print(json.dumps(out), flush=True)
    sys.exit(0)

SESSION = "sess-42"

class H(BaseHTTPRequestHandler):
    def log_message(self, *a): pass
    def do_POST(self):
        if self.headers.get("authorization") != "Bearer good":
            self.send_response(401); self.end_headers(); return
        msg = json.loads(self.rfile.read(int(self.headers.get("content-length") or 0)))
        if msg.get("method") != "initialize" and self.headers.get("mcp-session-id") != SESSION:
            self.send_response(400); self.end_headers(); self.wfile.write(b"no session"); return
        out = handle(msg)
        if out is None:
            self.send_response(202); self.end_headers(); return
        body = json.dumps(out)
        if msg.get("method") == "tools/list":
            data = f"event: message\ndata: {body}\n\n".encode()
            self.send_response(200); self.send_header("content-type", "text/event-stream")
        else:
            data = body.encode()
            self.send_response(200); self.send_header("content-type", "application/json")
        self.send_header("mcp-session-id", SESSION)
        self.send_header("content-length", str(len(data))); self.end_headers(); self.wfile.write(data)

server = ThreadingHTTPServer(("127.0.0.1", 0), H)
print(server.server_address[1], flush=True)
server.serve_forever()
