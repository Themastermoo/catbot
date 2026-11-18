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

## Catbot's own interface

Being rewritten in Rust with [Dioxus](https://dioxuslabs.com) 0.7 (`crates/catbot-ui`), keeping
OpenMausBot's look: its stylesheet and fonts are reused with credit (see `NOTICE`). It follows the
system's light or dark setting. Built in slices; so far it's the frame of the app.

```sh
scripts/setup.sh          # once: adds the WebAssembly target and dx 0.7.10 (step 6)
scripts/build-ui.sh       # builds the interface (run again after changing it)
cargo run -- serve        # then open http://127.0.0.1:8788
```

`catbot serve` serves the interface for every address outside `/api`, from
`target/dx/catbot-ui/release/web/public` (`--ui` to change). OpenMausBot's interface on port 5199
keeps working at the same time.

## Away from home (paired devices)

`catbot serve` also opens a **remote door** on `127.0.0.1:8789` (`--remote <address>` to move
it, `--remote off` to close it). Only this machine can reach it until Tailscale Funnel gives it a
public https address. Everything there needs a paired device's key, except trading a pairing
code for one:

- At home, make a pairing code (`POST /api/devices/pairing`). It lasts ten minutes and five
  tries, and works once.
- The phone trades it at the remote door (`POST /api/pair` `{ code, name }`) for its own key,
  and sends that as `Authorization: Bearer <key>` from then on. Catbot keeps only a hash of it.
- With its key a device can do what the home screen can with Catbot's bots (chats, messages,
  approvals, settings, live events). It can't reach OpenMausBot's reference server, the
  interface's files, or the device list.
- `GET /api/devices` at home lists paired devices; `DELETE /api/devices/<id>` unpairs one, which
  stops its key at once and ends any live stream it has open.

The address in the pairing QR code is `CATBOT_PUBLIC_URL` if set, else this machine's Tailscale
name (`https://<machine>.<tailnet>.ts.net`).

**Giving Catbot its public address** (Tailscale Funnel, free; Tailscale goes on this machine
only, the phone needs nothing extra):

```sh
curl -fsSL https://tailscale.com/install.sh | sh   # once: install Tailscale
sudo tailscale up                                  # once: sign in (opens a browser)
sudo tailscale set --operator=$USER                # once: let yourself run Funnel
bash scripts/remote-setup.sh                       # carry https://<machine>.ts.net to the door
bash scripts/remote-setup.sh --off                 # take the public address away
```

The first run may print a link to turn on HTTPS and Funnel for your tailnet; approve it and run
the script again. It ends by checking the address from outside, where Catbot's door should
answer 401 (locked). Then pair a phone from the phone button at the top of the sidebar.

## The phone app (Android)

The same interface, as an Android app that talks to Catbot at home through its public address.
It opens on **Pair with Catbot**: scan the QR code from the home screen's Devices dialog (or type
the address and code), and it keeps its key in its own private storage. If home removes the
phone, the app says so and asks to pair again. On a phone, the bot list and a chat take turns
filling the screen (a back arrow in the chat header returns to the list).

```sh
bash scripts/build-android.sh   # → dist/catbot.apk, signed with Catbot's own key
```

