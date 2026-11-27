"""A stand-in for the `claude` CLI: speaks the same stream-json, answers without an account.

The prompt picks the behaviour:
  "crash"  -> writes to stderr and exits 1 with no result line
  "tool"   -> calls a tool that is refused, like an unapproved Bash command
  "approve" -> asks the permission prompt tool (as the real CLI does), and obeys the answer;
  "question"/"questions" -> asks AskUserQuestion through it, and replies with the answers;
               user lines written meanwhile (steering) end its reply as "Steered: …"
  "draw me <what>" -> asks Catbot for a picture of it in its reply
  (asked as Catbot's drawer, for SVG or a Blender script) -> answers with one
  "limit"  -> turns the turn away at the plan's usage limit, as the real CLI does
  "write <file>: <words>" -> writes the words to that file in its folder (as a bot's edit would)
  "Name the conversation…" -> a chat's name, the way Catbot asks for one
  anything else -> streams "Hello, <prompt>" back (ending "@Name, over to you." for a bot
                   whose instructions say "fake-pass @Name" (and DONE in a direct chat, for "fake-done" or "fake-agree"); "what are your instructions?" -> its system prompt;
                   "team: {…}" -> proposes that team change; "offer a skill" -> offers one;
                   "catbot-tool: name {json}" -> calls one of Catbot's tools and says what it answered)
"""
import json, subprocess, sys

def arg_after(flag):
    return sys.argv[sys.argv.index(flag) + 1] if flag in sys.argv else None

def out(obj):
    print(json.dumps(obj), flush=True)

