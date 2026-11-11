#!/usr/bin/env python3
"""Old against new for Sarina's spoken-question programs: the Python originals (Labyrinth_OS, profile/airootfs/usr/local/bin)
and their Rust port, sarina-voice (packaging/voice/README.md, "Why Rust: measured"). Stand-ins only: no model, no voice, no
real sarina, Heimdall or speaker is reached.

    python3 -I packaging/voice/tests/bench_port.py [--runs N] [--labyrinth DIR] [--py-commit C] [--json FILE]

OLD: the three Python programs as they were at --py-commit of the Labyrinth_OS checkout (git show; the default is the last
commit that changed any of them before the port). NEW: packaging/voice/target/release/sarina-voice (build it first: cd
packaging/voice && cargo build --release --locked --offline), started through links with the programs' names, as installed.

How (the same as Heimdall's rust/parity/bench-port.py):
- each case runs OLD and NEW alternately (old then new, then new then old), RUNS times each after one unmeasured run of
  each, one process at a time, at nice 19;
- a small C launcher (bench-launch, built here with cc) starts each program and reports its wall time, its user + system
  CPU and its peak RSS from wait4(2) (from Python directly, the peak would include Python's own ~12 MB);
- the programs run in a user, mount and network namespace of this script's own: only a loopback, on which a stand-in for
  Heimdall's API answers 127.0.0.1:8930 (the check-up asks that fixed address); a stand-in for the voice service on a Unix
  socket; and one small C stand-in (bench-fake) for every program they start (sarina, sarina-tone, the player,
  coredumpctl, systemctl, df, journalctl, nvidia-smi, notify-send, labyrinth-ask-sarina), answering from a file in ~0.3 ms;
- reported per case: median and p10-p90 wall time, median CPU (the program and the stand-ins it starts: the same calls for
  both), median peak RSS (the largest of the program and its stand-ins, ~1.6 MB), and whether both gave the same output.
"""
import argparse
import ctypes
import fcntl
import http.server
import json
import math
import os
import re
import shutil
import socket
import statistics
import struct
import subprocess
import sys
import tempfile
import threading

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
REPO = os.path.dirname(os.path.dirname(CRATE))
RUST_BIN = os.path.join(CRATE, "target", "release", "sarina-voice")
PROGRAMS = ("labyrinth-ask-sarina", "labyrinth-sarina-checkup", "labyrinth-sarina-speak-voiced")
SCRIPTS = "profile/airootfs/usr/local/bin/"
PY_COMMIT = "ea449d9"  # Labyrinth_OS: the last change to any of the three before the port (2026-11-09)
STANDINS = ("sarina", "sarina-tone", "player", "coredumpctl", "systemctl", "df", "journalctl", "nvidia-smi", "notify-send", "labyrinth-ask-sarina")

LAUNCH_C = r"""
#define _GNU_SOURCE
#include <stdio.h>
#include <time.h>
#include <unistd.h>
#include <sys/resource.h>
#include <sys/wait.h>
/* bench-launch RESULT PROGRAM [ARGS...]: run PROGRAM with this process's standard input, output and error, then write to
   RESULT: wall_ns user_us sys_us maxrss_kb wait_status. The child is forked from this small program, so its peak RSS is its
   own (and that of the children it waited for), not the peak of whatever started the launcher. */
int main(int argc, char **argv) {
    struct timespec a, b;
    struct rusage ru;
    int st;
    if (argc < 3) return 125;
    clock_gettime(CLOCK_MONOTONIC, &a);
    pid_t pid = fork();
    if (pid < 0) return 126;
    if (pid == 0) { execv(argv[2], argv + 2); _exit(127); }
    if (wait4(pid, &st, 0, &ru) < 0) return 126;
    clock_gettime(CLOCK_MONOTONIC, &b);
    FILE *f = fopen(argv[1], "w");
    if (!f) return 126;
    fprintf(f, "%lld %lld %lld %ld %d\n", (long long)(b.tv_sec - a.tv_sec) * 1000000000LL + (b.tv_nsec - a.tv_nsec),
            (long long)ru.ru_utime.tv_sec * 1000000LL + ru.ru_utime.tv_usec,
            (long long)ru.ru_stime.tv_sec * 1000000LL + ru.ru_stime.tv_usec, ru.ru_maxrss, st);
    return fclose(f) == 0 ? 0 : 126;
}
"""

