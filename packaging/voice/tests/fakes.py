"""Stand-ins for everything the programs start: sarina (route, vault-search, system-status, daily-report, crash-detail, chat,
resume, skill), sarina-tone, coredumpctl, systemctl, df, journalctl, nvidia-smi, notify-send, labyrinth-ask-sarina, the audio
player, an add-on. Each writes what it was given (arguments, working folder, environment) as one JSON line to $FAKE_LOG and answers
from $FAKE_SCENARIO (a JSON file: per program or sarina subcommand, a list of answers taken in turn, the last one repeated).

An answer: {"out": text, "rc": 0, "stream": [chunks], "bytes": [byte values], "session": true, "sleep": seconds}.
"""
import json
import os
import sys

FAKE = r'''#!%(python)s -I
import json, os, sys, time
name = os.path.basename(sys.argv[0])
args = sys.argv[1:]
key = name
if name == "sarina" and args:
    key = "sarina " + ("skill" if args[0] == "skill" else args[0])
with open(os.environ["FAKE_LOG"], "a") as f:
    f.write(json.dumps({"prog": name, "args": args, "cwd": os.getcwd(), "env": dict(sorted(os.environ.items()))}) + "\n")
try:
    with open(os.environ["FAKE_SCENARIO"]) as f:
        scen = json.load(f)
except OSError:
    scen = {}
answers = scen.get(key)
if answers is None and name == "sarina" and args and args[0] in ("chat", "resume"):
    answers = scen.get("sarina answer")
if answers is None:
    sys.exit(0)
count_file = os.environ["FAKE_LOG"] + "." + key.replace(" ", "_") + ".n"
try:
    n = int(open(count_file).read())
except (OSError, ValueError):
    n = 0
with open(count_file, "w") as f:
    f.write(str(n + 1))
a = answers[min(n, len(answers) - 1)]
if a.get("session") and args and args[0] == "chat" and "--session-dir" in args:
    d = args[args.index("--session-dir") + 1]
    os.makedirs(d, exist_ok=True)
    open(os.path.join(d, "s%%d%%d.json" %% (len(os.listdir(d)), n)), "w").write("{}")
out = sys.stdout.buffer
if "-stream" in args and "stream" in a:
    for chunk in a["stream"]:
        out.write(chunk.encode("utf-8") if isinstance(chunk, str) else bytes(chunk))
        out.flush()
        if a.get("sleep"):
            time.sleep(a["sleep"])
elif "bytes" in a:
    out.write(bytes(a["bytes"]))
else:
    out.write(a.get("out", "").encode("utf-8"))
out.flush()
sys.exit(a.get("rc", 0))
'''

PROGRAMS = ("sarina", "sarina-tone", "coredumpctl", "systemctl", "df", "journalctl", "nvidia-smi", "notify-send",
            "labyrinth-ask-sarina", "player", "addon")


def install(bin_dir, scenario, programs=PROGRAMS):
    """Write the fakes into bin_dir and the scenario next to them; returns the environment they need."""
    os.makedirs(bin_dir, exist_ok=True)
    for p in programs:
        path = os.path.join(bin_dir, p)
        with open(path, "w") as f:
            f.write(FAKE % {"python": sys.executable})
        os.chmod(path, 0o755)
    scen = os.path.join(bin_dir, "scenario.json")
    with open(scen, "w") as f:
        json.dump(scenario, f)
    return {"FAKE_LOG": os.path.join(bin_dir, "calls.log"), "FAKE_SCENARIO": scen}


def calls(files):
    """The calls the fakes wrote, from a snapshot (parity.snapshot): [(program, args, cwd, env)]."""
    for f in files:
        if f[0].endswith("calls.log"):
            return [json.loads(line) for line in f[2].splitlines()]
    return []
