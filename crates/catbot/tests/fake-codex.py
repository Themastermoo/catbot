#!/usr/bin/env python3
"""A stand-in for `codex app-server` (stage 12.4's tests), speaking the shapes of codex-cli 0.170's
app-server protocol: one JSON-RPC message a line on stdin and stdout.

Logs to FAKE_CODEX_LOG: first {"home", "cwd"}, then every message Catbot sends. A sign-in by code
writes auth.json into CODEX_HOME. A turn's text picks what happens: "run" asks to run a command,
"edit" asks to change notes.md, "fail" fails as signed out, "quota" is
turned away at the plan's usage limit; anything else is echoed.
"""
import json, os, sys, time

home = os.environ.get("CODEX_HOME", "")
log_path = os.environ.get("FAKE_CODEX_LOG")
def log(entry):
    if log_path:
        with open(log_path, "a") as f:
            f.write(json.dumps(entry) + "\n")
log({"home": home, "cwd": os.getcwd(), "args": sys.argv[1:]})

def send(message):
    sys.stdout.write(json.dumps(message) + "\n")
    sys.stdout.flush()

def note(method, params):
    send({"method": method, "params": params})

pending = []
steered = []
next_id = [100]
def ask(method, params):
    """A request of Codex's own; waits for Catbot's answer (other messages wait their turn)."""
    next_id[0] += 1
    rid = next_id[0]
    send({"id": rid, "method": method, "params": params})
    while True:
        line = sys.stdin.readline()
        if not line:
            sys.exit(0)
        message = json.loads(line)
        log(message)
        if message.get("id") == rid and "method" not in message:
            return message.get("result", {})
        # Steered while it waits (turn/steer, stage 29.14): taken in now, for the reply.
        if message.get("method") == "turn/steer":
            steered.append(message["params"]["input"][0]["text"])
            send({"id": message["id"], "result": {"turnId": message["params"].get("expectedTurnId")}})
            continue
        pending.append(message)

def auth():
    try:
        return json.load(open(os.path.join(home, "auth.json")))
    except Exception:
        return None

threads = set()
def turn(params, tid):
    thread = params["threadId"]
    text = params["input"][0]["text"]
    send_turn = lambda status, error=None: note("turn/completed", {"threadId": thread, "turn": {"id": tid, "status": status, "items": [], "error": error}})
    base = {"threadId": thread, "turnId": tid}
    def say(words, item="m1"):
        note("item/started", dict(base, item={"type": "agentMessage", "id": item, "text": ""}, startedAtMs=0))
        for part in [words[:5], words[5:]]:
            if part:
                note("item/agentMessage/delta", dict(base, itemId=item, delta=part))
        note("item/completed", dict(base, item={"type": "agentMessage", "id": item, "text": words}))
    if "quota" in text:
        said = "You've hit your usage limit. Upgrade to Pro (https://chatgpt.com/explore/pro) or try again at 5:23 PM."
        note("error", dict(base, error={"message": said, "codexErrorInfo": "usageLimitExceeded"}, willRetry=False))
        return send_turn("failed", {"message": said, "codexErrorInfo": "usageLimitExceeded"})
    if "fail" in text:
        # Like codex-cli 0.170: an error notice, then the failed turn with the same error.
        note("error", dict(base, error={"message": "unexpected status 401 Unauthorized"}, willRetry=False))
        return send_turn("failed", {"message": "unexpected status 401 Unauthorized"})
    if "run" in text:
        note("item/started", dict(base, item={"type": "commandExecution", "id": "c1", "command": "ls", "commandActions": [], "cwd": os.getcwd(), "status": "inProgress"}, startedAtMs=0))
        answer = ask("item/commandExecution/requestApproval", dict(base, itemId="c1", command="ls", reason="list the folder"))
        if answer.get("decision") == "accept":
            note("item/completed", dict(base, item={"type": "commandExecution", "id": "c1", "command": "ls", "status": "completed", "exitCode": 0, "aggregatedOutput": "a.md\n"}))
            say("Ran it." + (" Steered: " + " / ".join(steered) if steered else ""))
        else:
            note("item/completed", dict(base, item={"type": "commandExecution", "id": "c1", "command": "ls", "status": "declined"}))
            say("Didn't run it.")
        return send_turn("completed")
    if "edit" in text:
        path = os.path.join(os.getcwd(), "notes.md")
        change = {"type": "fileChange", "id": "f1", "changes": [{"path": path, "diff": "+hello", "kind": {"type": "add"}}], "status": "inProgress"}
        note("item/started", dict(base, item=change, startedAtMs=0))
        policy = threads_policy.get(thread, "")
        answer = {"decision": "accept"} if policy == "workspace-write" else ask("item/fileChange/requestApproval", dict(base, itemId="f1", startedAtMs=0))
        if answer.get("decision") == "accept":
            open(path, "w").write("hello from codex")
            note("item/completed", dict(base, item=dict(change, status="completed")))
            say("Edited.")
        else:
            note("item/completed", dict(base, item=dict(change, status="declined")))
            say("Not edited.")
        return send_turn("completed")
    note("item/reasoning/summaryTextDelta", dict(base, itemId="r1", delta="hmm", summaryIndex=0))
    word = text.split("\n")[-1]
    say(f"Echo: {word} ({'resumed' if thread in resumed else 'new'})")
    note("thread/tokenUsage/updated", dict(base, tokenUsage={"last": {"inputTokens": 120, "cachedInputTokens": 20, "outputTokens": 30, "reasoningOutputTokens": 5, "totalTokens": 150}, "total": {"inputTokens": 120, "cachedInputTokens": 20, "outputTokens": 30, "reasoningOutputTokens": 5, "totalTokens": 150}}))
    send_turn("completed")

