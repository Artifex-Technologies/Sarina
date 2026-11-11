"""How long the spoken-question programs take, Python original against Rust port, with stand-ins that answer at once (small
shell scripts, about a millisecond each), so what is left is each program's own cost: starting, reading, deciding, writing.
No model, no voice, no real sarina.

  python3 packaging/voice/tests/bench_voice.py [RUNS]

Prints the median wall time of each case for both, and the difference (the time a spoken question saves)."""
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
os.environ.setdefault("SARINA_VOICE_PROFILE", "release")
from parity import build, program_cmd  # noqa: E402

RUNS = int(sys.argv[1]) if len(sys.argv) > 1 else 30

FAKE_SARINA = r"""#!/bin/sh
case "$1" in
  route) echo "$ROUTE" ;;
  vault-search) printf 'Talks/2026-10.md  [meaning]\n  - 2026-10-20 21:00 | they said: my cat knocked the mug over | I said: Gravity tests.\nGuides/Sound.md  [meaning]\n  - x\n' ;;
  chat|resume) printf 'It is four in the afternoon. The sun is still up. Pixel is asleep.\n' ;;
  daily-report) echo ok ;;
esac
"""
FAKE_TONE = "#!/bin/sh\necho 'They sound a little tired lately.'\n"
PLAYER = "#!/bin/sh\nexit 0\n"


def setup(root):
    b = os.path.join(root, "bin")
    os.makedirs(b)
    for name, body in (("sarina", FAKE_SARINA), ("sarina-tone", FAKE_TONE), ("player", PLAYER)):
        with open(os.path.join(b, name), "w") as f:
            f.write(body)
        os.chmod(os.path.join(b, name), 0o755)
    v = os.path.join(root, "vault")
    for sub in ("Entities", "Jokes", "Talks", "Guides"):
        os.makedirs(os.path.join(v, sub))
    with open(os.path.join(v, "Guides", "Sound.md"), "w") as f:
        f.write("# Sound\n\nRestart pipewire.\n")
    with open(os.path.join(v, "Persona.md"), "w") as f:
        f.write("Warm and brief.\n")
    with open(os.path.join(v, "Entities", "About Sam.md"), "w") as f:
        f.write("# About Sam\n\n## Observations\n\n- Sam has a cat named Pixel — seen 3×, last 2026-10-01.\n")
    return {"PATH": b + ":/usr/bin:/bin", "HOME": root, "XDG_CONFIG_HOME": os.path.join(root, ".config"),
            "XDG_RUNTIME_DIR": os.path.join(root, "run"), "SARINA_SESSION_DIR": os.path.join(root, "sessions"),
            "SARINA_VAULT": v, "SARINA_OWNER_NAME": "Sam", "SARINA_EMBED_URL": "http://127.0.0.1:1/api/embed",
            "LABYRINTH_TALK_LOG": "0", "LABYRINTH_VOICE_RULES": "Rules."}


CASES = [
    ("spoken talk (companion: tone, recall, what she knows)", ["--voice", "how is my cat doing today, do you think"], {"ROUTE": "chat"}),
    ("spoken computer question (guides)", ["--voice", "why is the sound so quiet on my speaker"], {"ROUTE": "audio"}),
    ("spoken computer question, route cached", ["--voice", "why is the sound so quiet on my speaker"], {"ROUTE": "audio"}),
    ("typed question", ["what is in this folder"], {"ROUTE": "chat"}),
]


def median_ms(cmd, env, runs):
    times = []
    for _ in range(runs):
        t = time.perf_counter()
        subprocess.run(cmd, env=env, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        times.append((time.perf_counter() - t) * 1000)
    return statistics.median(times)


def main():
    build()
    root = tempfile.mkdtemp(prefix="bench-voice-")
    env = setup(root)
    print("runs per case: %d (median wall time in ms)" % RUNS)
    print("%-55s %9s %9s %9s" % ("case", "Python", "Rust", "saved"))
    for name, args, extra in CASES:
        e = dict(env, **extra)
        if "cached" not in name:
            shutil.rmtree(os.path.join(root, "run"), ignore_errors=True)
        r = {}
        for impl in ("python", "rust"):
            cmd = program_cmd(impl, "labyrinth-ask-sarina") + args
            subprocess.run(cmd, env=e, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)  # warm (and fill the route cache)
            r[impl] = median_ms(cmd, e, RUNS)
        print("%-55s %9.1f %9.1f %9.1f" % (name, r["python"], r["rust"], r["python"] - r["rust"]))
    # the voice client, one sentence that is not a file of the service's: connects, sends, reads "done"
    import socket
    import threading
    sock = os.path.join(root, "run", "labyrinth-sarina", "voiced.sock")
    os.makedirs(os.path.dirname(sock), exist_ok=True)
    s = socket.socket(socket.AF_UNIX)
    s.bind(sock)
    s.listen(8)

    def serve():
        while True:
            try:
                c, _ = s.accept()
            except OSError:
                return
            f = c.makefile("rwb")
            f.readline()
            f.write(json.dumps({"done": True}).encode() + b"\n")
            f.flush()
            c.close()
    threading.Thread(target=serve, daemon=True).start()
    r = {}
    for impl in ("python", "rust"):
        cmd = program_cmd(impl, "labyrinth-sarina-speak-voiced") + ["Hello there."]
        r[impl] = median_ms(cmd, dict(env, LABYRINTH_PLAYER=os.path.join(root, "bin", "player")), RUNS)
    print("%-55s %9.1f %9.1f %9.1f" % ("voice client, per sentence (speak-voiced)", r["python"], r["rust"], r["python"] - r["rust"]))
    s.close()
    shutil.rmtree(root, ignore_errors=True)


if __name__ == "__main__":
    main()
