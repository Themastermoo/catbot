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

## Running the interface on Catbot

```sh
# tab 1: the reference server (still answers everything Catbot doesn't yet)
cd ~/Projects/openmausbot-ref && pnpm dev:server

# tab 2: Catbot, in front of it
cd ~/Projects/Catbot && cargo run -- serve

# tab 3: the interface, pointed at Catbot instead of the reference server
cd ~/Projects/openmausbot-ref && OMB_PORT=8788 pnpm dev
```

Then open http://127.0.0.1:5199. The two servers run side by side, split by bot:

- **Catbot bots** (made with `catbot bot new`, or with **+** in the interface) are answered by
  Catbot: chat, streaming replies, approval cards (Allow once / Deny), Stop, rename, approval
  level, model, delete, and history.
- **The reference server's bots** (MausBot and any it makes) and everything else (settings,
  routines, inbox…) pass through to the reference server unchanged.
- The bot list and the live updates are merged, so the interface shows one team.

Things a Catbot bot can't do yet answer with a clear error, and Catbot prints a line saying
which route was asked for: that's the to-do list. Catbot listens on 127.0.0.1 only, with no
sign-in, like the reference server's development mode. Don't expose it to your network yet.

## Bots

```sh
cargo build    # then use target/debug/catbot, or: cargo install --path crates/catbot

catbot bot new "Tree Helper" --instructions "You help with my tree-work jobs: quotes, invoices, scheduling."
catbot bot list
catbot chat "Tree Helper" "Draft a quote for removing two oaks"   # continues the last conversation
catbot chat "Tree Helper" --new "Start over: …"                    # a fresh conversation
catbot history "Tree Helper"
catbot bot edit "Tree Helper" --model opus --mode accept-edits
catbot bot delete "Tree Helper"                                    # asks you to type the name
```

Each bot has its own instructions, model, approval setting and working folder, and
everything said is saved. Data lives in `~/.local/share/catbot` (`$CATBOT_HOME` to move it):
`catbot.db` (bots, conversations, messages) and `bots/<name>/` (each bot's folder).

**Codex.** Bots can read and write the Codex vault at `~/VaultCodex` (`$CATBOT_CODEX` to
move it); they're told to follow its `AGENTS.md` protocol: they look there for context about you and your projects, and add notes when they
learn something worth keeping. On the default approval setting every write asks you first;
a bot set to `accept-edits` (or looser) can write there without asking. `--no-codex` on
`bot new`, or `bot edit --codex false`, keeps a bot out.

## Talking to Claude through Catbot

`catbot ask` runs one unsaved turn through Catbot's own Claude driver (`crates/catbot/src/claude.rs`),
the same piece every bot will run on.

```sh
cargo run -- ask "What's in this folder?"
cargo run -- ask --resume <session id> "And the biggest file?"   # continue a conversation
cargo run -- ask --model opus --system "You are terse." "Explain Rust lifetimes"
```

Bots are isolated from this machine's own Claude setup (CLAUDE.md, MCP servers, plugins)
unless `--inherit-user-config` is given.

**Approvals.** Anything that needs a yes (running a command, editing a file) is asked in the
terminal: `y` allows it, `n` refuses, and anything longer refuses and is passed to the bot as
your reason ("no, use /tmp instead"). Without a terminal, or with `--no-approvals`, such steps
are refused. How it works is described at the top of `crates/catbot/src/approval.rs`.

## Recording the interface's traffic

`catbot record` sits between the interface and the reference server, passes every
request through unchanged, and writes what it saw to `~/Projects/catbot-traffic/`
(`catbot serve --record` does the same for what Catbot passes on)
(one JSON Lines file per run, outside this repo). Values under names like `key`,
`token`, `password` or `code` are replaced with `[redacted]`, but other text can
still be private: treat the recordings as private.

```sh
# tab 1: the reference server, as usual
cd ~/Projects/openmausbot-ref && pnpm dev:server

# tab 2: the recorder (listens on 8788, forwards to 8799; the reference server also uses 8800)
cd ~/Projects/Catbot && cargo run -- record

# tab 3: the interface, pointed at the recorder instead of the server
cd ~/Projects/openmausbot-ref && OMB_PORT=8788 pnpm dev

# later: the distinct routes the interface used
cd ~/Projects/Catbot && cargo run -- routes
```

## Licensing

Code taken or translated from OpenMausBot stays under Apache 2.0: keep its
LICENSE and NOTICE, and mark what changed. Nothing from its `enterprise/`
folder, and none of its names or mascot.