threads_policy = {}
resumed = set()
while True:
    message = pending.pop(0) if pending else None
    if message is None:
        line = sys.stdin.readline()
        if not line:
            break
        message = json.loads(line)
        log(message)
    method, mid, params = message.get("method"), message.get("id"), message.get("params", {})
    if mid is None:
        continue
    if method == "initialize":
        send({"id": mid, "result": {"userAgent": "fake-codex/0.170.0"}})
    elif method == "account/read":
        a = auth()
        account = {"type": "chatgpt", "email": "moo@example.com", "planType": "plus"} if a else None
        send({"id": mid, "result": {"account": account, "requiresOpenaiAuth": True}})
    elif method == "account/login/start":
        send({"id": mid, "result": {"type": "chatgptDeviceCode", "loginId": "L1", "verificationUrl": "https://auth.openai.com/codex/device", "userCode": "ABCD-1234"}})
        time.sleep(0.3)
        json.dump({"auth_mode": "chatgpt", "tokens": {"id_token": "x"}}, open(os.path.join(home, "auth.json"), "w"))
        note("account/login/completed", {"loginId": "L1", "success": True})
    elif method == "account/rateLimits/read":
        now = int(time.time())
        send({"id": mid, "result": {"rateLimits": {"planType": "plus",
            "primary": {"usedPercent": 42, "windowDurationMins": 300, "resetsAt": now + 3600},
            "secondary": {"usedPercent": 10, "windowDurationMins": 10080, "resetsAt": now + 86400}}}})
    elif method == "model/list":
        send({"id": mid, "result": {"data": [{"id": "gpt-5.5-codex", "displayName": "GPT-5.5 Codex", "hidden": False}, {"id": "secret", "displayName": "Hidden", "hidden": True}], "nextCursor": None}})
    elif method in ("thread/start", "thread/resume"):
        if not auth():
            send({"id": mid, "error": {"code": -32000, "message": "not logged in"}})
            continue
        if method == "thread/resume":
            if params.get("threadId") not in ("th-1",):
                send({"id": mid, "error": {"code": -32600, "message": "no rollout found"}})
                continue
            thread = params["threadId"]
            resumed.add(thread)
        else:
            thread = "th-1"
        threads_policy[thread] = params.get("sandbox")
        send({"id": mid, "result": {"thread": {"id": thread}, "model": "gpt-5.5-codex", "sandbox": params.get("sandbox")}})
    elif method == "turn/start":
        tid = f"turn-{time.time_ns()}"
        send({"id": mid, "result": {"turn": {"id": tid, "status": "inProgress", "items": []}}})
        turn(params, tid)
    else:
        send({"id": mid, "error": {"code": -32601, "message": f"unknown method {method}"}})
