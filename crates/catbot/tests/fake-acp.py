"""A stand-in agent program speaking the Agent Client Protocol, for Catbot's tests.

session/load replays the conversation before it answers, as ACP has agents do (and Grok CLI
does): the person's earlier message, a tool call and the earlier reply, as session updates.

session/prompt behaviour by the message's last line:
  contains "look" -> says "Looking.", reads a file (a tool call), then says "Found it."
  contains "slow" -> says "working.", waits up to 5 s for a second session/prompt (a steer, as Grok
                     takes one), answers it as removed from the queue, then " done; steered: <it>"
  contains "quota" -> refuses the prompt: out of quota (RESOURCE_EXHAUSTED), as Gemini's agents do
  contains "edit" -> a tool call that asks permission, then (if allowed) writes notes.md through
                     Catbot's fs/write_text_file, and says what happened
  anything else   -> thinks briefly, then answers "Echo: <line> (new|resumed)"; "new" when the
                     prompt carried Catbot's instructions (a new session)

As Antigravity (FAKE_ACP_GOOGLE set): offers "oauth-personal"; authenticate prints a sign-in link
on stderr, as the real one does, whose redirect_uri is a listener here; a GET to it with a code
writes $GEMINI_HOME/antigravity-acp/acp_token.json and answers authenticate.

As Muse Code (FAKE_ACP_META set): offers "muse-login"; authenticate shows a device code through
ACP's URL elicitation (refused, as the real one does, when the client can't show links), then
waits for the file FAKE_ACP_APPROVE names (the person approving at Meta's page) before it
answers. Until then, session/new is refused.
"""
import time
import json, os, sys, socket, urllib.parse

def send(obj):
    sys.stdout.write(json.dumps(obj) + "\n"); sys.stdout.flush()

if os.environ.get("FAKE_ACP_LOG"):
    with open(os.environ["FAKE_ACP_LOG"], "a") as log:
        log.write(json.dumps({"args": sys.argv[1:], "cwd": os.getcwd(), "env": {k: v for k, v in os.environ.items() if k in ("GEMINI_HOME", "ANTIGRAVITY_HARNESS_PATH", "AGY_ACP_FORCE_FILE_STORAGE", "MUSE_EXPERIMENTAL_ACP")}}) + "\n")

def meta_sign_in(rid, can_show):
    code = "WVCW-VRLT"
    url = f"https://auth.meta.com/oauth/device/?code={code}"
    if not can_show:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32603, "message": f"This editor cannot show the Muse login link: open {url}", "data": {"reason": "url_elicitation_unsupported"}}})
        return
    answer = ask("elicitation/create", {"elicitationId": "e1", "mode": "url", "requestId": rid, "url": url,
                                        "message": f"Open the link to log in to Muse, and enter the code {code} there."})
    if answer.get("result", {}).get("action") != "accept":
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32000, "message": "login declined"}})
        return
    for _ in range(200):
        if os.path.exists(os.environ["FAKE_ACP_APPROVE"]):
            send({"jsonrpc": "2.0", "id": rid, "result": {}})
            return
        time.sleep(0.05)
    send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32000, "message": "the code expired"}})

def google_sign_in(rid):
    listener = socket.socket(); listener.bind(("127.0.0.1", 0)); listener.listen(1)
    port = listener.getsockname()[1]
    back = urllib.parse.quote(f"http://localhost:{port}/oauth2callback", safe="")
    sys.stderr.write(f"Open the following link to authenticate the ACP server: https://accounts.google.com/o/oauth2/v2/auth?client_id=x&redirect_uri={back}&state=s1\n")
    sys.stderr.flush()
    conn, _ = listener.accept()
    request = conn.recv(4096).decode()
    ok = "code=" in request.split("\r\n")[0]
    conn.sendall(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok"); conn.close()
    if ok:
        folder = os.path.join(os.environ["GEMINI_HOME"], "antigravity-acp")
        os.makedirs(folder, exist_ok=True)
        open(os.path.join(folder, "acp_token.json"), "w").write('{"refresh_token": "fake"}')
        send({"jsonrpc": "2.0", "id": rid, "result": {}})
    else:
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32000, "message": "access_denied"}})

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

