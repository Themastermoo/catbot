#!/usr/bin/env python3
"""A stand-in for Cloudflare's wrangler (stage 13.12.2 tests): logs each run to FAKE_WRANGLER_LOG
as a JSON line (its words, the token it was given, and for a deploy the files it was given)."""
import json, os, sys

log = os.environ["FAKE_WRANGLER_LOG"]
args = sys.argv[1:]
entry = {"args": args, "token": os.environ.get("CLOUDFLARE_API_TOKEN"), "account": os.environ.get("CLOUDFLARE_ACCOUNT_ID")}
if args[:2] == ["pages", "deploy"]:
    folder = args[2]
    entry["files"] = sorted(os.path.relpath(os.path.join(d, f), folder).replace(os.sep, "/") for d, _, fs in os.walk(folder) for f in fs)
    with open(os.path.join(folder, "index.html")) as f:
        entry["index"] = f.read()
with open(log, "a") as f:
    f.write(json.dumps(entry) + "\n")
if args[:3] == ["pages", "project", "list"]:
    made = []
    with open(log) as f:
        for line in f:
            a = json.loads(line)["args"]
            if a[:3] == ["pages", "project", "create"]:
                made.append(a[3])
    print("Some banner first")
    print(json.dumps([{"Project Name": n, "Project Domains": f"{n}-x1.pages.dev, example.com"} for n in made]))
elif args[:3] == ["pages", "project", "delete"] and "--yes" not in args:
    print("would ask", file=sys.stderr)
    sys.exit(1)
else:
    print("✨ Success!")
