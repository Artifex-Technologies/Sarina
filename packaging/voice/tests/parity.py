"""Side by side: the Python originals (Labyrinth_OS, profile/airootfs/usr/local/bin) and their Rust ports (this crate) on the
same inputs, compared byte for byte.

Function-level tests run each test body twice, once against the Python module and once against the Rust port (through the
sarina-voice-testhook binary), each in a fresh folder, with the clock fixed to the same moment; every call made, every answer and
every file left behind must be identical. Program-level tests run both programs with the same fakes on PATH (never the real
sarina, model, whisper or speaker) and compare exit status, output and files.

The Python originals are found in LABYRINTH_OS_DIR, else in a Labyrinth_OS checkout next to this repository's.
"""
import datetime
import importlib.machinery
import importlib.util
import json
import os
import shutil
import subprocess
import sys
import tempfile
import unittest
import warnings

# the carried-over test bodies read files with open(...).read(), as the Labyrinth tests did
warnings.simplefilter("ignore", ResourceWarning)

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)


def _find_labyrinth():
    if os.environ.get("LABYRINTH_OS_DIR"):
        return os.environ["LABYRINTH_OS_DIR"]
    d = CRATE
    while d != os.path.dirname(d):
        d = os.path.dirname(d)
        cand = os.path.join(os.path.dirname(d), "Labyrinth_OS")
        if os.path.isdir(os.path.join(cand, "profile", "airootfs", "usr", "local", "bin")):
            return cand
    return ""


LAB = _find_labyrinth()
SCRIPTS = os.path.join(LAB, "profile", "airootfs", "usr", "local", "bin")
PROGRAMS = ("labyrinth-ask-sarina", "labyrinth-sarina-checkup", "labyrinth-sarina-speak-voiced")


def have_python_originals():
    """The Python scripts themselves: since the switch (Labyrinth_OS 4ecd342) the image's names are links to sarina-voice, and
    LABYRINTH_OS_DIR must name a checkout from before it (git worktree add DIR 1c905fe)."""
    def python(p):
        try:
            with open(p, "rb") as f:
                return not os.path.islink(p) and b"python" in f.readline()
        except OSError:
            return False
    return all(python(os.path.join(SCRIPTS, p)) for p in PROGRAMS)


skip_without_originals = unittest.skipUnless(have_python_originals(), "the Python originals (Labyrinth_OS) are not next to this checkout; set LABYRINTH_OS_DIR")

_built = {}


def build():
    """target/debug/sarina-voice and the test hook, built once per test run (offline)."""
    if not _built:
        target = os.environ.get("CARGO_TARGET_DIR") or os.path.join(CRATE, "target")
        # SARINA_VOICE_PROFILE=release: the build that gets installed (bench_voice.py uses it)
        profile = os.environ.get("SARINA_VOICE_PROFILE", "debug")
        flags = ["--release"] if profile == "release" else []
        subprocess.run(["cargo", "build", "--offline", "--quiet", "--features", "testhook"] + flags, cwd=CRATE, check=True)
        _built["voice"] = os.path.join(target, profile, "sarina-voice")
        _built["hook"] = os.path.join(target, profile, "sarina-voice-testhook")
        links = tempfile.mkdtemp(prefix="sarina-voice-links-")
        for p in PROGRAMS:
            os.symlink(_built["voice"], os.path.join(links, p))
        _built["links"] = links
    return _built


def rust_program(name):
    return os.path.join(build()["links"], name)


def python_program(name):
    return os.path.join(SCRIPTS, name)


def program_cmd(impl, name):
    """How each implementation is started: the Python script with this Python (-I, as the Labyrinth tests do), or the link."""
    if impl == "python":
        return [sys.executable, "-I", python_program(name)]
    return [rust_program(name)]


def load_py(name, modname):
    loader = importlib.machinery.SourceFileLoader(modname, python_program(name))
    spec = importlib.util.spec_from_loader(modname, loader)
    m = importlib.util.module_from_spec(spec)
    loader.exec_module(m)
    return m


# ---- the clock, fixed for both

class Frozen:
    """A stand-in for the datetime module inside a Python original: now() and today() give the fixed moment."""

    def __init__(self, at):
        real = datetime

        class D(real.datetime):
            @classmethod
            def now(cls, tz=None):
                return cls(at.year, at.month, at.day, at.hour, at.minute, at.second)

        class T(real.date):
            @classmethod
            def today(cls):
                return cls(at.year, at.month, at.day)

        self.datetime, self.date, self.timedelta = D, T, real.timedelta


# ---- the Rust side, function by function

class RustCrash(Exception):
    pass


class Hook:
    def __init__(self):
        self.p = subprocess.Popen([build()["hook"]], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, encoding="utf-8")
        self.now = None

    def call(self, fn, *args):
        req = {"fn": fn, "args": list(args), "environ": dict(os.environ), "cwd": os.getcwd(), "now": self.now}
        self.p.stdin.write(json.dumps(req) + "\n")
        self.p.stdin.flush()
        line = self.p.stdout.readline()
        if not line:
            raise RustCrash("the test hook ended")
        r = json.loads(line)
        if "crash" in r:
            raise RustCrash(r["crash"])
        return r["ok"]

    def close(self):
        try:
            self.p.stdin.close()
            self.p.wait(timeout=10)
        except Exception:
            self.p.kill()
        self.p.stdout.close()


