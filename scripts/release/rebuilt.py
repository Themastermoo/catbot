#!/usr/bin/env python3
"""After the release workflow rebuilds an older release's phone app (Run workflow, "rebuild"),
this updates that release's release.json and SHA256SUMS to the new APK and version code.

    python3 scripts/release/rebuilt.py DIR CODE

DIR holds the release's downloaded release.json and the new catbot-android.apk.
"""
import json, os, sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from package import describe  # noqa: E402

folder, code = sys.argv[1], int(sys.argv[2])
path = os.path.join(folder, "release.json")
release = json.load(open(path))
release["android"] = describe(os.path.join(folder, "catbot-android.apk"), "catbot-android.apk") | {"code": code}
with open(path, "w") as f:
    json.dump(release, f, indent=2)
    f.write("\n")
with open(os.path.join(folder, "SHA256SUMS"), "w") as f:
    for key in ("windows", "windows_setup", "linux_deb", "linux_appimage", "android", "android_rollback"):
        if key in release:
            f.write(f"{release[key]['sha256']}  {release[key]['file']}\n")
print(f"{release['version']}: phone app rebuilt with code {code}")
