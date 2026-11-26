#!/usr/bin/env python3
"""Puts a release's files together for GitHub (stage 28.1), in the folder given:

    Catbot-Setup.exe             the Windows installer people download (stage 29.5)
    catbot-windows-x64.zip       the Windows app, as an update unpacks it (scripts/build-windows.sh)
    catbot-amd64.deb             Linux: Ubuntu, Kubuntu, Mint, Debian… (scripts/build-linux.sh)
    Catbot-x86_64.AppImage       Linux: any other distro
    catbot-android.apk           the phone app (scripts/build-android.sh)
    catbot-android-rollback.apk  the release before this one, rebuilt so a phone can go back to it
    release.json                 what Catbot reads: version, storage format, each file's checksum
    SHA256SUMS                   the same checksums, for `sha256sum -c`

    python3 scripts/release/package.py OUT_DIR --android-code N
        [--setup EXE] [--deb DEB] [--appimage APPIMAGE]
        [--rollback APK --rollback-version V --rollback-code N --rollback-store S]

Catbot downloads a file only by the name and checksum in release.json, and refuses it if the
download doesn't match.
"""
import argparse, datetime, hashlib, json, os, re, shutil, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))


def version():
    text = open(os.path.join(ROOT, "Cargo.toml")).read()
    return re.search(r'^version = "([^"]+)"', text, re.M).group(1)


def store_version(root=ROOT):
    text = open(os.path.join(root, "crates/catbot/src/store.rs")).read()
    return int(re.search(r"pub const STORE_VERSION: i64 = (\d+);", text).group(1))


def describe(path, name):
    digest = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(1 << 20), b""):
            digest.update(chunk)
    return {"file": name, "sha256": digest.hexdigest(), "bytes": os.path.getsize(path)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("out")
    ap.add_argument("--windows", default=os.path.join(ROOT, "dist/catbot-windows.zip"))
    ap.add_argument("--android", default=os.path.join(ROOT, "dist/catbot.apk"))
    ap.add_argument("--android-code", type=int, required=True)
    ap.add_argument("--setup")
    ap.add_argument("--deb")
    ap.add_argument("--appimage")
    ap.add_argument("--rollback")
    ap.add_argument("--rollback-version")
    ap.add_argument("--rollback-code", type=int)
    ap.add_argument("--rollback-store", type=int)
    ap.add_argument("--notes", default="")
    args = ap.parse_args()

    os.makedirs(args.out, exist_ok=True)
    release = {
        "version": version(),
        "store": store_version(),
        "built": datetime.datetime.now(datetime.timezone.utc).replace(microsecond=0).isoformat(),
        "commit": subprocess.run(["git", "rev-parse", "HEAD"], cwd=ROOT, capture_output=True, text=True).stdout.strip() or None,
        "notes": args.notes,
    }
    files = [("windows", args.windows, "catbot-windows-x64.zip"), ("android", args.android, "catbot-android.apk")]
    for key, src, name in files:
        if not os.path.isfile(src):
            sys.exit(f"{src} is missing: build it first")
        shutil.copyfile(src, os.path.join(args.out, name))
        release[key] = describe(src, name)
    release["android"]["code"] = args.android_code
    # The downloads for people installing Catbot for the first time (stage 29.4-29.5), under
    # names that stay the same from release to release, so a link to the latest always works.
    for key, src, name in [("windows_setup", args.setup, "Catbot-Setup.exe"), ("linux_deb", args.deb, "catbot-amd64.deb"), ("linux_appimage", args.appimage, "Catbot-x86_64.AppImage")]:
        if src:
            if not os.path.isfile(src):
                sys.exit(f"{src} is missing")
            shutil.copyfile(src, os.path.join(args.out, name))
            release[key] = describe(src, name)
    if args.rollback:
        name = "catbot-android-rollback.apk"
        shutil.copyfile(args.rollback, os.path.join(args.out, name))
        release["android_rollback"] = describe(args.rollback, name) | {
            "version": args.rollback_version, "code": args.rollback_code, "store": args.rollback_store,
        }

    with open(os.path.join(args.out, "release.json"), "w") as f:
        json.dump(release, f, indent=2)
        f.write("\n")
    entries = [release[k] for k in ("windows", "windows_setup", "linux_deb", "linux_appimage", "android", "android_rollback") if k in release]
    with open(os.path.join(args.out, "SHA256SUMS"), "w") as f:
        for entry in entries:
            f.write(f"{entry['sha256']}  {entry['file']}\n")
    print(json.dumps(release, indent=2))


if __name__ == "__main__":
    main()
