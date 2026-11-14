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

## Licensing

Code taken or translated from OpenMausBot stays under Apache 2.0: keep its
LICENSE and NOTICE, and mark what changed. Nothing from its `enterprise/`
folder, and none of its names or mascot.