FAKE_C = r"""
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
/* bench-fake: a stand-in for an outside program. Its answer is the file $BENCH_FAKES/NAME_ARG1_ARG2... (default folder
   /run/bench-fakes) for the longest run of its first arguments that has one, '/' written as '%'; with none it prints nothing.
   It reads its standard input to the end (a program may write to it) and exits 0. */
int main(int argc, char **argv) {
    const char *dir = getenv("BENCH_FAKES");
    const char *name = strrchr(argv[0], '/');
    char key[4096], buf[65536];
    size_t n;
    if (!dir) dir = "/run/bench-fakes";
    name = name ? name + 1 : argv[0];
    for (int k = argc - 1; k >= 0; k--) {
        int len = snprintf(key, sizeof key, "%s/%s", dir, name);
        for (int i = 1; i <= k && len < (int)sizeof key; i++) len += snprintf(key + len, sizeof key - len, "_%s", argv[i]);
        if (len >= (int)sizeof key) continue;
        for (char *p = key + strlen(dir) + 1; *p; p++) if (*p == '/') *p = '%';
        FILE *f = fopen(key, "r");
        if (!f) continue;
        while ((n = fread(buf, 1, sizeof buf, f)) > 0) fwrite(buf, 1, n, stdout);
        fclose(f);
        break;
    }
    fflush(stdout);
    if (!isatty(0)) while (read(0, buf, sizeof buf) > 0) {}
    return 0;
}
"""


def run(cmd, **kw):
    return subprocess.run(cmd, check=True, **kw)


def build_c(tmp, name, src):
    path = os.path.join(tmp, name)
    with open(path + ".c", "w") as f:
        f.write(src)
    for static in (["-static"], []):
        if subprocess.run(["cc", "-O2"] + static + ["-o", path, path + ".c"], capture_output=True).returncode == 0:
            return path, bool(static)
    sys.exit("bench_port: needs a C compiler (cc) for the launcher")


def find_labyrinth():
    d = REPO
    cand = os.path.join(os.path.dirname(d), "Labyrinth_OS")
    return os.environ.get("LABYRINTH_OS_DIR") or cand


# ---- measuring one run

class Launcher:
    def __init__(self, path, tmp):
        self.path = path
        self.result = os.path.join(tmp, "launch.result")

    def run(self, argv, env, stdin=None, reset=None):
        if reset:
            reset()
        with open(stdin or os.devnull, "rb") as si:
            p = subprocess.run([self.path, self.result] + argv, stdin=si, capture_output=True, env=env)
        if p.returncode != 0:
            raise RuntimeError("bench-launch failed (%d) for %s" % (p.returncode, argv))
        with open(self.result) as f:
            wall, ut, st, rss, status = f.read().split()
        return {"wall": int(wall) / 1e9, "cpu": (int(ut) + int(st)) / 1e6, "rss": int(rss), "rc": os.waitstatus_to_exitcode(int(status)),
                "out": p.stdout, "err": p.stderr}


def stats(runs):
    wall = sorted(r["wall"] for r in runs)
    q = statistics.quantiles(wall, n=10, method="inclusive") if len(wall) > 1 else [wall[0]] * 9
    return {"runs": len(runs), "wall_ms": statistics.median(wall) * 1e3, "p10_ms": q[0] * 1e3, "p90_ms": q[8] * 1e3,
            "cpu_ms": statistics.median(r["cpu"] for r in runs) * 1e3, "rss_kb": statistics.median(r["rss"] for r in runs)}


TIME_RE = re.compile(rb"\d{4}-\d\d-\d\d[T ]\d\d:\d\d(:\d\d)?(\.\d+)?|\b\d\d:\d\d\b")


def normal(b, tmp):
    return re.sub(rb"/(py|rust)([/-])", rb"/<side>\2", TIME_RE.sub(b"<time>", b)).replace(tmp.encode(), b"<tmp>")


