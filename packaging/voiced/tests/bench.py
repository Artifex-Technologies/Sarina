"""Why Rust: measured. The Python voice service (from git) against sarina-voiced, on the processor, by the method of
Yggdrasil's docs/PORT_NUMBERS.md: old version from git, the same inputs, alternating runs (old, new, new, old, ...), every
process at nice 19, one at a time, medians with the 10th to 90th percentile.

    CUDA_VISIBLE_DEVICES= nice -n 19 ~/.local/share/labyrinth/tts/cb-venv/bin/python -I packaging/voiced/tests/bench.py [engine|service|idle|all] [--runs N] [--json FILE]

- engine: one process loads the voice and makes 6 sentences (fixed seeds); per sentence wall and CPU time, the time to be
  ready, and the process's peak memory (ru_maxrss from wait4).
- service: each service started on a scratch socket, warmed, then asked to speak a five-sentence answer with a fixed seed;
  a stand-in player "plays" each file for its length (nothing is played: it only waits) and the measure is the time to the
  first sentence, the gaps a listener would hear between sentences, and the time to the end. Rust is run with its pipeline
  (the next sentence written while one is decoded) and without it (in turn, like the Python).
- idle: each service started and left alone (voice not loaded) for 60 s: CPU time, wake-ups and memory.

Both load the CPU build of ONNX Runtime 1.31 from this venv; CUDA_VISIBLE_DEVICES is empty, so the card is never used.
"""
import json
import os
import socket
import statistics
import subprocess
import sys
import tempfile
import time
import wave

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
REPO = os.path.dirname(os.path.dirname(CRATE))
LAB = os.environ.get("LABYRINTH_OS_DIR") or os.path.join(os.path.dirname(REPO), "Labyrinth_OS")
REF = os.environ.get("LABYRINTH_OS_REF", "HEAD")
PY = sys.executable
VENV = os.path.dirname(os.path.dirname(PY))
DIR = os.path.expanduser("~/.local/share/labyrinth/tts/chatterbox")
RUST = os.path.join(CRATE, "target", "release", "sarina-voiced")
PARITY = os.path.join(CRATE, "target", "release", "sarina-voiced-parity")
SCRATCH = tempfile.mkdtemp(prefix="voiced-bench-")

SENTENCES = [
    ("The backup finished at three, and nothing was skipped this time.", 7, 0.8),
    ("Hello, I am checking that the voice still works.", 1234, 0.8),
    ("Stop it, the fan is at full speed!", 3141592, 0.72),
    ("That is rough. Take your time, I will be here.", 2**32 - 1, 0.62),
    ("Back it up before you try anything else.", 0, 0.55),
    ("Suhreenuh is ready when you are, my friend.", 31337, 0.8),
]
ANSWER = ("The backup finished at three, and nothing was skipped this time. The snapshot from this morning is still there "
          "if you need it. Your fan curve is back to normal, so the noise should stop. [gentle] Take a break now, you have "
          "earned it. I will keep an eye on the logs while you are away.")
SEED = 20261111

PY_ENGINE = r'''
import json, os, sys, time
import numpy as np
sys.path.insert(0, sys.argv[1])
import chatterbox_engine as ce
t0 = time.perf_counter()
e = ce.Chatterbox(sys.argv[2], "fp32", gpu=False)
e.set_voice(os.path.join(sys.argv[2], "voice", "sarina.wav"))
print(json.dumps({"ready_ms": (time.perf_counter() - t0) * 1000}), flush=True)
for line in open(sys.argv[3]):
    c = json.loads(line)
    t1, c1 = time.perf_counter(), time.process_time()
    w = e.speak(c["text"], np.random.default_rng(c["seed"]), temperature=c["temperature"])
    print(json.dumps({"ms": (time.perf_counter() - t1) * 1000, "cpu_ms": (time.process_time() - c1) * 1000, "samples": len(w)}), flush=True)
'''

