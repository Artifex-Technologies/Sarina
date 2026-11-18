#!/usr/bin/env python3
"""The Go sarina-tone and the Rust one, compared run by run (docs/RUST_PORT.md "sarina-tone").

Each case runs both programs with the same arguments and environment in a throwaway folder made afresh with the same
made-up files each time (a home, a working folder, recordings this script writes: made-up speech, never a real voice;
made-up baselines and mood histories, never the real ones). Compared are stdout, stderr, the exit status (a death by a
signal included) and every file left in the folder (type, mode, contents). A case with steps runs them one after
another in the same folder (a voice heard question after question, until the baseline knows it) and compares after
every step. The one value that cannot match is the time a new history entry is written with (each program reads the
clock itself): a `"t":` within a few seconds of the run is written as NOW in both before they are compared.

    python3 -I rust/parity/tone-differential.py --go GO_SARINA_TONE --rs RUST_SARINA_TONE [--only SUBSTRING] [--show]

Run it with the Go build as both first (`--rs` the Go build too): it must agree with itself. Exit status 0 when the two
agree on every case.
"""

import argparse
import math
import os
import random
import re
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import time

SR = 16000


def speech(seed, seconds, f0, glide, rate, amp, per_phrase, pause, sr=SR):
    """A stand-in for talking (as go/tone/tone_test.go's speech): harmonics at f0 gliding by `glide` semitones, shaped into
    syllables at `rate` per second with a pause after every `per_phrase`, at loudness `amp`, over a faint noise floor."""
    r = random.Random(seed)
    n = int(seconds * sr)
    x = [0.0] * n
    t, syl, count, phase = 0.2, 1 / rate, 0, 0.0
    while t < seconds - 0.2:
        for i in range(int(t * sr), min(int((t + syl * 0.75) * sr), n)):
            u = (i / sr - t) / (syl * 0.75)
            env = math.sin(math.pi * u)
            f = f0 * 2 ** (glide * math.sin(2 * math.pi * 0.7 * i / sr + seed) / 12)
            phase += 2 * math.pi * f / sr
            x[i] += amp * env * sum(math.sin(h * phase) / h for h in range(1, 7)) / 2
        t += syl
        count += 1
        if count % per_phrase == 0:
            t += pause
    return [v + 0.002 * r.gauss(0, 1) for v in x]