class _Search:
    def __init__(self, hook, fn):
        self.hook, self.fn = hook, fn

    def search(self, s):
        return True if self.hook.call(self.fn, s) else None


class RustStreamer:
    def __init__(self, hook, said="", chat=False, limit=75):
        self.hook = hook
        self.id = hook.call("streamer_new", said, chat)

    def feed(self, chunk, final=False):
        return self.hook.call("streamer_feed", self.id, chunk, final)

    def finish(self):
        return self.hook.call("streamer_finish", self.id)

    def text(self):
        return self.hook.call("streamer_text", self.id)


def _iso(dt):
    return dt.strftime("%Y-%m-%dT%H:%M:%S") if dt is not None else None


class RustAsk:
    """labyrinth-ask-sarina's functions, answered by the Rust port."""
    DRAFT_WITHDRAWN = "\x1edraft withdrawn\x1e"

    def __init__(self, hook):
        self.hook = hook
        self.LAUGH_RX = _Search(hook, "laugh")

    def Streamer(self, said="", chat=False, limit=75):
        return RustStreamer(self.hook, said, chat, limit)

    def spoken(self, q):
        return self.hook.call("spoken", q)

    def drop_offers(self, t):
        return self.hook.call("drop_offers", t)

    def flatten(self, t):
        return self.hook.call("flatten", t)

    def shorten(self, t, limit=75):
        return self.hook.call("shorten", t)

    def clean_sentence(self, t):
        return self.hook.call("clean_sentence", t)

    def keyword_topics(self, q):
        return self.hook.call("keyword_topics", q)

    def plain(self, t, n):
        return self.hook.call("plain", t, n)

    def recall(self, q, vault, env):
        return self.hook.call("recall", q, vault, env)

    def learned(self, vault, today=None, n=6):
        return self.hook.call("learned", vault, today.isoformat() if today else None)

    def relevant(self, q, observations, env):
        return self.hook.call("relevant", q, observations, env)

    def companion(self, q, env, now=None):
        return self.hook.call("companion", q, env, _iso(now))

    def told_jokes(self, vault):
        return self.hook.call("told_jokes", vault)

    def a_repeat(self, answer, told):
        return self.hook.call("a_repeat", answer, told)

    def is_echo(self, said, prev):
        return self.hook.call("is_echo", said, prev)

    def about_the_computer(self, topics, said=""):
        return self.hook.call("about_the_computer", topics, said)

    def log_talk(self, q, a, env):
        return self.hook.call("log_talk", q, a, env)

    def log_joke(self, q, a, env):
        return self.hook.call("log_joke", q, a, env)

    def save_turn(self, state, q, a):
        return self.hook.call("save_turn", state, q, a)

    def last_turn(self, state):
        r = self.hook.call("last_turn", state)
        return tuple(r) if r is not None else None

    def note_laugh(self, prev, env):
        return self.hook.call("note_laugh", list(prev) if prev else None, env)

    def route(self, q, env):
        return self.hook.call("route", q, env)

    def command(self, question, cwd, write=False, shell=False, voice=False, resume=None, rules=""):
        return self.hook.call("command", question, cwd, write, shell, voice, resume, rules)


class RustCheckup:
    """labyrinth-sarina-checkup's functions, answered by the Rust port."""

    def __init__(self, hook):
        self.hook = hook
        self.REQUESTS = os.environ.get("LABYRINTH_HEIMDALL_REQUESTS", "/var/lib/heimdall-requests")

    def note(self, vault, text):
        return self.hook.call("checkup_note", vault, text)

    def security_keys(self, audit):
        return dict(self.hook.call("security_keys", audit))

    def security_review(self, env, vault, new):
        return self.hook.call("security_review", env, vault, new)

    def held_check(self, held=None, broken=None):
        return self.hook.call("held_check", held, [list(b) for b in broken] if broken is not None else None, self.REQUESTS)


# ---- recording what each side did

def normalize(v, root):
    if isinstance(v, str):
        return v.replace(root, "<D>")
    if isinstance(v, (list, tuple)):
        return [normalize(x, root) for x in v]
    if isinstance(v, dict):
        return [[normalize(k, root), normalize(x, root)] for k, x in v.items()]
    if isinstance(v, (datetime.date, datetime.datetime)):
        return v.isoformat()
    if v is None or isinstance(v, (bool, int, float)):
        return v
    if hasattr(v, "group"):  # a regex match: only whether it matched
        return True
    return repr(type(v))