PY_SERVICE = r'''
import importlib.machinery, importlib.util, os, sys
sys.path.insert(0, sys.argv[1])
loader = importlib.machinery.SourceFileLoader("labyrinth_voiced", os.path.join(sys.argv[1], "labyrinth_voiced.py"))
spec = importlib.util.spec_from_loader("labyrinth_voiced", loader)
v = importlib.util.module_from_spec(spec)
loader.exec_module(v)
seed = int(os.environ.get("BENCH_SEED", "0"))
v.random.getrandbits = lambda bits: seed
# (onnxruntime is imported late, by the service itself, as in real use; the CPU build's preload_dlls loads nothing)
v.serve()
'''


def old_python():
    src = os.path.join(SCRATCH, "py")
    os.makedirs(src, exist_ok=True)
    for repo_path, name in [("profile/airootfs/usr/local/bin/labyrinth-voiced", "labyrinth_voiced.py"),
                            ("profile/airootfs/usr/local/lib/labyrinth/chatterbox_engine.py", "chatterbox_engine.py")]:
        data = subprocess.run(["git", "-C", LAB, "show", "%s:%s" % (REF, repo_path)], check=True, capture_output=True).stdout
        with open(os.path.join(src, name), "wb") as f:
            f.write(data)
    with open(os.path.join(src, "run_engine.py"), "w") as f:
        f.write(PY_ENGINE)
    with open(os.path.join(src, "run_service.py"), "w") as f:
        f.write(PY_SERVICE)
    commit = subprocess.run(["git", "-C", LAB, "rev-parse", "--short", REF], capture_output=True, text=True).stdout.strip()
    return src, commit


def pct(xs, q):
    xs = sorted(xs)
    if not xs:
        return float("nan")
    k = (len(xs) - 1) * q
    lo = int(k)
    hi = min(lo + 1, len(xs) - 1)
    return xs[lo] + (xs[hi] - xs[lo]) * (k - lo)


def summary(xs):
    return {"median": statistics.median(xs), "p10": pct(xs, 0.1), "p90": pct(xs, 0.9), "n": len(xs)}


def env_base():
    e = dict(os.environ, CUDA_VISIBLE_DEVICES="", VENV=VENV, DIR=DIR, LABYRINTH_VOICE_VENV=VENV, LABYRINTH_VOICE_DTYPE="fp32")
    return e


def run_measured(argv, env):
    """Runs argv at nice 19; returns (stdout lines, wall s, cpu s, peak RSS kB)."""
    t = time.perf_counter()
    p = subprocess.Popen(["nice", "-n", "19"] + argv, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, text=True, env=env)
    out = p.stdout.read()
    _, status, ru = os.wait4(p.pid, 0)
    wall = time.perf_counter() - t
    if status != 0:
        raise RuntimeError("%s failed (%d)" % (argv[0], status))
    return out.splitlines(), wall, ru.ru_utime + ru.ru_stime, ru.ru_maxrss


