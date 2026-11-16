"""A stand-in agent program speaking the Agent Client Protocol, for Catbot's tests.

session/prompt behaviour by the message's last line:
  contains "edit" -> a tool call that asks permission, then (if allowed) writes notes.md through
                     Catbot's fs/write_text_file, and says what happened
  anything else   -> thinks briefly, then answers "Echo: <line> (new|resumed)"; "new" when the
                     prompt carried Catbot's instructions (a new session)
"""
import json, os, sys

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n"); sys.stdout.flush()

if os.environ.get("FAKE_ACP_LOG"):
    with open(os.environ["FAKE_ACP_LOG"], "a") as log:
        log.write(json.dumps({"args": sys.argv[1:], "cwd": os.getcwd()}) + "\n")

next_id = [1000]
def ask(method, params):
    next_id[0] += 1
    rid = next_id[0]
    send({"jsonrpc": "2.0", "id": rid, "method": method, "params": params})
    for line in sys.stdin:
        msg = json.loads(line)
        if msg.get("id") == rid and "method" not in msg:
            return msg
    sys.exit(0)

def update(session, u):
    send({"jsonrpc": "2.0", "method": "session/update", "params": {"sessionId": session, "update": u}})

def chunk(session, text, kind="agent_message_chunk"):
    update(session, {"sessionUpdate": kind, "content": {"type": "text", "text": text}})

for line in sys.stdin:
    msg = json.loads(line)
    method, rid, params = msg.get("method"), msg.get("id"), msg.get("params", {})
    if method == "initialize":
        send({"jsonrpc": "2.0", "id": rid, "result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}, "authMethods": []}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": "fake-session-1"}})
    elif method == "session/load":
        if params.get("sessionId", "").startswith("fake-session"):
            send({"jsonrpc": "2.0", "id": rid, "result": {}})
        else:
            send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32602, "message": "no such session"}})
    elif method == "session/prompt":
        session = params["sessionId"]
        text = params["prompt"][0]["text"]
        last = text.strip().splitlines()[-1]
        if "edit" in last:
            path = os.path.join(os.getcwd(), "notes.md")
            update(session, {"sessionUpdate": "tool_call", "toolCallId": "t1", "title": "Write notes.md", "kind": "edit", "status": "pending", "locations": [{"path": path}]})
            answer = ask("session/request_permission", {"sessionId": session, "toolCall": {"toolCallId": "t1", "title": "Write notes.md", "kind": "edit", "locations": [{"path": path}]},
                "options": [{"optionId": "yes", "name": "Allow", "kind": "allow_once"}, {"optionId": "no", "name": "Reject", "kind": "reject_once"}]})
            if answer["result"]["outcome"].get("optionId") == "yes":
                wrote = ask("fs/write_text_file", {"sessionId": session, "path": path, "content": "hello from the agent"})
                ok = "error" not in wrote
                update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "completed" if ok else "failed"})
                chunk(session, "Edited." if ok else "Couldn't write.")
            else:
                update(session, {"sessionUpdate": "tool_call_update", "toolCallId": "t1", "status": "failed"})
                chunk(session, "Not allowed.")
        else:
            chunk(session, "hmm", "agent_thought_chunk")
            state = "new" if text.startswith("<catbot_instructions>") else "resumed"
            chunk(session, "Echo: ")
            chunk(session, f"{last} ({state})")
        send({"jsonrpc": "2.0", "id": rid, "result": {"stopReason": "end_turn"}})