# Its thinking levels (ACP's thought_level config option); a level set is logged.
def options(level, model="fake-pro"):
    return [{"id": "reasoning", "name": "Thinking", "category": "thought_level", "type": "select", "currentValue": level,
             "options": [{"value": "low", "name": "Light"}, {"value": "medium", "name": "Medium"}, {"value": "high", "name": "Deep"}]},
            {"id": "model", "name": "Model", "category": "model", "type": "select", "currentValue": model,
             "options": [{"value": "fake-pro", "name": "Fake Pro"}, {"value": "fake-flash", "name": "Fake Flash"}]}]

def logged(entry):
    if os.environ.get("FAKE_ACP_LOG"):
        with open(os.environ["FAKE_ACP_LOG"], "a") as log:
            log.write(json.dumps(entry) + "\n")

for line in sys.stdin:
    msg = json.loads(line)
    method, rid, params = msg.get("method"), msg.get("id"), msg.get("params", {})
    if method == "initialize":
        can_show = bool(params.get("clientCapabilities", {}).get("elicitation", {}).get("url") is not None)
        methods = [{"id": "oauth-personal", "name": "Google"}] if os.environ.get("FAKE_ACP_GOOGLE") else []
        if os.environ.get("FAKE_ACP_META"):
            methods = [{"id": "muse-login", "name": "Log in to Muse"}]
        send({"jsonrpc": "2.0", "id": rid, "result": {"protocolVersion": 1, "agentCapabilities": {"loadSession": True}, "authMethods": methods}})
    elif method == "authenticate" and os.environ.get("FAKE_ACP_META"):
        meta_sign_in(rid, can_show)
    elif method == "authenticate":
        google_sign_in(rid)
    elif method == "session/new" and os.environ.get("FAKE_ACP_META") and not os.path.exists(os.environ["FAKE_ACP_APPROVE"]):
        send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32000, "message": "log in to Muse before opening a session"}})
    elif method == "session/new":
        send({"jsonrpc": "2.0", "id": rid, "result": {"sessionId": "fake-session-1", "configOptions": options("medium")}})
    elif method == "session/set_config_option":
        logged({"set": params.get("configId"), "value": params.get("value"), "session": params.get("sessionId")})
        send({"jsonrpc": "2.0", "id": rid, "result": {"configOptions": options(params.get("value"))}})
    elif method == "session/load":
        if params.get("sessionId", "").startswith("fake-session"):
            session = params["sessionId"]
            chunk(session, "hello", "user_message_chunk")
            update(session, {"sessionUpdate": "tool_call", "toolCallId": "old-1", "title": "Read notes.md", "kind": "read", "status": "completed"})
            chunk(session, "Echo: hello (new)")
            send({"jsonrpc": "2.0", "id": rid, "result": {"configOptions": options("medium")}})
        else:
            send({"jsonrpc": "2.0", "id": rid, "error": {"code": -32602, "message": "no such session"}})
    elif method == "session/prompt":
        session = params["sessionId"]
        text = params["prompt"][0]["text"]
        last = text.strip().splitlines()[-1]
        if "quota" in last:
            send({"jsonrpc": "2.0", "id": rid, "error": {"code": 429, "message": "RESOURCE_EXHAUSTED: Quota exceeded for this account. Try again later."}})
            continue
        if "slow" in last:
            # As Grok steering (stage 32.11): a second prompt while this one runs is taken in at
            # the next step, and answered as removed from the queue.
            import select
            chunk(session, "working.")
            steered = None
            ready, _, _ = select.select([sys.stdin], [], [], 5)
            if ready:
                extra = json.loads(sys.stdin.readline())
                if extra.get("method") == "session/prompt":
                    steered = extra["params"]["prompt"][0]["text"]
                    send({"jsonrpc": "2.0", "id": extra["id"], "result": {"stopReason": "cancelled", "_meta": {"completionKind": "removedFromQueue"}}})
            chunk(session, f" done; steered: {steered}" if steered else " done.")
        elif "look" in last:
            chunk(session, "Looking.")
            update(session, {"sessionUpdate": "tool_call", "toolCallId": "t2", "title": "Read notes.md", "kind": "read", "status": "completed"})
            chunk(session, "Found it.")
        elif "edit" in last:
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