def bench(launcher, case, runs, tmp):
    first = {s: launcher.run(**case[s]) for s in ("old", "new")}
    same = all(normal(first["old"][k], tmp) == normal(first["new"][k], tmp) for k in ("out", "err")) and first["old"]["rc"] == first["new"]["rc"]
    res = {"old": [], "new": []}
    for i in range(runs):
        for s in (("old", "new") if i % 2 == 0 else ("new", "old")):
            res[s].append(launcher.run(**case[s]))
    out = {"case": case["name"], "group": case["group"], "same_output": same, "rc": [first["old"]["rc"], first["new"]["rc"]],
           "old": stats(res["old"]), "new": stats(res["new"])}
    if not same:
        lines = {s: normal(first[s]["out"] + b"\n--stderr--\n" + first[s]["err"], tmp).decode(errors="replace").splitlines() for s in ("old", "new")}
        i = next((i for i, (x, y) in enumerate(zip(lines["old"], lines["new"])) if x != y), min(len(lines["old"]), len(lines["new"])))
        out["first_difference"] = {s: lines[s][i] if i < len(lines[s]) else "<end>" for s in ("old", "new")}
    return out


# ---- namespaces (a user, mount and network namespace of our own: nothing outside sees what happens in it)

libc = ctypes.CDLL(None, use_errno=True)
MS_REC, MS_PRIVATE = 16384, 1 << 18


def mount(src, dst, fstype=None, flags=0, data=None):
    enc = lambda s: s.encode() if isinstance(s, str) else s
    if libc.mount(enc(src), enc(dst), enc(fstype), ctypes.c_ulong(flags), enc(data)) != 0:
        e = ctypes.get_errno()
        raise OSError(e, "mount %s: %s" % (dst, os.strerror(e)))


def enter_namespaces():
    uid, gid = os.getuid(), os.getgid()
    os.unshare(os.CLONE_NEWUSER | os.CLONE_NEWNS | os.CLONE_NEWNET)
    with open("/proc/self/setgroups", "w") as f:
        f.write("deny")
    with open("/proc/self/uid_map", "w") as f:
        f.write("%d %d 1\n" % (uid, uid))
    with open("/proc/self/gid_map", "w") as f:
        f.write("%d %d 1\n" % (gid, gid))
    mount(None, "/", None, MS_REC | MS_PRIVATE)
    s = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    flags = struct.unpack_from("16sH", fcntl.ioctl(s, 0x8913, struct.pack("16sH22x", b"lo", 0)))[1]  # SIOCGIFFLAGS
    fcntl.ioctl(s, 0x8914, struct.pack("16sH22x", b"lo", flags | 1))  # SIOCSIFFLAGS: lo up
    s.close()
    mount("tmpfs", "/run", "tmpfs", 0, "mode=755")
    os.makedirs("/run/bench-fakes")


def answers(d):
    for f in os.listdir("/run/bench-fakes"):
        os.unlink(os.path.join("/run/bench-fakes", f))
    for key, text in d.items():
        with open("/run/bench-fakes/" + key.replace(" ", "_").replace("/", "%"), "w") as f:
            f.write(text)


# ---- stand-ins for Heimdall's API and the voice service

HEIMDALL = {}


class Heimdall(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        data = HEIMDALL.get(self.path)
        if data is None:
            self.send_response(404)
            self.send_header("Content-Length", "0")
            self.end_headers()
            return
        body = json.dumps(data).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *a):
        pass


def wav_bytes(rate=22050, seconds=1.5):
    """A spoken-sentence-sized wav: a tone that rises and falls."""
    n = int(rate * seconds)
    data = b"".join(struct.pack("<h", int(9000 * abs(math.sin(i / n * math.pi)) * math.sin(i * 0.07))) for i in range(n))
    fmt = struct.pack("<HHIIHH", 1, 1, rate, rate * 2, 2, 16)
    body = b"WAVE" + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"data" + struct.pack("<I", len(data)) + data
    return b"RIFF" + struct.pack("<I", len(body)) + body


