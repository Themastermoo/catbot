#!/usr/bin/env python3
"""A stand-in for Pi (stage 12.4.4's tests): `pi --list-models`, and `pi --mode rpc` as Catbot
drives it, with Catbot's extension asking for approvals the way the real one does.

It logs its arguments, folder and Catbot's settings, then every record Catbot sends, to the file
FAKE_PI_LOG names (JSON a line). The prompt decides what it does:
  "please run"   runs `ls` through bash, asking first unless the level is "full"
  "please edit"  writes notes.md, asking first on "ask"
  "fail"         ends with Pi's error for a provider with no key
  anything else  thinks "hmm" and echoes the message's last line, saying whether its session is new
"""
import json, os, sys

LOG = os.environ.get("FAKE_PI_LOG")

def log(record):
    if LOG:
        with open(LOG, "a") as f:
            f.write(json.dumps(record) + "\n")

def out(record):
    sys.stdout.write(json.dumps(record) + "\n")
    sys.stdout.flush()

def arg(name):
    args = sys.argv[1:]
    return args[args.index(name) + 1] if name in args else None

def read():
    line = sys.stdin.readline()
    if not line:
        sys.exit(0)
    record = json.loads(line)
    log({"got": record})
    return record

def reply(text, stop="stop", error=None):
    for piece in (text[: len(text) // 2], text[len(text) // 2 :]):
        if piece:
            out({"type": "message_update", "assistantMessageEvent": {"type": "text_delta", "delta": piece}})
    message = {"role": "assistant", "content": [{"type": "text", "text": text}] if text else [], "stopReason": stop,
               "usage": {"input": 120, "output": 9, "cacheRead": 30, "cacheWrite": 0, "totalTokens": 159}}
    if error:
        message["errorMessage"] = error
    out({"type": "message_end", "message": message})

def ask(tool, input):
    out({"type": "extension_ui_request", "id": "ui-1", "method": "confirm", "title": "catbot-approval",
         "message": json.dumps({"tool": tool, "input": input})})
    answer = read()
    return answer.get("type") == "extension_ui_response" and answer.get("confirmed") is True

def main():
    if "--list-models" in sys.argv:
        print("provider  model  context  max-out  thinking  images")
        print("openai    gpt-5  400K     128K     yes       yes")
        return
    mode = os.environ.get("CATBOT_PI_MODE", "")
    folder, session = arg("--session-dir"), arg("--session-id")
    path = os.path.join(folder, session + ".jsonl")
    new = not os.path.exists(path)
    open(path, "a").close()
    extension = arg("-e")
    log({"args": sys.argv[1:], "cwd": os.getcwd(), "mode": mode, "mcp": os.environ.get("CATBOT_PI_MCP", ""),
         "extension": open(extension).read() if extension and os.path.exists(extension) else None,
         "instructions": open(arg("--append-system-prompt")).read()})
    prompt = read()
    out({"id": prompt.get("id"), "type": "response", "command": "prompt", "success": True, "data": {"disposition": "started"}})
    out({"type": "agent_start"})
    text = prompt.get("message", "")
    if "please run" in text:
        out({"type": "tool_execution_start", "toolCallId": "t1", "toolName": "bash", "args": {"command": "ls"}})
        ok = mode == "full" or ask("bash", {"command": "ls"})
        out({"type": "tool_execution_end", "toolCallId": "t1", "toolName": "bash",
             "result": {"content": [{"type": "text", "text": "notes.md" if ok else "The person said no."}]}, "isError": not ok})
        reply("Ran it." if ok else "Didn't run it.")
    elif "please edit" in text:
        out({"type": "tool_execution_start", "toolCallId": "t2", "toolName": "write", "args": {"path": "notes.md", "content": "hello from pi"}})
        ok = mode in ("edits", "full") or ask("write", {"path": "notes.md", "content": "hello from pi"})
        if ok:
            with open("notes.md", "w") as f:
                f.write("hello from pi")
        out({"type": "tool_execution_end", "toolCallId": "t2", "toolName": "write",
             "result": {"content": [{"type": "text", "text": "Wrote notes.md" if ok else "The person said no."}]}, "isError": not ok})
        reply("Edited." if ok else "Didn't edit it.")
    elif text.strip().endswith("fail"):
        reply("", stop="error", error="No API key for provider: openai")
    else:
        out({"type": "message_update", "assistantMessageEvent": {"type": "thinking_delta", "delta": "hmm"}})
        last = text.strip().splitlines()[-1]
        reply(f"Echo: {last} ({'new' if new else 'resumed'})")
    out({"type": "agent_end"})
    out({"type": "agent_settled", "aborted": False})
    # Pi stays up for more commands until its input closes.
    while sys.stdin.readline():
        pass

main()