def ask_permission(tool_name, tool_input):
    """What the real CLI does with --permission-prompt-tool: start the MCP server, call the tool."""
    prompt_tool = arg_after("--permission-prompt-tool")
    if not prompt_tool:
        return {"behavior": "deny", "message": "This command requires approval"}
    server_name, tool = prompt_tool.split("__")[1:3]
    config = json.load(open(arg_after("--mcp-config")))["mcpServers"][server_name]
    server = subprocess.Popen([config["command"], *config["args"]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    def rpc(id, method, params):
        server.stdin.write(json.dumps({"jsonrpc": "2.0", "id": id, "method": method, "params": params}) + "\n")
        server.stdin.flush()
        return json.loads(server.stdout.readline())
    rpc(1, "initialize", {"protocolVersion": "2024-11-05"})
    tools = [t["name"] for t in rpc(2, "tools/list", {})["result"]["tools"]]
    assert tool in tools, tools
    result = rpc(3, "tools/call", {"name": tool, "arguments": {"tool_name": tool_name, "input": tool_input}})
    server.stdin.close()
    server.wait()
    return json.loads(result["result"]["content"][0]["text"])

def call_catbot_tool(name, arguments):
    """A Catbot tool, the way the real CLI reaches an MCP server's tool: start it, list, call."""
    config = json.load(open(arg_after("--mcp-config")))["mcpServers"]["catbot"]
    env = dict(os.environ, **config.get("env", {}))
    server = subprocess.Popen([config["command"], *config["args"]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, env=env)
    def rpc(id, method, params):
        server.stdin.write(json.dumps({"jsonrpc": "2.0", "id": id, "method": method, "params": params}) + "\n")
        server.stdin.flush()
        return json.loads(server.stdout.readline())
    rpc(1, "initialize", {"protocolVersion": "2024-11-05"})
    tools = [t["name"] for t in rpc(2, "tools/list", {})["result"]["tools"]]
    if name not in tools:
        server.stdin.close(); server.wait()
        return f"(no tool {name}; have {tools})"
    result = rpc(3, "tools/call", {"name": name, "arguments": arguments})["result"]
    server.stdin.close()
    server.wait()
    text = result["content"][0]["text"]
    for part in result["content"][1:]:
        if part.get("type") == "image":
            text += f" [image {part['mimeType']} {len(part['data'])}]"
    return ("ERROR: " if result.get("isError") else "") + text

import os
session = arg_after("--session-id") or arg_after("--resume") or "none"
message = json.loads(sys.stdin.readline())

# Tests can see exactly how the CLI was started.
import os
if os.environ.get("FAKE_CLAUDE_LOG"):
    with open(os.environ["FAKE_CLAUDE_LOG"], "a") as log:
        log.write(json.dumps({"args": sys.argv[1:], "cwd": os.getcwd()}) + "\n")

# The real CLI, asked to resume a conversation it no longer has, answers with an error result.
if (arg_after("--resume") or "").startswith("gone"):
    out({"type": "result", "is_error": True, "result": f"No conversation found with session ID: {arg_after('--resume')}",
         "session_id": arg_after("--resume")})
    sys.exit(1)
prompt = message["message"]["content"]

system = arg_after("--append-system-prompt") or ""
if "draw pictures as SVG" in system or "Blender Python" in system:
    out({"type": "system", "subtype": "init", "session_id": session, "model": "claude-fake"})
    if "SVG" in system and "Blender" not in system:
        drawn = '<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 200 100"><rect width="200" height="100" fill="#3a7"/><circle cx="100" cy="50" r="30" fill="#fd3"/></svg>'
    else:
        drawn = "```python\nimport bpy, sys\nbpy.ops.render.render(write_still=True)\n```"
    out({"type": "result", "is_error": False, "result": drawn, "session_id": session})
    sys.exit(0)

last_line = prompt.strip().splitlines()[-1] if prompt.strip() else ""
if last_line.startswith("draw me "):
    out({"type": "system", "subtype": "init", "session_id": session, "model": "claude-fake"})
    said = f"Coming right up.\n<make-image>{last_line[len('draw me '):]}</make-image>"
    out({"type": "assistant", "message": {"content": [{"type": "text", "text": said}]}})
    out({"type": "result", "is_error": False, "result": said, "session_id": session})
    sys.exit(0)

if prompt.startswith("limit"):
    out({"type": "system", "subtype": "init", "session_id": session, "model": "claude-fake"})
    out({"type": "result", "is_error": True, "result": "Claude AI usage limit reached|4102444800", "session_id": session})
    sys.exit(1)

if prompt == "crash":
    print("Error: something broke inside the CLI", file=sys.stderr)
    sys.exit(1)

# "fake-mcp-down" (in its instructions): Catbot's tool server didn't start, as the CLI reports it.
servers = [{"name": "catbot", "status": "failed" if "fake-mcp-down" in system else "connected"}]
out({"type": "system", "subtype": "init", "session_id": session, "model": arg_after("--model") or "claude-fake",
     "permissionMode": arg_after("--permission-mode"), "args": sys.argv[1:], "mcp_servers": servers})
out({"type": "user", "isReplay": True, "message": message["message"]})
denials = []
if prompt == "tool":
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "rm -rf /tmp/x"}}]}})
    out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "is_error": True,
         "content": "This command requires approval"}]}})
    denials = [{"tool_name": "Bash", "tool_use_id": "t1"}]
# (Also in a channel, where it's the person's last line in the talk.)
if prompt == "approve" or ("</conversation_so_far>" in prompt and prompt.rsplit("</conversation_so_far>", 1)[0].rstrip().endswith("\nPerson: approve")):
    tool_input = {"command": "echo hi"}
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t2", "name": "Bash", "input": tool_input}]}})
    verdict = ask_permission("Bash", tool_input)
    # Words the person steered in while it waited (stage 29.14): more user lines on stdin.
    import select
    steered = []
    while select.select([sys.stdin], [], [], 0.3)[0]:
        extra = sys.stdin.readline()
        if not extra:
            break
        steered.append(json.loads(extra)["message"]["content"])
    if verdict["behavior"] == "allow":
        out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t2", "content": "hi"}]}})
    else:
        out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t2", "is_error": True,
             "content": verdict["message"]}]}})
        denials = [{"tool_name": "Bash", "tool_use_id": "t2"}]