def voiced(sock_path, mode, wav):
    """The voice service: per connection, one sentence's wav (copied in place, the client deletes it after playing) or none,
    then done."""
    s = socket.socket(socket.AF_UNIX)
    s.bind(sock_path)
    s.listen(8)
    n = [0]

    def serve():
        while True:
            try:
                c, _ = s.accept()
            except OSError:
                return
            f = c.makefile("rwb")
            f.readline()
            if mode[0] == "sentence":
                n[0] += 1
                path = os.path.join(os.path.dirname(sock_path), "s%d.wav" % n[0])
                with open(path, "wb") as w:
                    w.write(wav)
                f.write((json.dumps({"wav": path, "pause_before_ms": 0, "pause_after_ms": 0}) + "\n").encode())
            f.write(b'{"done": true}\n')
            f.flush()
            f.close()
            c.close()
    threading.Thread(target=serve, daemon=True).start()


# ---- the cases

def setup_home(root):
    v = os.path.join(root, "vault")
    for sub in ("Entities", "Jokes", "Talks", "Guides", "Daily"):
        os.makedirs(os.path.join(v, sub))
    with open(os.path.join(v, "Guides", "Sound.md"), "w") as f:
        f.write("# Sound\n\nRestart pipewire.\n")
    with open(os.path.join(v, "Persona.md"), "w") as f:
        f.write("Warm and brief.\n")
    with open(os.path.join(v, "Entities", "About Sam.md"), "w") as f:
        f.write("# About Sam\n\n## Observations\n\n- Sam has a cat named Pixel \u2014 seen 3\u00d7, last 2026-10-01.\n")
    for d in ("run/labyrinth-sarina", "sessions", "requests", "state", ".config"):
        os.makedirs(os.path.join(root, d))
    return {"PATH": os.path.join(os.path.dirname(root), "bin") + ":/usr/bin:/bin", "HOME": root, "LANG": "C.UTF-8", "TZ": "UTC",
            "XDG_CONFIG_HOME": os.path.join(root, ".config"), "XDG_RUNTIME_DIR": os.path.join(root, "run"), "XDG_STATE_HOME": os.path.join(root, "state"),
            "SARINA_SESSION_DIR": os.path.join(root, "sessions"), "SARINA_VAULT": v, "SARINA_OWNER_NAME": "Sam",
            "SARINA_EMBED_URL": "http://127.0.0.1:1/api/embed", "LABYRINTH_TALK_LOG": "0", "LABYRINTH_VOICE_RULES": "Rules.",
            "LABYRINTH_HEIMDALL_REQUESTS": os.path.join(root, "requests"), "LABYRINTH_PLAYER": os.path.join(os.path.dirname(root), "bin", "player")}


ASK = {"sarina vault-search": "Talks/2026-10.md  [meaning]\n  - 2026-10-20 21:00 | they said: my cat knocked the mug over | I said: Gravity tests.\n"
                              "Guides/Sound.md  [meaning]\n  - x\n",
       "sarina chat": "It is four in the afternoon. The sun is still up. Pixel is asleep.\n",
       "sarina resume": "It is four in the afternoon. The sun is still up. Pixel is asleep.\n",
       "sarina daily-report": "ok\n", "sarina-tone": "They sound a little tired lately.\n"}
QUIET = {"nvidia-smi": "12\n", "df": "Use% Mounted on\n 40% /\n", "sarina daily-report": "status kept\n"}
CRASHES = ("Mon 2026-11-10 10:00:00 CET 1234 0 0 SIGSEGV present /usr/bin/spotify 1.2M\n"
           "Mon 2026-11-10 10:05:00 CET 1235 0 0 SIGSEGV present /usr/bin/spotify 1.2M\n"
           "Mon 2026-11-10 10:06:00 CET 1236 1000 1000 SIGABRT present /usr/bin/other 1M\n")
AUDIT = {"packages": [{"package": "openssl", "installed": "3.5.1-1", "severity": "High", "advisory": "AVG-9"}],
         "kernels": [{"kernel": "7.3.0-4-cachyos", "running": True, "count": 0}], "hardening": [{"id": "kvm-world-writable", "now": "/dev/kvm is 0666"}]}
