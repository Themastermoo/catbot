#!/usr/bin/env python3
"""A stand-in for the 1Password CLI (stage 17.3's tests), as CATBOT_OP="python3 fake-op.py".

One service account token works (ops_good1234), reading a vault called Bots that holds a GitHub
login. Every command line is logged to FAKE_OP_LOG, without the token.
"""
import json, os, sys

args = sys.argv[1:]
if os.environ.get("FAKE_OP_LOG"):
    with open(os.environ["FAKE_OP_LOG"], "a") as log:
        log.write(json.dumps(args) + "\n")
if os.environ.get("OP_SERVICE_ACCOUNT_TOKEN") != "ops_good1234":
    sys.exit("[ERROR] 2026/11/17 18:00:00 invalid service account token")
vault = {"id": "v1", "name": "Bots"}
item = {"id": "i1", "title": "GitHub", "category": "LOGIN", "vault": vault}
fields = [
    {"id": "username", "label": "username", "type": "STRING", "value": "moo-bot", "reference": "op://Bots/GitHub/username"},
    {"id": "password", "label": "password", "type": "CONCEALED", "value": "hunter2-secret-value", "reference": "op://Bots/GitHub/password"},
]
if args[:2] == ["vault", "list"]:
    print(json.dumps([vault]))
elif args[:2] == ["item", "list"]:
    print(json.dumps([item]))
elif args[:2] == ["item", "get"] and args[2] == "i1":
    print(json.dumps(dict(item, fields=fields)))
elif args[:1] == ["read"]:
    ref = args[-1]
    found = next((f for f in fields if f["reference"] == ref), None)
    if not found:
        sys.exit(f'[ERROR] could not read secret "{ref}": no item matched')
    sys.stdout.write(found["value"])
else:
    sys.exit("[ERROR] unknown command " + " ".join(args))
