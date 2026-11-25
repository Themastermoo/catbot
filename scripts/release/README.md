# Catbot's releases and updates

Catbot updates itself from its GitHub repository's releases (stage 28). This was Moo's choice on
2026-11-25. Pushing a tag such as `v0.2.0` starts the release workflow on GitHub's machines, and the release it publishes holds these files:

| File | What it is |
|---|---|
| `catbot-windows-x64.zip` | the Windows app |
| `catbot-android.apk` | the phone app, signed with Catbot's key |
| `catbot-android-rollback.apk` | the release before this one, rebuilt so a phone can go back to it |
| `release.json` | the version, the storage format (`store`), and each file's SHA-256 |
| `SHA256SUMS` | the same checksums, for `sha256sum -c` |

Each Catbot checks the latest release. When it finds a newer one, it shows **Update**. It
downloads only the files named in `release.json`, and throws away any download whose checksum
doesn't match.

## Setting it up (once, when the repository exists)

The repository doesn't exist yet, so these steps are still to do.

1. **Before anything goes public:**
   - **Your email.** Every commit carries its author's email. To see which addresses are in
     the history, run `git log --format='%ae' | sort -u`.
     - For future commits, use GitHub's private address (GitHub › Settings › Emails, "Keep my
       email addresses private"):
       `git config user.email <id>+<user>@users.noreply.github.com`.
     - For the commits already made, you can push them as they are, in which case the address
       is public. Or you can rewrite them before the first push with
       `git filter-repo --mailmap`, but that changes every commit hash, including the ones the
       Codex notes refer to.
   - **Licence.** `Cargo.toml` says Apache-2.0, like OpenMausBot, but the repo has no
     `LICENSE` file yet. Without one, a public repository is "all rights reserved".
   - **Secrets.** Keys and tokens live in Catbot's data folder, never in the repo, and
     `.gitignore` keeps out `.env` files and databases. The signing key stays in its own
     folder outside the repo.
2. **Put the workflow where GitHub looks for it.** Claude's tools aren't allowed to write
   into `.github`, so it's kept here as `scripts/release/release.yml`:
   ```
   mkdir -p .github/workflows && git mv scripts/release/release.yml .github/workflows/release.yml
   git commit -m "Release workflow into .github"
   ```
3. **Make the repository** on GitHub, as public (Moo's choice), and push:
   ```
   gh auth login
   git remote add origin https://github.com/OWNER/NAME.git
   git push -u origin main
   ```
4. **Give the workflow the phone's signing key**, from the key folder on your PC
   (`~/Projects/Catbot-android-key`, or `~/.local/share/catbot/android-key`). These commands
   pipe the key straight into GitHub, so it isn't shown or copied anywhere else:
   ```
   base64 -w0 ~/Projects/Catbot-android-key/catbot-release.keystore | gh secret set CATBOT_ANDROID_KEYSTORE_B64
   gh secret set CATBOT_ANDROID_KEY_PASSWORD < ~/Projects/Catbot-android-key/password
   ```
   It has to be the same key the phone's Catbot was signed with, or the phone refuses the
   update. If the secrets aren't set, the workflow stops rather than signing with a new key.
5. **Tell Claude the repository's `owner/name`.** It goes in `REPO` in
   `crates/catbot/src/server/version.rs`, which is marked PLACEHOLDER. Until then, a Catbot
   can be pointed at it by hand in Settings › This app.

## Making a release

1. Raise `version` in the top `Cargo.toml` (for example `0.2.0` → `0.3.0`) and commit.
2. Tag it with a note. The tag's message becomes the release notes Catbot shows:
   ```
   git tag -a v0.3.0 -m "What's new in 0.3.0"
   git push origin main v0.3.0
   ```
3. GitHub › Actions › Release shows the build. It takes a while, because it makes two phone
   builds. If the tag doesn't match `Cargo.toml`, it stops straight away.

Version numbers:
- `0.2.0` is the first version that can update itself. Builds from before stage 28 say `0.1.0`
  and have to be installed by hand once.
- If a release adds a step to `Store::open`, raise `STORE_VERSION` in
  `crates/catbot/src/store.rs` with it (a test checks this). That number tells an older
  Catbot whether it can still read the chats.

## Going back

Moo's choices (2026-11-25):
- Catbot asks before installing, and installs only once no bot is in the middle of answering.
- Going back keeps today's chats.
  - If the newer version changed how chats are stored, the older one can't read them. Catbot
    then offers the copy it made just before the update. The newer data is set aside next to
    it, not deleted.
- If a new version won't start, Catbot asks before going back. It never goes back on its own.

How it works on each device:
- **Windows** keeps the last few versions side by side, so going back needs no download.
- **The phone** can't install an older version over a newer one, because Android refuses a
  lower version code. So:
  - every release also carries the release before it, rebuilt with a newer code
    (`catbot-android-rollback.apk`). That's what a phone goes back to.
  - To go back further, open GitHub › Actions › Release › Run workflow and enter the tag (for
    example `v0.2.0`). That release's phone app is rebuilt with a newer code, and a phone can
    then install it.
  - A phone that went back is offered the next release, not the one it left. That one's code
    is now lower than the rebuilt app's.
- **The home PC** builds itself from this repo through the Claude helper, as before.