QUARANTINE = [{"sha256": "ab" * 32, "original": "/home/u/.local/bin/mytool", "status": "held", "reason": "unsigned"}]


def cases(py, rs, homes, envs, mode):
    def both(name, prog, args, fakes, heimdall=None, voice=None, fresh=False):
        def setup():
            answers(fakes)
            HEIMDALL.clear()
            HEIMDALL.update(heimdall or {})
            mode[0] = voice

        def reset(s):
            def r():
                shutil.rmtree(os.path.join(homes[s], "run", "labyrinth-sarina", "route-cache.json"), ignore_errors=True)
                p = os.path.join(homes[s], "run", "labyrinth-sarina", "route-cache.json")
                if os.path.exists(p):
                    os.unlink(p)
            return r if fresh else None
        return {"name": name, "group": prog, "setup": setup,
                "old": {"argv": [os.path.join(py, prog)] + args, "env": envs["py"], "reset": reset("py")},
                "new": {"argv": [os.path.join(rs, prog)] + args, "env": envs["rust"], "reset": reset("rust")}}

    ask, chk, spk = PROGRAMS
    talk = dict(ASK, **{"sarina route": "chat\n"})
    audio = dict(ASK, **{"sarina route": "audio\n"})
    findings = dict(QUIET, **{"coredumpctl": CRASHES, "systemctl": "bluetooth.service loaded failed failed\n",
                              "df": "Use% Mounted on\n 45% /\n 93% /home\n", "journalctl": "a\nb\n\nc\n",
                              "labyrinth-ask-sarina": "Spotify crashed twice.\nRun it from a terminal.\n",
                              "sarina skill": "Summary: update openssl first.\nDo now: 1. pacman -Syu\n"})
    held = {"systemctl": "mytool.service loaded failed failed\n"}
    return [
        both("spoken talk (tone, recall, what Sarina knows), a new question", ask, ["--voice", "how is my cat doing today, do you think"], talk, fresh=True),
        both("spoken computer question (guides), a new question", ask, ["--voice", "why is the sound so quiet on my speaker"], audio, fresh=True),
        both("the same spoken question again (route cached)", ask, ["--voice", "why is the sound so quiet on my speaker"], audio),
        both("typed question (no --voice)", ask, ["what is in this folder"], talk, fresh=True),
        both("check-up: all fine", chk, [], QUIET, heimdall={"/v1/audit": {"packages": [], "kernels": [], "hardening": []}, "/v1/quarantine": []}),
        both("check-up: crashes, a failed unit, a full disk, a hole; Sarina asked, noted, notified", chk, [], findings,
             heimdall={"/v1/audit": AUDIT, "/v1/quarantine": QUARANTINE}),
        both("check-up --held: a held program asked back", chk, ["--held"], held, heimdall={"/v1/quarantine": QUARANTINE}),
        both("voice client: one sentence (loudness read, played)", spk, ["Hello there."], {}, voice="sentence"),
        both("voice client: the service has nothing to play", spk, ["Hello there."], {}, voice="done"),
    ]