def pcm16(x, channels=1):
    out = bytearray()
    for v in x:
        s = max(-32768, min(32767, int(v * 32767)))
        for c in range(channels):
            out += struct.pack("<h", s if c == 0 else s // (c + 1))
    return bytes(out)


def wav(data, channels=1, sr=SR, bits=16, fmt=1, size=None, extra=b""):
    body = struct.pack("<HHIIHH", fmt, channels, sr, sr * channels * bits // 8, channels * bits // 8, bits)
    return (b"RIFF" + struct.pack("<I", 36 + len(data)) + b"WAVE" + extra + b"fmt " + struct.pack("<I", 16) + body + b"data"
            + struct.pack("<I", len(data) if size is None else size) + data)


def recordings():
    """The recordings, made once: name -> bytes."""
    w = {}
    for i in range(14):  # the usual voice, with the natural variation between questions
        w[f"usual{i}"] = wav(pcm16(speech(100 + i, 4, 130 + i % 5 * 3, 1.5 + i % 3 * 0.3, 4.5 + i % 4 * 0.2, 0.25 + i % 3 * 0.02, 6,
                                          0.3 + i % 3 * 0.05)))
    w["usual"] = wav(pcm16(speech(300, 4, 133, 1.6, 4.6, 0.26, 6, 0.32)))
    w["rushed"] = wav(pcm16(speech(301, 4, 135, 0.8, 7.5, 0.33, 40, 0)))
    w["tired"] = wav(pcm16(speech(302, 6, 118, 0.6, 3.0, 0.10, 3, 0.7)))
    w["animated"] = wav(pcm16(speech(303, 4, 160, 4.5, 5.2, 0.45, 6, 0.2)))
    w["stereo"] = wav(pcm16(speech(304, 3, 140, 1, 5, 0.3, 6, 0.3), 2), 2)
    w["8k"] = wav(pcm16(speech(305, 3, 140, 1, 5, 0.3, 6, 0.3, 8000)), sr=8000)
    w["48k"] = wav(pcm16(speech(306, 2, 200, 1, 5, 0.3, 6, 0.3, 48000)), sr=48000)
    w["short"] = wav(pcm16(speech(307, 1, 140, 1, 5, 0.3, 6, 0.3)))
    w["silence"] = wav(pcm16([0.0] * SR * 3))
    w["noise"] = wav(pcm16([0.003 * random.Random(5).gauss(0, 1) for _ in range(SR * 3)]))
    w["loud clipped"] = wav(pcm16(speech(308, 3, 140, 1, 5, 2.5, 6, 0.3)))
    w["still being written"] = wav(pcm16(speech(309, 3, 140, 1, 5, 0.3, 6, 0.3)), size=0xFFFFFFFF)
    w["size zero"] = wav(pcm16(speech(310, 3, 140, 1, 5, 0.3, 6, 0.3)), size=0)
    w["a list chunk first"] = wav(pcm16(speech(311, 3, 140, 1, 5, 0.3, 6, 0.3)), extra=b"LIST\x03\x00\x00\x00abc\x00")
    w["float format"] = wav(pcm16(speech(312, 3, 140, 1, 5, 0.3, 6, 0.3)), fmt=3)
    w["8 bits"] = wav(b"\x80" * 1000, bits=8)
    w["24 bits"] = wav(b"\x00" * 3000, bits=24)
    w["no channels"] = wav(b"\x00" * 3000, channels=0)
    w["no data"] = b"RIFF\x24\x00\x00\x00WAVEfmt \x10\x00\x00\x00" + struct.pack("<HHIIHH", 1, 1, SR, SR * 2, 2, 16) + b"JUNK" + b"\x00" * 16
    w["not a wav"] = b"ID3\x04" + b"\x00" * 100
    w["43 bytes"] = wav(b"")[:43]
    w["empty"] = b""
    return w


def entry(t, mood):
    return '{"t":%d,"mood":"%s"}\n' % (t, mood)


def history(now, n, seed, bias, tz):
    """n made-up entries over the last two weeks and a bit, none within ten seconds of a line Pattern draws (the 7- and
    14-day marks, now, the day's start, the hours of the day), so both runs, a moment apart, see the same."""
    from zoneinfo import ZoneInfo
    import datetime
    zone = ZoneInfo(tz) if tz != "UTC" else datetime.timezone.utc
    r = random.Random(seed)
    moods = ["usual", "low", "tense", "upbeat", "usual", "low", "other"]
    lt = datetime.datetime.fromtimestamp(now, zone)
    midnight = datetime.datetime(lt.year, lt.month, lt.day, tzinfo=zone).timestamp()
    out = []
    while len(out) < n:
        t = now - r.randrange(-3600, 15 * 86400)
        local = datetime.datetime.fromtimestamp(t, zone)
        near = [now, now - 7 * 86400, now - 14 * 86400, midnight]
        if any(abs(t - b) < 10 for b in near) or (local.minute, local.second) < (0, 10) or (local.minute, local.second) >= (59, 50):
            continue
        out.append(entry(t, bias if r.randrange(3) == 0 else r.choice(moods)))
    return "".join(out)


def cases(now):
    wavs = recordings()
    S = ".local/state/labyrinth/"
    C = []

    def case(name, args=(), files=None, env=None, steps=None, **kw):
        C.append(dict(name=name, steps=steps or [args], files=files or {}, env=env or {}, **kw))

    # the command line: Go's flag package on the program's flags
    for a in ([], ["a.wav", "b.wav"], ["-h"], ["-help"], ["--help"], ["--h"], ["-x"], ["--json=maybe", "a.wav"], ["-json=", "a.wav"],
              ["-baseline"], ["--history"], ["--", "-x"], ["-"], ["---x"], ["-=x"], ["--=x"], ["-json=T", "-json=0", "missing.wav"],
              ["-json=TRUE", "-forget=false", "-pattern=f"], [b"-\xff"], [b"--json=\xff", "a.wav"], [b"missing\xff.wav"],
              ["-baseline=", "-history=", "-forget"], ["-json", "-no-update", "missing.wav"], ["missing.wav", "-json"],
              ["--baseline", "b.json", "--history", "h.jsonl", "--", "missing.wav"], ["-h", "-x"], ["-x", "-h"], ["-baseline=x", "-h"]):
        case("flags " + repr(a), a)
    for home in (None, "", "/", "/x//y/", "rel/dir", b"/home/\xff\xfe", "/home/tést"):
        case(f"usage with HOME={home!r}", ["-h"], env={"HOME": home})
    for argv0 in ("/usr/bin/sarina-tone", "", "sarina tone", b"\xff"):
        case(f"usage named {argv0!r}", ["-x"], argv0=argv0)

    # --forget
    base, hist = "home/" + S + "tone-baseline.json", "home/" + S + "mood-history.jsonl"
    case("forget: nothing there", ["--forget"])
    case("forget: both there", ["--forget"], {base: "{}", hist: entry(5, "low")})
    case("forget: other paths", ["--forget", "--baseline", "b.json", "--history=h.jsonl"], {"work/b.json": "{}", "work/h.jsonl": "x", base: "{}"})
    case("forget: an empty folder", ["--forget", "--baseline", "d"], {"work/d/": None})
    case("forget: a folder with something in it", ["--forget", "--baseline", "d"], {"work/d/x": "1", hist: "x"})
    case("forget: under a file", ["--forget", "--history", "f/x"], {"work/f": "1", base: "{}"})
    case("forget: a folder it may not change", ["--forget", "--baseline", "locked/b"], {"work/locked/b": "{}", "work/locked/": 0o500})
    case("forget: no HOME", ["--forget"], {"work/" + S + "tone-baseline.json": "{}"}, env={"HOME": None})
    case("forget: a link", ["--forget", "--baseline", "l"], {"work/l": ("link", "target"), "work/target": "{}"})
    case("forget: standard output closed", ["--forget"], {base: "{}"}, stdout="closed")
    case("forget: standard output closed, SIGPIPE ignored", ["--forget"], {base: "{}"}, stdout="closed", sigpipe="ignore")

    # --pattern
    tzs = ["UTC", "Europe/Berlin", "America/New_York", "Asia/Kolkata", "Australia/Lord_Howe"]
    for i, tz in enumerate(tzs):
        for n, bias in ((0, "low"), (5, "low"), (14, "low"), (40, "tense"), (80, "low"), (200, "upbeat"), (400, "tense"), (700, "low")):
            case(f"pattern {tz} {n} {bias}", ["--pattern"], {hist: history(now, n, i * 1000 + n, bias, tz)}, env={"TZ": tz})
    today = "".join(entry(now - 60 * k - 30, "low") for k in range(1, 4))
    case("pattern: today", ["-pattern"], {hist: today + entry(now - 8 * 86400, "usual")})
    case("pattern: bad lines", ["-pattern"], {hist: "x\n\n{}\nnull\n" + today + '{"T":%d,"MOOD":"low"}\n' % (now - 100) + "\udcff\n"})
    case("pattern: CRLF", ["-pattern"], {hist: today.replace("\n", "\r\n")})
    case("pattern: a long line", ["-pattern"], {hist: today + '{"t":5,"mood":"' + "a" * 70000 + '"}\n' + today})
    case("pattern: other file", ["-pattern", "-history", "h"], {"work/h": today})
    case("pattern: a folder", ["-pattern", "-history", "."])
    case("pattern: no file", ["-pattern"])
    case("pattern: ignores arguments", ["-pattern", "a", "b"], {hist: today})
    case("pattern: TZ that names no file", ["-pattern"], {hist: today}, env={"TZ": "<+03>-3"})
    case("pattern: standard output closed", ["-pattern"], {hist: today}, stdout="closed")

    # recordings
    for name in wavs:
        case(f"recording {name}", ["--json", "--no-update", "r.wav"], {"work/r.wav": wavs[name]})
        case(f"recording {name}, learning", ["r.wav"], {"work/r.wav": wavs[name]})
    case("recording: missing", ["nothere.wav"])
    case("recording: a folder", ["."])
    case("recording: unreadable", ["r.wav"], {"work/r.wav": wavs["usual"], "work/r.wav#mode": 0o000})

    learn = [["--json", f"{n}.wav"] for n in [f"usual{i}" for i in range(14)]]
    tests = ["usual", "rushed", "tired", "animated", "usual3"]
    allw = {f"work/{n}.wav": wavs[n] for n in [f"usual{i}" for i in range(14)] + tests[:4]}
    case("a voice learned, then heard", steps=learn + [["--json", f"{n}.wav"] for n in tests] + [[f"{n}.wav"] for n in tests]
         + [["--pattern"], ["--json", "--no-update", "rushed.wav"]], files=allw)
    long_hist = "".join(entry(1000 + i, "usual") for i in range(700))
    odd_hist = (entry(1, "<b>&") + '{"t":2,"mood":" "}\n' + '{"t":3,"mood":"\udcff"}\n' + "garbage\n" + '{"T":4,"Mood":"low"}\n'
                + '{"t":5,"mood":"' + "a" * 66000 + '"}\n' + entry(6, "lost after the long line"))
    for hname, h in (("700 entries", long_hist), ("odd entries", odd_hist)):
        case(f"a mood written into {hname}", steps=learn[:13] + [["rushed.wav"]], files=dict(allw, **{hist: h}))
    tmp_mode = {hist + ".tmp": "stale", hist + ".tmp#mode": 0o644, base + ".tmp": "stale", base + ".tmp#mode": 0o640}
    case("stale temporary files", steps=learn[:13] + [["rushed.wav"]], files=dict(allw, **tmp_mode))
    for bname, b in (("means but no variances", '{"n":20,"mean":{"loud":-30,"f0":130}}'),
                     ("huge values", '{"n":30,"mean":{"loud":1e308,"f0":-1e308},"var":{"loud":1e308,"f0":1e308},"count":{"loud":30,"f0":30}}'),
                     ("negative variances", '{"n":30,"mean":{"loud":-24,"f0":130,"range":5,"rate":4.5,"pause":0.25},"var":{"loud":-4,"f0":-100,"range":2,"rate":-0.3,"pause":0.01}}'),
                     ("odd keys", '{"n":12,"mean":{"<a&b>":1," ":2,"loud":-28},"var":{"\\u00e9":1},"count":{"z":1}}'),
                     ("old, without counts", '{"n":30,"mean":{"loud":-24,"f0":133,"range":3,"rate":4.6,"pause":0.3},"var":{"loud":4,"f0":100,"range":2,"rate":0.3,"pause":0.01}}'),
                     ("not JSON", "{"), ("n past the top", '{"n":9223372036854775807,"count":{"loud":9223372036854775807}}')):
        for w in ("rushed", "usual"):
            case(f"baseline {bname}, {w}", ["--json", f"{w}.wav"], dict(allw, **{base: b}))
    case("the default folders are made", ["usual0.wav"], allw)
    case("other paths", ["-baseline", "b/c/base.json", "-history", "h/hist.jsonl", "usual0.wav"], allw)
    case("baseline is a folder", steps=learn[:13] + [["-baseline", "d", "-json", "rushed.wav"]], files=dict(allw, **{"work/d/x": "1"}))
    case("history is a folder", steps=learn[:13] + [["-history", "d", "-json", "rushed.wav"]], files=dict(allw, **{"work/d/x": "1"}))
    case("baseline under a file", ["-baseline", "f/b.json", "--json", "usual0.wav"], dict(allw, **{"work/f": "x"}))
    case("baseline two folders under a file", ["-baseline", "f/sub/b.json", "--json", "usual0.wav"], dict(allw, **{"work/f": "x"}))
    case("history two folders down in a folder it may not write", steps=[["-history", "ro/a/b/h.jsonl"] + s for s in learn[:13]]
         + [["-history", "ro/a/b/h.jsonl", "rushed.wav"]], files=dict(allw, **{"work/ro/": 0o500}))
    case("folders it may not write", steps=[["-baseline", "ro/b.json", "-history", "ro/h.jsonl"] + s for s in learn[:13]]
         + [["-baseline", "ro/b.json", "-history", "ro/h.jsonl", "--json", "rushed.wav"]],
         files=dict(allw, **{"work/ro/": None}), chmod_after=(13, "work/ro", 0o500))
    nb = b"x\xff/b.json"
    case("an error naming a path that is not UTF-8", ["-baseline", nb, "--json", "usual0.wav"], {**allw, b"work/x\xff": "file"})
    case("a test run, outside its folders", steps=learn[:13] + [["-json", "rushed.wav"]], files=allw,
         env={"SARINA_TEST": "1", "SARINA_TEST_ROOT": "/nonexistent", "TMPDIR": "/nonexistent"}, test_env_from=13)
    nu = [b"-baseline", b"x\xff/b.json", b"-history", b"x\xff/h.jsonl"]
    case("a test run, paths that are not UTF-8", steps=[nu + s for s in learn[:13]] + [nu + ["-json", "rushed.wav"]], files=allw,
         env={"SARINA_TEST": "1", "SARINA_TEST_ROOT": "/nonexistent", "TMPDIR": "/nonexistent"}, test_env_from=13)
    case("a test run, inside its folders", ["-json", "usual0.wav"], allw, env={"SARINA_TEST": "1", "SARINA_TEST_ROOT": "@ROOT"})
    case("recording: standard output closed", ["--json", "usual0.wav"], allw, stdout="closed")
    case("recording: standard error closed", ["--json", "-baseline", "f/b", "usual0.wav"], dict(allw, **{"work/f": "x"}), stderr="closed")
    return C


def setup(root, files):
    os.makedirs(os.path.join(root, "home"))
    os.makedirs(os.path.join(root, "work"))
    modes = []
    for rel, data in files.items():
        if isinstance(rel, str) and rel.endswith("#mode"):
            modes.append((rel[:-5], data))
            continue
        p = os.path.join(root.encode(), rel.encode() if isinstance(rel, str) else rel)
        if isinstance(rel, str) and rel.endswith("/"):
            os.makedirs(p, exist_ok=True)
            if isinstance(data, int):
                modes.append((rel, data))
            continue
        os.makedirs(os.path.dirname(p), exist_ok=True)
        if isinstance(data, tuple):
            os.symlink(data[1], p)
            continue
        with open(p, "wb") as f:
            f.write(data.encode("utf-8", "surrogateescape") if isinstance(data, str) else data)
    for rel, mode in modes:
        os.chmod(os.path.join(root, rel), mode)


def snapshot(root, t0, t1):
    """Every file under root: path, type, mode, contents (a history time from the run itself written as NOW). A folder or
    file the case made unreadable is opened up for the look and put back as it was."""
    out = {}
    put_back = []

    def readable(p, st, need):
        if stat.S_IMODE(st.st_mode) & need != need:
            os.chmod(p, stat.S_IMODE(st.st_mode) | need)
            put_back.append((p, stat.S_IMODE(st.st_mode)))

    rb = root.encode()
    readable(rb, os.lstat(rb), 0o700)
    for dirpath, dirnames, filenames in os.walk(rb):
        for d in dirnames:
            p = os.path.join(dirpath, d)
            st = os.lstat(p)
            out[os.path.relpath(p, rb)] = ("dir", oct(stat.S_IMODE(st.st_mode)))
            if not stat.S_ISLNK(st.st_mode):
                readable(p, st, 0o700)
        for fn in filenames:
            p = os.path.join(dirpath, fn)
            st = os.lstat(p)
            rel = os.path.relpath(p, rb)
            if stat.S_ISLNK(st.st_mode):
                out[rel] = ("link", os.readlink(p))
                continue
            readable(p, st, 0o400)
            with open(p, "rb") as f:
                data = f.read()
            data = re.sub(rb'"t":(\d+)', lambda m: b'"t":NOW' if t0 - 3 <= int(m.group(1)) <= t1 + 3 else m.group(0), data)
            out[rel] = ("file", oct(stat.S_IMODE(st.st_mode)), data)
    for p, mode in reversed(put_back):
        os.chmod(p, mode)
    return out


def run_case(binary, c):
    root = tempfile.mkdtemp(prefix="tone-diff-")
    try:
        setup(root, c["files"])
        results = []
        for i, args in enumerate(c["steps"]):
            env = {b"PATH": b"/usr/bin:/bin", b"HOME": os.path.join(root, "home").encode(), b"TZ": b"UTC", b"LANG": b"C.UTF-8"}
            if i >= c.get("test_env_from", 0):
                for k, v in c["env"].items():
                    if v is None:
                        env.pop(k.encode(), None)
                    else:
                        v = v.replace("@ROOT", root) if isinstance(v, str) else v
                        env[k.encode()] = v.encode() if isinstance(v, str) else v
            ca = c.get("chmod_after")
            if ca and i == ca[0]:
                os.chmod(os.path.join(root, ca[1]), ca[2])
            argv0 = c.get("argv0", "sarina-tone")
            argv = [argv0] + list(args)
            argv = [a.encode() if isinstance(a, str) else a for a in argv]
            pre = None
            if c.get("sigpipe") == "ignore":
                pre = lambda: signal.signal(signal.SIGPIPE, signal.SIG_IGN)
            else:
                pre = lambda: signal.signal(signal.SIGPIPE, signal.SIG_DFL)
            outs = {}
            for which in ("stdout", "stderr"):
                if c.get(which) == "closed":
                    r, w = os.pipe()
                    os.close(r)
                    outs[which] = w
                else:
                    outs[which] = subprocess.PIPE
            t0 = int(time.time())
            p = subprocess.Popen(argv, executable=binary, cwd=os.path.join(root, "work"), env=env, stdin=subprocess.DEVNULL,
                                 stdout=outs["stdout"], stderr=outs["stderr"], preexec_fn=pre)
            so, se = p.communicate(timeout=120)
            t1 = int(time.time())
            for v in outs.values():
                if v != subprocess.PIPE:
                    os.close(v)
            # the folder's own name differs from run to run: written as @ROOT
            so, se = (x.replace(root.encode(), b"@ROOT") if x is not None else None for x in (so, se))
            results.append({"step": i, "args": args, "rc": p.returncode, "stdout": so, "stderr": se, "files": snapshot(root, t0, t1)})
        return results
    finally:
        for dirpath, dirnames, _ in os.walk(root):
            for d in dirnames:
                os.chmod(os.path.join(dirpath, d), 0o700)
        shutil.rmtree(root)


def show(r):
    return "rc %s\nstdout %r\nstderr %r\nfiles %r" % (r["rc"], r["stdout"], r["stderr"], {k: v[:2] + ((v[2][:300],) if len(v) > 2 else ()) for k, v in r["files"].items()})


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--go", required=True)
    ap.add_argument("--rs", required=True)
    ap.add_argument("--only", default="")
    ap.add_argument("--show", action="store_true")
    a = ap.parse_args()
    # each case runs in a folder of its own: a relative path to a program is read from here
    a.go, a.rs = os.path.abspath(a.go), os.path.abspath(a.rs)
    now = int(time.time())
    all_cases = [c for c in cases(now) if a.only in c["name"]]
    bad = 0
    steps = 0
    for c in all_cases:
        g, r = run_case(a.go, c), run_case(a.rs, c)
        if g != r:  # a run that met the turn of a second twice: once more before calling it a difference
            g, r = run_case(a.go, c), run_case(a.rs, c)
        steps += len(g)
        if a.show:
            print(f"== {c['name']}\n{show(g[-1])}")
        if g != r:
            bad += 1
            for x, y in zip(g, r):
                if x != y:
                    print(f"DIFFERENT: {c['name']} (step {x['step']}: {x['args']!r})\n-- go:\n{show(x)}\n-- rust:\n{show(y)}\n")
                    break
    print(f"tone differential: {len(all_cases) - bad} of {len(all_cases)} cases ({steps} runs) the same")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
