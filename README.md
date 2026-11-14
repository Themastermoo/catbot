# Catbot

Working name. A self-hosted team of AI bots in a chat app: a Rust server on a
home Linux box runs the agents (`claude`, `codex`), and any paired device
(Windows PC, Android phone, a browser) connects to it over Tailscale.

## Plan

- **Look:** the React interface from
  [OpenMausBot](https://github.com/milind-soni/OpenMausBot) (Apache 2.0), served
  unchanged by the Rust server.
- **Server:** written in Rust (tokio, axum, SQLite), speaking the same protocol
  the interface expects.
- **Migration:** route by route. Routes the Rust server doesn't handle yet are
  passed through to the original Node server, which shrinks to nothing.
- **Access:** Tailscale only, never a forwarded port. Every device pairs with a
  one-time code and can be revoked. Bots run as their own Linux user.

## Layout

```
~/Projects/
├── Catbot/            this repo
└── openmausbot-ref/   read-only clone of the original, for reference and fallback
```

Runtime data (database, transcripts, keys) lives outside this repo, never in it.

## Talking to Claude through Catbot

`catbot ask` runs one turn through Catbot's own Claude driver (`crates/catbot/src/claude.rs`),
the same piece every bot will run on.

```sh
cargo run -- ask "What's in this folder?"
cargo run -- ask --resume <session id> "And the biggest file?"   # continue a conversation
cargo run -- ask --model opus --system "You are terse." "Explain Rust lifetimes"
```

Bots are isolated from this machine's own Claude setup (CLAUDE.md, MCP servers, plugins)
unless `--inherit-user-config` is given. With the default `--mode default`, anything that
would need approval is refused, because approval cards don't exist yet.

## Recording the interface's traffic

`catbot record` sits between the interface and the reference server, passes every
request through unchanged, and writes what it saw to `~/Projects/catbot-traffic/`
(one JSON Lines file per run, outside this repo). Values under names like `key`,
`token`, `password` or `code` are replaced with `[redacted]`, but other text can
still be private: treat the recordings as private.

```sh
# tab 1: the reference server, as usual
cd ~/Projects/openmausbot-ref && pnpm dev:server

# tab 2: the recorder (listens on 8800, forwards to 8799)
cd ~/Projects/Catbot && cargo run -- record

# tab 3: the interface, pointed at the recorder instead of the server
cd ~/Projects/openmausbot-ref && OMB_PORT=8800 pnpm dev

# later: the distinct routes the interface used
cd ~/Projects/Catbot && cargo run -- routes
```

## Licensing

Code taken or translated from OpenMausBot stays under Apache 2.0: keep its
LICENSE and NOTICE, and mark what changed. Nothing from its `enterprise/`
folder, and none of its names or mascot.