It needs the Android SDK and NDK (`ANDROID_HOME`, `ANDROID_NDK_HOME`; installing them means
accepting Google's Android SDK licence), Java 17+, `dx`, and `rustup target add
aarch64-linux-android`. The signing key is made on the first build in
`~/.local/share/catbot/android-key` (`CATBOT_ANDROID_KEY_DIR` to move it); keep it, since updates
must be signed with the same key. To install: copy `dist/catbot.apk` to the phone, open it, and
allow installing apps from that source when Android asks.

## On Windows (the work PC)

The Windows app has two places, switched at the top of the sidebar: **This PC**, its own Catbot
with its own bots, chats and memory vault (`%USERPROFILE%\CatbotMemory`, no Codex link), and
**Home**, Catbot on the Linux computer through its public address. Opening the app starts this
PC's Catbot; closing it stops it. Home is paired once, the first time you pick it: on the Linux
computer open Devices away from home → Pair a phone, and type the address and code it shows.

It's built on Linux and copied over:

```sh
bash scripts/build-windows.sh   # → dist/catbot-windows.zip
```

That needs `rustup target add x86_64-pc-windows-gnu`, MinGW (`sudo apt install
gcc-mingw-w64-x86-64`) and `dx`. On the PC: unzip it, double-click `Install.cmd` (no
administrator needed; it puts Catbot in the Start menu and on the desktop; `install.ps1 -Remove`
takes it away and keeps your data). This PC's own bots need Claude Code installed there and
signed in (`irm https://claude.ai/install.ps1 | iex` in PowerShell, then `claude` once). Data
lives in `%LOCALAPPDATA%\Catbot` (with `server.log`, what this PC's Catbot says).

## The desktop app

Catbot's interface also runs as its own window (WebKitGTK), with Catbot's icon in the KDE menu:

```sh
sudo apt install libwebkit2gtk-4.1-dev libxdo-dev   # once: the desktop libraries
bash scripts/install-app.sh                         # build it and add "Catbot" to the menu
bash scripts/install-app.sh --remove                # take it out of the menu
```

The window talks to Catbot on port 8788, so Catbot has to be running (best as the service below).
Closing the window leaves your bots running. The menu entry opens the app from
`target/dx/catbot-ui/release/linux/app/`; once it's installed, `scripts/build-ui.sh` rebuilds it
along with the web interface, so close and reopen the window to get a new build.

## Running Catbot in the background

So your bots keep running with no terminal or window open, Catbot can run as your own user
service (systemd): it starts when you log in and comes back by itself if it ever stops.

```sh
bash scripts/install-service.sh       # build, install the service and start it
systemctl --user status catbot        # is it running?
journalctl --user -u catbot -f        # what it's saying
systemctl --user restart catbot       # after a new build (the Claude helper does this itself)
bash scripts/install-service.sh --remove  # back to starting it by hand
```

Run the install from your normal terminal: the service takes that terminal's `PATH` (to find
`claude`) and any `CATBOT_HOME`, `CATBOT_CODEX`, `CATBOT_MEMORY`, `CATBOT_TITLE_MODEL`,
`CATBOT_ARCHIVE_DAYS` or `CATBOT_PUBLIC_URL`.
Run it again after moving or reinstalling Claude Code, or to change one of those; running it
again also restarts Catbot on the new build. Stop any Catbot you started by hand
first, since only one can use port 8788.

## Running OpenMausBot's interface on Catbot

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

**Chats.** A bot can have several chats, and they can work at the same time (they share the
bot's folder). A chat is named from the first line of its first message, then a quick call to
Claude's small model (`haiku`) gives it a better name a few seconds later, unless you've
renamed it. `CATBOT_TITLE_MODEL` picks another model, or `off` keeps the first-line names.
Chats untouched for 30 days are archived (each bot keeps its newest chat and the one it opens
to); they wait under Archived in the sidebar, and a new message brings one back.
`CATBOT_ARCHIVE_DAYS` changes the 30, and `0` turns this off.

**Codex.** Bots can read and write the Codex vault at `~/VaultCodex` (`$CATBOT_CODEX` to
move it); they're told to follow its `AGENTS.md` protocol: they look there for context about you and your projects, and add notes when they
learn something worth keeping. On the default approval setting every write asks you first;
a bot set to `accept-edits` (or looser) can write there without asking. `--no-codex` on
`bot new`, or `bot edit --codex false`, keeps a bot out.

**Which AI.** A bot runs on Claude (through Claude Code) unless you move it to a chat API in its
settings: OpenAI, Grok (xAI), Groq, Mistral, Cerebras, DeepSeek, OpenRouter, MiniMax, or a
model on this computer through Ollama or LM Studio. Their keys are typed into Catbot (kept in its
private store, never sent to a phone or shown again, only their last four characters) or set in
the environment (`OPENAI_API_KEY`, `XAI_API_KEY`, `GROQ_API_KEY`, …). Bots on a chat API get
Catbot's own tools: reading, listing and searching files, writing and editing them, and running
commands, in their own folder and the folders they're given (memory vault, Codex). Changes and
commands ask first as a Claude bot's would; a path outside their folders is refused.

Bots can also run on an agent program installed on this computer, speaking the Agent Client
Protocol: Grok CLI, Gemini CLI, GitHub Copilot, Cursor, OpenCode, Qwen Code, Kimi Code, Droid,
Cline, Kiro, Hermes, DeepSeek Harness, OMP or Command Code. Each uses its own sign-in and plan
(install it and sign it in once in a terminal; the bot's settings show how) and its own tools;
its permission requests become Catbot's approval cards, its file requests are served inside the
bot's folders only, and its session carries on from message to message. `CATBOT_AGENT_<ID>`
(e.g. `CATBOT_AGENT_GROK_CLI=/opt/grok/bin/grok`) points Catbot at a program it can't find.

**Memory vault.** Each computer running Catbot keeps a folder of linked Markdown notes the bots
write as they work: `~/CatbotMemory` (`$CATBOT_MEMORY` to move it, `off` to turn it off).
`Bots/` holds a note per bot (Catbot seeds it when the bot is made and moves it when the bot is
renamed), `Projects/` a note per project shared by the bots, and `Journal/` a note per bot per
day. Every bot is told where the vault is and how to keep it (its `AGENTS.md`), and gets it as a
folder it can work in. When the Codex exists, the vault has a `Codex` link to it (a link, not a
copy), so the notes point into it: `[[Codex/20 Projects/Catbot|Catbot]]`. Open the folder in
Obsidian as its own vault, or read it in Catbot: the book button at the top of the sidebar opens a
read-only Memory viewer (on the phone app too), where `[[links]]` open the note they lead to,
Codex notes included. A bot kept out of the Codex isn't told about it or given it, but the
link is still in its vault, so that switch points bots away rather than walling them off.

**A computer for each bot.** Settings › Computers gives a bot its own small Linux desktop with
a browser, in a container on this computer. It needs Podman or Docker (on Kubuntu,
`sudo apt install podman`); `CATBOT_CONTAINER` names another program. The first start builds
the image from `crates/catbot/computer/` (a few minutes). The bot runs commands, clicks, types
and browses there without asking; nothing of yours is put inside, and sending, posting and using
your apps still ask. Watch opens a live view (watching only, unless you switch on "Let me use
it"); it's served at home only. Each computer keeps its files when stopped; Reset throws it
away. One nobody uses or watches for 30 minutes is stopped (`CATBOT_COMPUTER_IDLE_MINUTES`,
`0` never). Keep `catbot serve` on 127.0.0.1 (the default): the boxes can't reach it there.
A bot's computer can be at [Boat](https://boat.dev) instead (a cloud computer, on while this
one is off): connect a Boat API key in the same tab, then pick "At Boat" for the bot. Boat bills
per second while it's on; a stop saves its disk and the next use resumes it. It gets none of the
Boat account's secrets, and the key never leaves Catbot except to Boat.

**Secrets.** Settings › Computers can connect a 1Password service account (make one at
1Password.com › Developer › Service accounts, with read access to a vault made for bots, and
paste its `ops_` token; it needs the 1Password CLI, `op`, on this computer). Bots you tick there
can list the vault's items and fields (never values) and name a secret
(`op://Vault/Item/field`): Catbot reads it and hands it into a command on the bot's computer as an
environment variable, or into a file only the bot can read there. The value isn't kept, isn't
shown in the chat, and isn't given to the AI: Catbot blanks it (and its base64) out of what
comes back. Every use is listed under "Last used". `CATBOT_OP` points at another `op`.

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