def change(old, new):
    pct = (old - new) / old * 100
    return "%s%.0f %%, %.2f ms %s" % ("\u2212" if pct > 0 else "+", abs(pct), abs(old - new), "faster" if pct > 0 else "slower")


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("--runs", type=int, default=30)
    ap.add_argument("--labyrinth", default=find_labyrinth(), help="the Labyrinth_OS checkout holding the Python originals")
    ap.add_argument("--py-commit", default=PY_COMMIT)
    ap.add_argument("--rust-bin", default=RUST_BIN)
    ap.add_argument("--json", help="write every number here")
    a = ap.parse_args()
    os.nice(19 - os.nice(0))
    tmp = tempfile.mkdtemp(prefix="bench-voice-port-")
    lab = lambda *x: subprocess.run(["git", "-C", a.labyrinth] + list(x), check=True, capture_output=True).stdout
    res = {"py_commit": lab("rev-parse", a.py_commit).decode().strip(),
           "rust_commit": subprocess.run(["git", "-C", REPO, "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip(),
           "runs": a.runs, "cpus": os.cpu_count()}
    try:
        py, rs = os.path.join(tmp, "py"), os.path.join(tmp, "rust")
        os.makedirs(py)
        os.makedirs(rs)
        rust_bin = os.path.abspath(a.rust_bin)
        for p in PROGRAMS:
            with open(os.path.join(py, p), "wb") as f:
                f.write(lab("show", "%s:%s%s" % (a.py_commit, SCRIPTS, p)))
            os.chmod(os.path.join(py, p), 0o755)
            os.symlink(rust_bin, os.path.join(rs, p))
        launch, static = build_c(tmp, "bench-launch", LAUNCH_C)
        fake, _ = build_c(tmp, "bench-fake", FAKE_C)
        os.makedirs(os.path.join(tmp, "bin"))
        for s in STANDINS:
            os.symlink(fake, os.path.join(tmp, "bin", s))
        res["sizes"] = {"rust_bytes": os.path.getsize(rust_bin), "python_scripts_bytes": {p: os.path.getsize(os.path.join(py, p)) for p in PROGRAMS}}
        L = Launcher(launch, tmp)
        res["floor"] = {"case": "/usr/bin/true (the launcher's floor)", "only": stats([L.run(["/usr/bin/true"], {}) for _ in range(a.runs)]),
                        "static_launcher": static}
        python3 = shutil.which("python3")
        res["python_start"] = {"case": "python3 -c pass (the interpreter alone)",
                               "only": stats([L.run([python3, "-c", "pass"], {"PATH": "/usr/bin:/bin"}) for _ in range(a.runs)])}
        homes = {s: os.path.join(tmp, "home-" + s) for s in ("py", "rust")}
        envs = {s: setup_home(h) for s, h in homes.items()}
        wav = wav_bytes()
        out = os.path.join(tmp, "result.json")
        pid = os.fork()
        if pid == 0:
            code = 1
            try:
                enter_namespaces()
                server = http.server.ThreadingHTTPServer(("127.0.0.1", 8930), Heimdall)
                threading.Thread(target=server.serve_forever, daemon=True).start()
                mode = [None]
                for s, h in homes.items():
                    voiced(os.path.join(h, "run", "labyrinth-sarina", "voiced.sock"), mode, wav)
                done = []
                for c in cases(py, rs, homes, envs, mode):
                    c.pop("setup")()
                    done.append(bench(L, c, a.runs, tmp))
                    print("  " + c["name"], flush=True)
                with open(out, "w") as f:
                    json.dump(done, f)
                code = 0
            except BaseException:
                import traceback
                traceback.print_exc()
            finally:
                os._exit(code)
        _, st = os.waitpid(pid, 0)
        if st != 0:
            sys.exit("bench_port: the namespace part failed")
        with open(out) as f:
            res["cases"] = json.load(f)
    finally:
        shutil.rmtree(tmp, ignore_errors=True)
    print(json.dumps({k: v for k, v in res.items() if k != "cases"}, indent=1))
    print("| case | Python: median (p10-p90) | Rust: median (p10-p90) | change | CPU | peak RSS | same output |")
    for r in res["cases"]:
        o, n = r["old"], r["new"]
        print("| %s | %.2f ms (%.2f-%.2f) | %.2f ms (%.2f-%.2f) | %s | %.2f -> %.2f ms | %.1f -> %.1f MB | %s |" % (
            r["case"], o["wall_ms"], o["p10_ms"], o["p90_ms"], n["wall_ms"], n["p10_ms"], n["p90_ms"], change(o["wall_ms"], n["wall_ms"]),
            o["cpu_ms"], n["cpu_ms"], o["rss_kb"] / 1024, n["rss_kb"] / 1024, "yes" if r["same_output"] else "NO"))
        if not r["same_output"]:
            print("    first difference:", r.get("first_difference"))
    if a.json:
        with open(a.json, "w") as f:
            json.dump(res, f, indent=1)


if __name__ == "__main__":
    main()