class Traced:
    """A module (or a streamer) whose every call is written down: name, arguments, answer (or the error)."""

    def __init__(self, target, trace, root, label=""):
        object.__setattr__(self, "_t", target)
        object.__setattr__(self, "_trace", trace)
        object.__setattr__(self, "_root", root)
        object.__setattr__(self, "_label", label)

    def __getattr__(self, name):
        v = getattr(self._t, name)
        if name == "Streamer" or not callable(v) or name.isupper():
            if name == "Streamer":
                def make(*a, **k):
                    self._trace.append(["Streamer", normalize(list(a), self._root), normalize(k, self._root)])
                    return Traced(v(*a, **k), self._trace, self._root, "Streamer.")
                return make
            return v

        def call(*a, **k):
            try:
                r = v(*a, **k)
            except Exception as e:
                self._trace.append([self._label + name, normalize(list(a), self._root), normalize(k, self._root), "raised"])
                raise
            self._trace.append([self._label + name, normalize(list(a), self._root), normalize(k, self._root), normalize(r, self._root)])
            return r
        return call

    def __setattr__(self, name, value):
        setattr(self._t, name, value)


def snapshot(root):
    """Every file under root: path, mode, content (root replaced by <D>)."""
    out = []
    for d, dirs, files in os.walk(root):
        dirs.sort()
        for f in sorted(files):
            p = os.path.join(d, f)
            rel = os.path.relpath(p, root)
            if os.path.islink(p):
                out.append([rel, "link", os.readlink(p).replace(root, "<D>")])
                continue
            if not os.path.isfile(p):
                out.append([rel, "special", ""])  # a socket
                continue
            with open(p, "rb") as fh:
                data = fh.read()
            out.append([rel, oct(os.stat(p).st_mode & 0o777), data.replace(root.encode(), b"<D>").decode("utf-8", "replace")])
    return out


class ParityCase(unittest.TestCase):
    """Run a carried-over test body against both implementations and compare what they did."""
    maxDiff = None
    SCRIPT = None      # the Python original's name
    RUST = None        # its Rust stand-in class

    def make_module(self, impl):
        if impl == "python":
            m = load_py(self.SCRIPT, self.SCRIPT.replace("-", "_"))
            m.datetime = Frozen(self.at)
            return m
        self.hook = Hook()
        self.hook.now = self.at.strftime("%Y-%m-%dT%H:%M:%S")
        return self.RUST(self.hook)

    def setUpImpl(self):
        pass

    def tearDownImpl(self):
        pass

    def both(self, body):
        build()
        self.at = datetime.datetime.now().replace(microsecond=0)
        seen = {}
        real_mkdtemp = tempfile.mkdtemp
        for impl in ("python", "rust"):
            root = os.path.realpath(real_mkdtemp(prefix="parity-%s-" % impl))
            counter = [0]

            def mkdtemp(*a, **k):
                counter[0] += 1
                d = os.path.join(root, "t%d" % counter[0])
                os.makedirs(d)
                return d
            trace = []
            self.impl, self.hook = impl, None
            tempfile.mkdtemp = mkdtemp
            saved_env, saved_cwd = dict(os.environ), os.getcwd()
            try:
                self.m = Traced(self.make_module(impl), trace, root)
                self.setUpImpl()
                body()
            finally:
                tempfile.mkdtemp = real_mkdtemp
                try:
                    self.tearDownImpl()
                finally:
                    os.environ.clear()
                    os.environ.update(saved_env)
                    os.chdir(saved_cwd)
                    if self.hook:
                        self.hook.close()
            seen[impl] = (trace, snapshot(root))
            shutil.rmtree(root, ignore_errors=True)
        self.assertEqual(seen["python"][0], seen["rust"][0], "the calls and answers differ")
        self.assertEqual(seen["python"][1], seen["rust"][1], "the files left behind differ")


# ---- programs, side by side

def write_exe(path, text):
    with open(path, "w") as f:
        f.write(text)
    os.chmod(path, 0o755)


def run_both(name, args, env, setup=None, compare_stderr=False, stdin=None, timeout=60, cwd=None, inside=None):
    """Run the Python original and the Rust port as `name` with `args`, each in a fresh folder made by setup(root) -> env
    additions; returns {impl: (returncode, stdout, stderr, files)} with the folder written as <D>."""
    out = {}
    for impl in ("python", "rust"):
        root = os.path.realpath(tempfile.mkdtemp(prefix="parity-%s-" % impl))
        e = dict(env)
        if setup:
            e.update(setup(root) or {})
        e = {k: v.replace("<D>", root) for k, v in e.items()}
        a = [x.replace("<D>", root) for x in args]
        cmd = program_cmd(impl, name) + a
        if inside:
            cmd = [x.replace("<D>", root) for x in inside] + cmd
        r = subprocess.run(cmd, env=e, capture_output=True, timeout=timeout, input=stdin, cwd=(cwd or root).replace("<D>", root))
        norm = lambda b: b.replace(root.encode(), b"<D>").decode("utf-8", "replace")
        out[impl] = (r.returncode, norm(r.stdout), norm(r.stderr) if compare_stderr else "", snapshot(root))
        shutil.rmtree(root, ignore_errors=True)
    return out