if prompt.startswith("write ") and ":" in prompt.splitlines()[0]:
    name, words = prompt.splitlines()[0][len("write "):].split(":", 1)
    with open(os.path.join(os.getcwd(), name.strip()), "w") as f:
        f.write(words.strip() + "\n")
reply = f"Hello, {prompt}"
# "question" / "questions" ask the person through AskUserQuestion (stage 32.5) and say what came back.
if prompt in ("question", "questions"):
    asked = [{"question": "Which colour?", "header": "Colour", "multiSelect": False,
              "options": [{"label": "Red", "description": "Warm"}, {"label": "Blue", "description": "Cool"}]}]
    if prompt == "questions":
        asked.append({"question": "Which beds?", "header": "Beds", "multiSelect": True,
                      "options": [{"label": "North", "description": ""}, {"label": "South", "description": ""}]})
    tool_input = {"questions": asked}
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "q1", "name": "AskUserQuestion", "input": tool_input}]}})
    verdict = ask_permission("AskUserQuestion", tool_input)
    if verdict["behavior"] == "allow":
        reply = "Answers: " + json.dumps(verdict["updatedInput"].get("answers", {}), sort_keys=True)
    else:
        reply = "Skipped: " + verdict["message"]
if "steered" in dir() and steered:
    reply += "\n\nSteered: " + " / ".join(steered)
# "fake-mode" (in its instructions) says what approval level it was run at.
if "fake-mode" in system:
    reply += f"\n\n(mode {arg_after('--permission-mode')})"
# A bot told "fake-pass @Name" (in its instructions, which come first) hands on to that bot.
import re
passing = re.search(r"fake-pass (@\w+)", system)
if passing:
    reply += f"\n\n{passing.group(1)}, over to you."
# In a direct chat between bots, "fake-done" says DONE each turn; "fake-agree" only once the other has.
if "saying just DONE" in prompt and ("fake-done" in system or ("fake-agree" in system and "say DONE too" in prompt)):
    reply += "\n\nDONE"
# "team: {…}" (the first line) proposes that team change, as a Chief of Staff would.
first_line = prompt.strip().splitlines()[0] if prompt.strip() else ""
if first_line.startswith("team: "):
    reply = f"Proposing it.\n<team-change>{first_line[len('team: '):]}</team-change>"
# "offer a skill" (the first line) offers one, as a bot that worked out a way would.
if first_line == "offer a skill":
    reply = "This will come up again.\n<save-skill>---\nname: weed-check\ndescription: Check the beds for weeds.\n---\n\nWalk each bed.\n</save-skill>"
# "catbot-tool: name {json}" (the first line) calls one of Catbot's own tools and replies with what it said.
if first_line.startswith("catbot-tool: "):
    name, _, arguments = first_line[len("catbot-tool: "):].partition(" ")
    reply = call_catbot_tool(name, json.loads(arguments or "{}"))
# "what are your instructions?" (the last line) gets the system prompt it was given, to check what a bot is told.
if last_line == "what are your instructions?":
    reply = system
# Catbot asking what the person might say next (stage 32.10).
if prompt.startswith("Suggest the next message"):
    reply = "- \"Send it to Priya too.\"" if "Priya" in prompt or "beds" in prompt else "NONE"
# Catbot asking for a chat's name ("sleepy" in the message makes the namer slow).
if prompt.startswith("Name the conversation"):
    if "sleepy" in prompt:
        import time
        time.sleep(2)
    reply = '"Plans for the garden."'
for piece in (reply[:3], reply[3:]):
    out({"type": "stream_event", "parent_tool_use_id": None,
         "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": piece}}})
out({"type": "assistant", "message": {"content": [{"type": "text", "text": reply}]}})
out({"type": "result", "is_error": False, "result": reply, "session_id": session, "total_cost_usd": 0.0012,
     "usage": {"input_tokens": 10 + len(prompt) // 4, "output_tokens": 5 + len(reply) // 4, "cache_read_input_tokens": 1000,
               "cache_creation_input_tokens": 200},
     "duration_ms": 850, "stop_reason": "end_turn", "permission_denials": denials})