def bench_engine(runs, src):
    cases = os.path.join(SCRATCH, "cases.jsonl")
    with open(cases, "w") as f:
        for t, s, temp in SENTENCES:
            f.write(json.dumps({"text": t, "seed": s, "temperature": temp}) + "\n")
    res = {"py": {"ready": [], "sentence": [], "sentence_cpu": [], "rtf": [], "rss": [], "cpu": []},
           "rs": {"ready": [], "sentence": [], "sentence_cpu": [], "rtf": [], "rss": [], "cpu": []}}
    order = ["py", "rs", "rs", "py"] * ((runs + 1) // 2)
    for who in ["py", "rs"] + order[: 2 * runs]:  # two warm-up runs, not counted
        warm = sum(len(v["rss"]) for v in res.values()) == 0 and who in ("py", "rs") and not res[who].get("_warm")
        if who == "py":
            argv = [PY, "-I", os.path.join(src, "run_engine.py"), src, DIR, cases]
        else:
            outdir = os.path.join(SCRATCH, "rs-out")
            os.makedirs(outdir, exist_ok=True)
            argv = [PARITY, "engine", cases, outdir]
        lines, wall, cpu, rss = run_measured(argv, env_base())
        if warm:
            res[who]["_warm"] = True
            continue
        recs = [json.loads(l) for l in lines]
        r = res[who]
        r["ready"].append(recs[0]["ready_ms"])
        for c in recs[1:]:
            ms = c["ms"] if who == "py" else c["write_ms"] + c["decode_ms"]
            r["sentence"].append(ms)
            r["sentence_cpu"].append(c["cpu_ms"])
            r["rtf"].append(ms / 1000 / (c["samples"] / 24000))
        r["rss"].append(rss / 1024)
        r["cpu"].append(cpu)
        print("engine %s: ready %.0f ms, sentences %s ms, peak %.0f MB" % (who, recs[0]["ready_ms"],
              [round(c["ms"] if who == "py" else c["write_ms"] + c["decode_ms"]) for c in recs[1:]], rss / 1024), flush=True)
    for v in res.values():
        v.pop("_warm", None)
    return {w: {k: summary(x) for k, x in v.items()} for w, v in res.items()}


def ask(sock, req, timeout=600):
    s = socket.socket(socket.AF_UNIX)
    s.settimeout(timeout)
    s.connect(sock)
    s.sendall((json.dumps(req) + "\n").encode())
    return s


def wait_socket(sock, proc, limit=60):
    t = time.time()
    while time.time() - t < limit:
        if proc.poll() is not None:
            raise RuntimeError("the service ended")
        try:
            s = ask(sock, {"op": "ping"}, 5)
            s.makefile().readline()
            s.close()
            return
        except OSError:
            time.sleep(0.05)
    raise RuntimeError("no socket")


def start_service(who, src, run_dir, pipeline="1"):
    # the socket path must stay under 108 bytes: it goes in a short folder of my own runtime folder (never the real service's)
    sock_dir = tempfile.mkdtemp(prefix="vb-", dir=os.environ.get("XDG_RUNTIME_DIR") or "/tmp")
    sock = os.path.join(sock_dir, "voiced.sock")
    env = env_base()
    env.update(XDG_RUNTIME_DIR=run_dir, LABYRINTH_VOICED_SOCKET=sock, LABYRINTH_VOICE_DIR=DIR, BENCH_SEED=str(SEED), LABYRINTH_VOICE_SEED=str(SEED), LABYRINTH_VOICE_PIPELINE=pipeline,
               HOME=run_dir)  # no pronunciations file of mine is read
    if who == "py":
        argv = [PY, "-I", os.path.join(src, "run_service.py"), src]
    else:
        argv = [RUST]
    os.makedirs(os.path.join(run_dir, "labyrinth-sarina"), mode=0o700, exist_ok=True)
    p = subprocess.Popen(["nice", "-n", "19"] + argv, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, env=env)
    wait_socket(sock, p)
    return p, sock


def stop_service(p, sock):
    p.terminate()
    p.wait()
    for f in [sock]:
        try:
            os.unlink(f)
        except OSError:
            pass
    try:
        os.rmdir(os.path.dirname(sock))
    except OSError:
        pass


def wav_seconds(path):
    with wave.open(path) as w:
        return w.getnframes() / w.getframerate()


def bench_service(runs, src):
    modes = {"py": ("py", "1"), "rs-pipelined": ("rs", "1"), "rs-in-turn": ("rs", "0")}
    res = {m: {"first": [], "gaps": [], "end": [], "loaded_rss": []} for m in modes}
    order = ["py", "rs-pipelined", "rs-in-turn", "rs-in-turn", "rs-pipelined", "py"] * ((runs + 1) // 2)
    for i, m in enumerate(["py", "rs-pipelined", "rs-in-turn"] + order[: 3 * runs]):
        who, pipeline = modes[m]
        run_dir = tempfile.mkdtemp(prefix="run-", dir=SCRATCH)
        p, sock = start_service(who, src, run_dir, pipeline)
        try:
            s = ask(sock, {"op": "warm"})
            s.makefile().readline()
            s.close()
            t_warm = time.time()
            while True:
                s = ask(sock, {"op": "ping"})
                if json.loads(s.makefile().readline()).get("loaded"):
                    s.close()
                    break
                s.close()
                if time.time() - t_warm > 180:
                    raise RuntimeError("%s: the voice did not load" % m)
                time.sleep(0.2)
            with open("/proc/%d/status" % p.pid) as f:
                rss = [int(l.split()[1]) for l in f if l.startswith("VmRSS")][0] / 1024
            t0 = time.perf_counter()
            s = ask(sock, {"op": "speak", "text": ANSWER})
            f = s.makefile()
            play_end = None
            gaps = 0.0
            first = None
            while True:
                line = f.readline()
                if not line:
                    break
                msg = json.loads(line)
                now = time.perf_counter() - t0
                if "wav" in msg:
                    dur = wav_seconds(msg["wav"]) + (msg.get("pause_before_ms") or 0) / 1000 + (msg.get("pause_after_ms") or 0) / 1000
                    os.unlink(msg["wav"])
                    if first is None:
                        first = now
                        play_end = now + dur
                    else:
                        gaps += max(0.0, now - play_end)
                        play_end = max(now, play_end) + dur
                elif "done" in msg or "error" in msg:
                    if "error" in msg:
                        raise RuntimeError(msg["error"])
                    break
            s.close()
        finally:
            stop_service(p, sock)
        if i < 3:
            continue  # warm-up
        res[m]["first"].append(first)
        res[m]["gaps"].append(gaps)
        res[m]["end"].append(play_end)
        res[m]["loaded_rss"].append(rss)
        print("service %s: first sentence after %.2f s, gaps %.2f s, last sentence ends at %.2f s, RSS loaded %.0f MB" % (m, first, gaps, play_end, rss), flush=True)
    return {m: {k: summary(x) for k, x in v.items()} for m, v in res.items()}


def proc_tree_cpu_and_mem(pid):
    with open("/proc/%d/stat" % pid) as f:
        st = f.read().rsplit(")", 1)[1].split()
    tick = os.sysconf("SC_CLK_TCK")
    cpu = (int(st[11]) + int(st[12])) / tick
    with open("/proc/%d/smaps_rollup" % pid) as f:
        m = {l.split(":")[0]: int(l.split()[1]) for l in f if l.split(":")[0] in ("Rss", "Pss")}
    wake = 0
    for tid in os.listdir("/proc/%d/task" % pid):
        with open("/proc/%d/task/%s/status" % (pid, tid)) as f:
            for l in f:
                if l.startswith(("voluntary_ctxt_switches", "nonvoluntary_ctxt_switches")):
                    wake += int(l.split()[1])
    return cpu, m["Rss"] / 1024, m["Pss"] / 1024, wake


def bench_idle(src):
    out = {}
    for who in ["py", "rs"]:
        run_dir = tempfile.mkdtemp(prefix="idle-", dir=SCRATCH)
        p, sock = start_service(who, src, run_dir)
        time.sleep(5)
        c0, _, _, w0 = proc_tree_cpu_and_mem(p.pid)
        time.sleep(60)
        c1, rss, pss, w1 = proc_tree_cpu_and_mem(p.pid)
        stop_service(p, sock)
        out[who] = {"cpu_ms_per_min": (c1 - c0) * 1000, "wakeups_per_min": w1 - w0, "rss_mb": rss, "pss_mb": pss}
        print("idle %s: %s" % (who, out[who]), flush=True)
    return out


if __name__ == "__main__":
    args = sys.argv[1:]
    what = args[0] if args and not args[0].startswith("--") else "all"
    runs = int(args[args.index("--runs") + 1]) if "--runs" in args else 5
    src, commit = old_python()
    result = {"python_from": "Labyrinth_OS %s" % commit, "runs": runs}
    if what in ("idle", "all"):
        result["idle"] = bench_idle(src)
    if what in ("engine", "all"):
        result["engine"] = bench_engine(runs, src)
    if what in ("service", "all"):
        result["service"] = bench_service(runs, src)
    print(json.dumps(result, indent=1))
    if "--json" in args:
        with open(args[args.index("--json") + 1], "w") as f:
            json.dump(result, f, indent=1)
