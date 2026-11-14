"""A stand-in for the `claude` CLI: speaks the same stream-json, answers without an account.

The prompt picks the behaviour:
  "crash"  -> writes to stderr and exits 1 with no result line
  "tool"   -> calls a tool that is refused, like an unapproved Bash command
  anything else -> streams "Hello, <prompt>" back
"""
import json, sys

def arg_after(flag):
    return sys.argv[sys.argv.index(flag) + 1] if flag in sys.argv else None

def out(obj):
    print(json.dumps(obj), flush=True)

session = arg_after("--session-id") or arg_after("--resume") or "none"
message = json.loads(sys.stdin.readline())
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
reply = f"Hello, {prompt}"
for piece in (reply[:3], reply[3:]):
    out({"type": "stream_event", "parent_tool_use_id": None,
         "event": {"type": "content_block_delta", "delta": {"type": "text_delta", "text": piece}}})
out({"type": "assistant", "message": {"content": [{"type": "text", "text": reply}]}})
out({"type": "result", "is_error": False, "result": reply, "session_id": session, "total_cost_usd": 0.0012,
     "duration_ms": 850, "stop_reason": "end_turn", "permission_denials": denials})
