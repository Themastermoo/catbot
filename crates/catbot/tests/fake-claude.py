"""A stand-in for the `claude` CLI: speaks the same stream-json, answers without an account.

The prompt picks the behaviour:
  "crash"  -> writes to stderr and exits 1 with no result line
  "tool"   -> calls a tool that is refused, like an unapproved Bash command
  "approve" -> asks the permission prompt tool (as the real CLI does), and obeys the answer
  "write <file>: <words>" -> writes the words to that file in its folder (as a bot's edit would)
  "Name the conversation…" -> a chat's name, the way Catbot asks for one
  anything else -> streams "Hello, <prompt>" back
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

if prompt == "crash":
    print("Error: something broke inside the CLI", file=sys.stderr)
    sys.exit(1)

out({"type": "system", "subtype": "init", "session_id": session, "model": arg_after("--model") or "claude-fake",
     "permissionMode": arg_after("--permission-mode"), "args": sys.argv[1:]})
out({"type": "user", "isReplay": True, "message": message["message"]})
denials = []
if prompt == "tool":
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t1", "name": "Bash", "input": {"command": "rm -rf /tmp/x"}}]}})
    out({"type": "user", "message": {"content": [{"type": "tool_result", "tool_use_id": "t1", "is_error": True,
         "content": "This command requires approval"}]}})
    denials = [{"tool_name": "Bash", "tool_use_id": "t1"}]
if prompt == "approve":
    tool_input = {"command": "echo hi"}
    out({"type": "assistant", "message": {"content": [{"type": "tool_use", "id": "t2", "name": "Bash", "input": tool_input}]}})
    verdict = ask_permission("Bash", tool_input)
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
     "duration_ms": 850, "stop_reason": "end_turn", "permission_denials": denials})
