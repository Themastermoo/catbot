"""A stand-in for sherpa-onnx's two programs, so Catbot's voice can be tested without them.

Named sherpa-onnx-offline, it "hears" a WAV: the words are whatever follows "SAY:" inside it, as
the real one prints its result, one line of JSON on stdout. Named sherpa-onnx-offline-tts, it
writes a "WAV" to --output-filename holding the arguments it was given, so a test can read them.
"""
import json, os, sys

args = sys.argv[1:]
if os.path.basename(sys.argv[0]).startswith("sherpa-onnx-offline-tts"):
    out = next(a.split("=", 1)[1] for a in args if a.startswith("--output-filename="))
    with open(out, "wb") as f:
        f.write(b"RIFF" + json.dumps(args).encode())
    print("Saved", file=sys.stderr)
else:
    for need in ("--whisper-encoder=", "--whisper-decoder=", "--tokens="):
        path = next(a.split("=", 1)[1] for a in args if a.startswith(need))
        assert os.path.isfile(path), path
    wav = open(args[-1], "rb").read()
    words = wav.split(b"SAY:", 1)[1].decode() if b"SAY:" in wav else ""
    print("Done!", file=sys.stderr)
    print(json.dumps({"lang": "", "text": " " + words, "tokens": []}))
