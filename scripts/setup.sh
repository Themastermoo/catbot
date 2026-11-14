#!/usr/bin/env bash
# Catbot: one-time setup for this machine.
# Safe to run more than once; each step skips itself if it's already done.
#
#   1. System packages (git, curl, a C compiler, SSL headers)  [asks for your password]
#   2. Starts the Catbot git repository with a first commit
#   3. Clones OpenMausBot into ~/Projects/openmausbot-ref (read-only reference)
#   4. Installs Rust (rustup)
#   5. Installs Node 24 (nvm) and pnpm 10, which the original server needs
set -euo pipefail

PROJECTS="$HOME/Projects"
CATBOT="$PROJECTS/Catbot"
REF="$PROJECTS/openmausbot-ref"
NVM_VERSION="v0.41.0"

step() { printf '\n\033[1;36m==> %s\033[0m\n' "$1"; }
ok()   { printf '    \033[32m%s\033[0m\n' "$1"; }

step "1/5  System packages"
sudo apt-get update
sudo apt-get install -y git curl build-essential pkg-config libssl-dev ca-certificates
ok "done"

step "2/5  Catbot git repository"
if [ -z "$(git config --global user.name || true)" ]; then
  read -rp "    Your name for commits: " gname
  git config --global user.name "$gname"
fi
if [ -z "$(git config --global user.email || true)" ]; then
  read -rp "    Your email for commits (your GitHub email or no-reply address): " gemail
  git config --global user.email "$gemail"
fi
cd "$CATBOT"
if [ -d .git ]; then
  ok "already a git repository, leaving it alone"
else
  git init -b main
  git add .
  git commit -m "Start Catbot"
  ok "repository created with a first commit"
fi

step "3/5  Reference clone of OpenMausBot"
if [ -d "$REF/.git" ]; then
  ok "already cloned; pulling the latest"
  git -C "$REF" pull --ff-only
else
  git clone https://github.com/milind-soni/OpenMausBot.git "$REF"
  ok "cloned to $REF"
fi

step "4/5  Rust"
if command -v rustup >/dev/null 2>&1 || [ -x "$HOME/.cargo/bin/rustup" ]; then
  ok "rustup already installed; updating"
  "$HOME/.cargo/bin/rustup" update stable
else
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
fi
# shellcheck disable=SC1091
. "$HOME/.cargo/env"
ok "$(rustc --version)"

step "5/5  Node 24 and pnpm"
export NVM_DIR="$HOME/.nvm"
if [ ! -s "$NVM_DIR/nvm.sh" ]; then
  curl -o- "https://raw.githubusercontent.com/nvm-sh/nvm/$NVM_VERSION/install.sh" | bash
fi
# shellcheck disable=SC1091
. "$NVM_DIR/nvm.sh"
nvm install 24
nvm alias default 24
npm install -g pnpm@10
ok "node $(node --version), pnpm $(pnpm --version)"

printf '\n\033[1;32mAll set.\033[0m Close this terminal and open a new one so rust and node are on your PATH.\n'
