"""Side by side: Labyrinth's Python voice service (labyrinth-voiced and chatterbox_engine.py) and sarina-voiced.

Run it with the Python of the voice's CPU venv (it needs numpy, onnxruntime, tokenizers, soundfile and scipy), from the
repository root, after `cargo build --release --features parity` in packaging/voiced:

    CUDA_VISIBLE_DEVICES= nice -n 19 ~/.local/share/labyrinth/tts/cb-venv/bin/python -I packaging/voiced/tests/parity.py [numpy|text|engine|request|all]

Everything runs on the processor: both sides load the CPU build of ONNX Runtime from that venv
(onnxruntime/capi/libonnxruntime.so.*), the Python through its own module, the Rust through the shared library of the same
release, and CUDA_VISIBLE_DEVICES is empty, so the graphics card is never touched. Nothing is played: the wav files go to a
scratch folder. The Python comes from the Labyrinth_OS checkout next to this one (or LABYRINTH_OS_DIR), at the commit given
by LABYRINTH_OS_REF (default HEAD), read with `git show`, so the files compared are the committed ones.
"""
import importlib.machinery
import importlib.util
import json
import os
import random
import subprocess
import sys
import tempfile
import time

HERE = os.path.dirname(os.path.abspath(__file__))
CRATE = os.path.dirname(HERE)
REPO = os.path.dirname(os.path.dirname(CRATE))
LAB = os.environ.get("LABYRINTH_OS_DIR") or os.path.join(os.path.dirname(REPO), "Labyrinth_OS")
REF = os.environ.get("LABYRINTH_OS_REF", "HEAD")
BIN = os.path.join(CRATE, "target", "release", "sarina-voiced-parity")
VENV = os.path.dirname(os.path.dirname(sys.executable))
DIR = os.path.expanduser(os.environ.get("DIR", "~/.local/share/labyrinth/tts/chatterbox"))
SCRATCH = tempfile.mkdtemp(prefix="voiced-parity-")
FAILS = []


def check(name, ok, detail=""):
    print(("ok    " if ok else "FAIL  ") + name + (("  " + detail) if detail else ""), flush=True)
    if not ok:
        FAILS.append(name)


def python_sources():
    """The two Python files at REF, written to the scratch folder; returns (voiced module, engine module)."""
    src = os.path.join(SCRATCH, "py")
    os.makedirs(src, exist_ok=True)
    for repo_path, name in [("profile/airootfs/usr/local/bin/labyrinth-voiced", "labyrinth_voiced.py"),
                            ("profile/airootfs/usr/local/lib/labyrinth/chatterbox_engine.py", "chatterbox_engine.py")]:
        data = subprocess.run(["git", "-C", LAB, "show", "%s:%s" % (REF, repo_path)], check=True, capture_output=True).stdout
        with open(os.path.join(src, name), "wb") as f:
            f.write(data)
    os.environ["LABYRINTH_LIB"] = src
    sys.path.insert(0, src)
    os.environ.setdefault("LABYRINTH_VOICE_DTYPE", "fp32")
    loader = importlib.machinery.SourceFileLoader("labyrinth_voiced", os.path.join(src, "labyrinth_voiced.py"))
    spec = importlib.util.spec_from_loader("labyrinth_voiced", loader)
    v = importlib.util.module_from_spec(spec)
    loader.exec_module(v)
    import chatterbox_engine as ce
    # never load CUDA libraries in a check: the CPU venv has none of its own, and the card is not to be touched
    if hasattr(ce.ort, "preload_dlls"):
        ce.ort.preload_dlls = lambda *a, **k: None
    return v, ce


def rust(args, stdin=None, env=None):
    e = dict(os.environ, VENV=VENV, DIR=DIR)
    e.update(env or {})
    return subprocess.run([BIN] + args, input=stdin, capture_output=True, text=True, env=e, check=True).stdout


# ---------------------------------------------------------------------------------------------------------------------------
def numpy_checks():
    import numpy as np
    r = np.random.default_rng(7)
    d = os.path.join(SCRATCH, "np")
    os.makedirs(d, exist_ok=True)
    x = np.concatenate([(-r.random(3_000_000) * 110).astype(np.float32), (r.standard_normal(1_000_000) * 30).astype(np.float32),
                        np.array([0, -0.0, 88.72283935546875, -103.97208404541015625, -87.3, np.inf, -np.inf, np.nan], np.float32)])
    x.tofile(os.path.join(d, "x.f32"))
    with np.errstate(over="ignore"):
        np.exp(x).astype(np.float32).tofile(os.path.join(d, "y.f32"))
    out = rust(["numpy", "exp", os.path.join(d, "x.f32"), os.path.join(d, "y.f32")])
    check("numpy exp float32, 4M values", out.strip().endswith(" 0 of 4000008 differ"), out.strip())
    parts = []
    for n in list(range(1, 300)) + [1000, 6561, 6563, 8191, 20000]:
        for k in range(3):
            a = (r.random(n) ** 8).astype(np.float32) if k < 2 else (r.standard_normal(n) * 1e4).astype(np.float32)
            parts += [np.array([n], np.uint32).view(np.float32), a, np.array([a.sum()], np.float32)]
    np.concatenate(parts).tofile(os.path.join(d, "sum.f32"))
    out = rust(["numpy", "sum", os.path.join(d, "sum.f32")])
    check("numpy float32 sum (pairwise), 912 arrays", out.strip() == "sum: 0 of 912 differ", out.strip())
    lines = []
    for seed in [0, 1, 42, 12345, 2**32 - 1, 3735928559, 2**31, 2**40 + 5]:
        g = np.random.default_rng(seed)
        lines.append("seed %d" % seed)
        for _ in range(300):
            c = int(r.integers(0, 6))
            if c == 0:
                lines.append("random %r" % g.random())
            elif c == 1:
                n = int(r.choice([2, 3, 7, 1000, 6563, 2**32]))
                lines.append("below %d %d" % (n, g.integers(0, n)))
            elif c == 2:
                lines.append("u64 %d" % g.bit_generator.random_raw())
            elif c == 3:
                lines.append("choice3 %s" % g.choice(["Hmm... ", "Um... ", "Well... "]).strip())
            elif c == 4:
                p = (r.random(int(r.integers(1, 50))) ** 4).astype(np.float32)
                p[r.random(len(p)) < 0.3] = 0
                if p.sum() == 0:
                    p[0] = 1
                p /= p.sum()
                lines.append("choicep %s %d" % (",".join(str(v) for v in p.view(np.uint32)), g.choice(len(p), p=p)))
            else:
                # the engine's sampling on logits shaped like the model's (6563 of them)
                _, ce = PY
                logits = (r.standard_normal(6563) * float(r.choice([1, 3, 8]))).astype(np.float32)
                t = float(r.choice([0.8, 0.92, 0.85, 0.72, 0.62, 0.55]))
                lines.append("sample %r %s %d" % (t, ",".join(str(v) for v in logits.view(np.uint32)), ce.sample(logits, g, temperature=t)))
    with open(os.path.join(d, "rng.txt"), "w") as f:
        f.write("\n".join(lines) + "\n")
    out = rust(["numpy", "rng", os.path.join(d, "rng.txt")])
    n = sum(1 for l in lines if not l.startswith("seed"))
    check("numpy default_rng draws, choice() and the engine's sample(), %d draws" % n, out.strip().endswith(" 0 of %d differ" % n), out.strip())


# ---------------------------------------------------------------------------------------------------------------------------
TRICKY = ["İ", "ı", "ſ", "K", "ß", "Σ", "ς", "é", "́", " ", " ", "\x1c", "\x1f", "\t", "\n", "«", "»", "²", "٣",
          "_", "*", "`", "#", "[", "]", "<", ">", ".", ",", "!", "?", ";", ":", "…", "🙂", "‍", "️", "☀", "⭐", "中", "x"]
WORDS = ["Sarina", "sarina's", "SARINA", "Suhreenuh", "Heimdall", "fylgja", "Chymaera", "hello", "the", "file", "is", "gone",
         "[joking]", "[JOKING]", "[unsure]", "[urgent]", "[gentle]", "[serious]", "[Serious]", "[chuckle]", "[laugh]", "<think>",
         "</think>", "**bold**", "`code`", "# head", "a_b", "«joking»", "«x»", "« y »", "Mr.", "e.g.", "1.", "2,5", "word,"]


def gen_text(r):
    n = r.randint(0, 60)
    parts = []
    for _ in range(n):
        k = r.random()
        if k < 0.55:
            parts.append(r.choice(WORDS))
        elif k < 0.8:
            parts.append("".join(r.choice(TRICKY) for _ in range(r.randint(1, 3))))
        else:
            parts.append("".join(r.choice("abcdefghijklmnopqrstuvwxyz,") for _ in range(r.randint(1, 300 if r.random() < 0.05 else 12))))
    seps = [" ", " ", " ", ". ", "! ", "? ", ", ", "  ", "\n", ".\n\n", " ", ""]
    return "".join(p + r.choice(seps) for p in parts)


def text_checks():
    v, _ = PY
    import numpy as np
    r = random.Random(11)
    table = [[k, s] for k, s in v.PRONUNCIATIONS.items()] + [["zoë", "Zoey"], ["ai", "A I"], ["k8s", "kubernetes"]]
    reqs, want = [], []
    long_ones = ["x" * 300, "Hi. " + "y" * 250 + ", z.", "a," * 10 + "b" * 260, "w " * 200, "[joking] " + "q" * 241]
    for i in range(1500):
        t = long_ones[i] if i < len(long_ones) else gen_text(r)
        tab = {k: s for k, s in table}
        reqs.append({"fn": "mark", "text": t}); want.append(v.mark_delivery(t))
        reqs.append({"fn": "take", "text": t}); want.append(list(v.take_delivery(t)))
        reqs.append({"fn": "clean", "text": t, "table": table}); want.append(_clean(v, t, tab))
        reqs.append({"fn": "respell", "text": t, "table": table}); want.append(v.respell(t, tab))
        reqs.append({"fn": "split", "text": t}); want.append(_split(v, t))
        reqs.append({"fn": "sentences", "text": t, "table": table})
        parts = _split(v, _clean(v, v.mark_delivery(t), tab))
        want.append(parts if parts == "python-hangs" else [list(v.take_delivery(s)) for s in parts if v.take_delivery(s)[1]])
        name = r.choice(["joking", "unsure", "urgent", "gentle", "serious", ""])
        sent = r.choice(["Very efficient.", "I think it is gone.", "Stop it.", "Is it?", "Éclair time!", "ok", "İstanbul."])
        seed = r.randrange(2**32)
        g = np.random.default_rng(seed)
        a = v.dress(sent, name, g)
        b = v.dress(sent, name, g)
        reqs.append({"fn": "dress", "text": sent, "name": name, "seed": seed}); want.append([a, b, str(np.float64(g.random()).view(np.uint64))])
    # tokenizer ids
    from tokenizers import Tokenizer
    tok = Tokenizer.from_file(os.path.join(DIR, "tokenizer.json"))
    for i in range(400):
        t = _clean(v, v.mark_delivery(gen_text(r)), dict(v.PRONUNCIATIONS)) + r.choice(["", " [chuckle]", "Hmm... "])
        reqs.append({"fn": "tok", "text": t}); want.append(tok.encode(t, add_special_tokens=False).ids)
    # pronunciation files
    files = ["# comment = no\ncorvid = Korvid\n", "Sarina = Sah-REE-nah\r\nnew=  thing \n=x\n y = \n", "a=b=c\n\n   # x=y\nK8S=K eights\n",
             b"bad \xff = byte\n"]
    for i, content in enumerate(files):
        p = os.path.join(SCRATCH, "pron%d" % i)
        with open(p, "wb") as f:
            f.write(content if isinstance(content, bytes) else content.encode())
        reqs.append({"fn": "pron", "path": p})
        home = os.environ.get("HOME")
        os.environ["HOME"] = SCRATCH
        os.makedirs(os.path.join(SCRATCH, ".config", "labyrinth"), exist_ok=True)
        os.replace(p, os.path.join(SCRATCH, ".config", "labyrinth", "pronunciations"))
        try:
            want.append([list(kv) for kv in v.pronunciations().items()])
        except ValueError:
            want.append("error")
        os.replace(os.path.join(SCRATCH, ".config", "labyrinth", "pronunciations"), p)
        os.environ["HOME"] = home
    out = rust(["text"], stdin="\n".join(json.dumps(q) for q in reqs) + "\n").splitlines()
    by_fn = {}
    hangs = 0
    for q, w, o in zip(reqs, want, out):
        got = json.loads(o)
        ok = got == w
        f = by_fn.setdefault(q["fn"], [0, 0, None])
        f[0] += 1
        if w == "python-hangs":
            hangs += 1  # the Python never returns here (a part over 240 characters with no comma or space); the Rust cuts it
            continue
        if not ok:
            f[1] += 1
            f[2] = f[2] or (q, w, got)
    for fn, (n, bad, ex) in by_fn.items():
        check("text %s: %d cases" % (fn, n), bad == 0, "" if not bad else "%d differ, e.g. %r" % (bad, ex))
    print("      (%d cases where the Python never returns: a part of over 240 characters with no comma after the 40th and no space)" % hangs)


def _clean(v, t, table):
    """clean() with a given table (the module's reads ~/.config/labyrinth/pronunciations)."""
    real = v.pronunciations
    v.pronunciations = lambda: dict(table)
    try:
        return v.clean(t)
    finally:
        v.pronunciations = real


def _split(v, t):
    """split_sentences, with a guard: the Python never returns for a long part without a comma or space (the Rust cuts it)."""
    import signal

    def boom(*_):
        raise TimeoutError

    signal.signal(signal.SIGALRM, boom)
    signal.alarm(2)
    try:
        return v.split_sentences(t)
    except TimeoutError:
        return "python-hangs"
    finally:
        signal.alarm(0)


# ---------------------------------------------------------------------------------------------------------------------------
def wav_checks():
    """Reading the reference recording: formats, channels and rates other than 24 kHz (resampled)."""
    import numpy as np
    import soundfile as sf
    _, ce = PY
    d = os.path.join(SCRATCH, "wav")
    os.makedirs(d, exist_ok=True)
    r = np.random.default_rng(5)
    t = np.arange(48000 * 2) / 48000.0
    sig = (0.4 * np.sin(2 * np.pi * 220 * t) + 0.1 * r.standard_normal(t.size)).astype(np.float32)
    for name, sr, ch, sub in [("mono16", 24000, 1, "PCM_16"), ("st16-44k", 44100, 2, "PCM_16"), ("pcm24-48k", 48000, 1, "PCM_24"),
                              ("float-16k", 16000, 1, "FLOAT"), ("pcm32-22k", 22050, 2, "PCM_32"), ("u8-8k", 8000, 1, "PCM_U8"),
                              ("double-24k", 24000, 3, "DOUBLE")]:
        n = int(sr * 1.5)
        data = np.stack([sig[:n] * (0.9 - 0.2 * c) for c in range(ch)], axis=1) if ch > 1 else sig[:n]
        p = os.path.join(d, name + ".wav")
        sf.write(p, data, sr, subtype=sub)
        want = ce.load_audio(p)[0]
        rust(["load", p, p + ".f32"])
        got = np.fromfile(p + ".f32", np.float32)
        exact = want.shape == got.shape and np.array_equal(want.view(np.uint32), got.view(np.uint32))
        diff = float(np.max(np.abs(want - got))) if want.shape == got.shape else float("nan")
        resampled = sr != 24000
        check("reference wav %s: %d samples" % (name, want.size), exact or (resampled and diff < 1e-6),
              "bit-exact" if exact else "max difference %.3g (SciPy's resample_poly in double precision, then float32)" % diff)


def pcm16_checks():
    """Each sentence's file: soundfile's 16-bit PCM against the Rust writer, on samples of every size."""
    import numpy as np
    import soundfile as sf
    r = np.random.default_rng(13)
    levels = np.arange(-31130, 31130, dtype=np.float64) / 32768
    near = np.concatenate([np.nextafter(levels.astype(np.float32), np.float32(-1)), levels.astype(np.float32),
                           np.nextafter(levels.astype(np.float32), np.float32(1))])
    x = np.concatenate([((r.random(300000) * 2 - 1) * 0.95).astype(np.float32), (r.standard_normal(300000) * 1e-3).astype(np.float32), near])
    d = os.path.join(SCRATCH, "pcm16")
    os.makedirs(d, exist_ok=True)
    x.tofile(os.path.join(d, "x.f32"))
    sf.write(os.path.join(d, "py.wav"), x, 24000, subtype="PCM_16")
    rust(["pcm16", os.path.join(d, "x.f32"), os.path.join(d, "rs.wav")])
    with open(os.path.join(d, "py.wav"), "rb") as f1, open(os.path.join(d, "rs.wav"), "rb") as f2:
        a, b = f1.read(), f2.read()
    check("sentence files (16-bit PCM): %d samples" % x.size, a == b, "" if a == b else "%d bytes differ" % sum(p != q for p, q in zip(a, b)))


SENTENCES = [
    ("Hello, I am checking that the voice still works.", 1234, 0.8),
    ("The backup finished at three, and nothing was skipped this time.", 7, 0.8),
    ("Hmm... i think the file is gone, but the snapshot from this morning still has it.", 99, 0.85),
    ("Stop it, the fan is at full speed!", 3141592, 0.72),
    ("That is rough. Take your time, I will be here.", 2**32 - 1, 0.62),
    ("Back it up before you try anything else.", 0, 0.55),
    ("[chuckle] My script ate its own log. Very efficient.", 4242, 0.92),
    ("Suhreenuh is ready when you are, my friend.", 31337, 0.8),
    # what dress() makes of a joke 60 % of the time: on the processor the embedding graph refuses it (see the README)
    ("My script ate its own log. Very efficient. [chuckle]", 4242, 0.92),
]


def engine_checks():
    import numpy as np
    import soundfile as sf
    _, ce = PY
    d = os.path.join(SCRATCH, "engine")
    os.makedirs(os.path.join(d, "py"), exist_ok=True)
    os.makedirs(os.path.join(d, "rs"), exist_ok=True)
    cases = os.path.join(d, "cases.jsonl")
    with open(cases, "w") as f:
        for t, s, temp in SENTENCES:
            f.write(json.dumps({"text": t, "seed": s, "temperature": temp}) + "\n")
    # Python: the tokens are taken from its sample() as it returns them
    drawn = []
    real_sample = ce.sample
    ce.sample = lambda *a, **k: (drawn.append(real_sample(*a, **k)) or drawn[-1])
    t0 = time.perf_counter()
    eng = ce.Chatterbox(DIR, "fp32", gpu=False)
    eng.set_voice(os.path.join(DIR, "voice", "sarina.wav"))
    py_ready = (time.perf_counter() - t0) * 1000
    py = []
    for i, (t, s, temp) in enumerate(SENTENCES):
        drawn.clear()
        t1 = time.perf_counter()
        c1 = time.process_time()
        try:
            wav = eng.speak(t, np.random.default_rng(s), temperature=temp)
        except Exception as e:  # noqa: BLE001
            py.append({"error": str(e)})
            continue
        py.append({"ms": (time.perf_counter() - t1) * 1000, "cpu_ms": (time.process_time() - c1) * 1000,
                   "tokens": drawn[:-1], "samples": len(wav)})
        sf.write(os.path.join(d, "py", "%d.wav" % i), wav, 24000, subtype="PCM_16")
        wav.astype(np.float32).tofile(os.path.join(d, "py", "%d.f32" % i))
    ce.sample = real_sample
    out = rust(["engine", cases, os.path.join(d, "rs")]).splitlines()
    head = json.loads(out[0])
    rs = [json.loads(l) for l in out[1:]]
    for i, (p, q) in enumerate(zip(py, rs)):
        if "error" in p or "error" in q:
            check("engine sentence %d fails the same way" % i, p.get("error") == q.get("error"), "Python %r, Rust %r" % (p.get("error"), q.get("error")))
            continue
        same_tokens = p["tokens"] == q.get("tokens")
        a = np.fromfile(os.path.join(d, "py", "%d.f32" % i), np.float32)
        b = np.fromfile(os.path.join(d, "rs", "%d.f32" % i), np.float32)
        same_f = a.shape == b.shape and np.array_equal(a.view(np.uint32), b.view(np.uint32))
        diff = float(np.max(np.abs(a - b))) if a.shape == b.shape and a.size else float("nan")
        with open(os.path.join(d, "py", "%d.wav" % i), "rb") as f1, open(os.path.join(d, "rs", "%d.wav" % i), "rb") as f2:
            same_wav = f1.read() == f2.read()
        check("engine sentence %d: %d tokens, %.2f s of speech" % (i, len(p["tokens"]), p["samples"] / 24000),
              same_tokens and same_f and same_wav,
              "tokens %s, float samples %s (max diff %g), wav file %s" % ("same" if same_tokens else "DIFFER",
                                                                         "bit-exact" if same_f else "differ", diff,
                                                                         "byte-identical" if same_wav else "differs"))
        print("      Python %.0f ms (CPU %.0f ms), Rust %.0f ms write + %.0f ms decode (CPU %.0f ms)" %
              (p["ms"], p["cpu_ms"], q["write_ms"], q["decode_ms"], q["cpu_ms"]))
    print("      ready: Python %.0f ms, Rust %.0f ms; Rust peak RSS %.0f MB" % (py_ready, head["ready_ms"], head["rss_kb"] / 1024))


# ---------------------------------------------------------------------------------------------------------------------------
REQUEST = ("[joking] My script ate its own log. Very efficient. [unsure] I think the file is gone. "
           "[gentle] That is rough, my friend. [serious] Back it up before you try anything else, and check Heimdall's log.")


def request_checks():
    """A whole request through Voice.speak (Python) and Service::speak (Rust, in turn and pipelined), with the seed fixed,
    and with decodes made to fail at chosen (sentence, attempt) pairs so the retries are compared too."""
    v, ce = PY
    import numpy as np
    d = os.path.join(SCRATCH, "request")
    os.makedirs(d, exist_ok=True)
    txt = os.path.join(d, "text.txt")
    with open(txt, "w") as f:
        f.write(REQUEST)
    voice = v.Voice()
    for seed, plan in [(5, []), (20261111, []), (5, [(0, 0), (2, 0), (2, 1)]), (77, [(1, 0), (1, 1), (1, 2)])]:
        run = os.path.join(d, "py-%d-%d" % (seed, len(plan)))
        os.makedirs(run, exist_ok=True)
        v.RUN = run
        v.random.getrandbits = lambda bits, s=seed: s
        voice.unload()
        voice.load()  # new sessions: the decoder's noise starts from the seed again, as in a new Rust process
        state = {"i": 0, "a": 0}
        real = voice.engine.speak

        def speak(text, rng, temperature=0.8, real=real, state=state, plan=plan):
            wav = real(text, rng, temperature=temperature)
            if (state["i"], state["a"]) in plan:
                state["a"] += 1
                raise ValueError("the voice produced invalid audio")
            state["i"] += 1
            state["a"] = 0
            return wav

        voice.engine.speak = speak
        msgs = []
        voice.counter = 0
        try:
            voice.speak(REQUEST, lambda m: msgs.append(m) or True)
            msgs.append({"done": True})
        except Exception as e:  # noqa: BLE001
            msgs.append({"error": str(e)[:200]})
        voice.engine.speak = real
        for pipeline in ["0", "1"]:
            rrun = os.path.join(d, "rs-%d-%s-%d" % (seed, pipeline, len(plan)))
            os.makedirs(rrun, exist_ok=True)
            out = rust(["request", txt, str(seed)], env={"RUN": rrun, "PIPELINE": pipeline, "SERVICE_DTYPE": "fp32",
                                                        "FAIL": ",".join("%d:%d" % p for p in plan)})
            got = [json.loads(l.split("\t", 1)[1]) for l in out.splitlines() if not l.startswith("rss_kb")]
            same = len(got) == len(msgs)
            for a, b in zip(msgs, got):
                if "wav" in a:
                    same = same and set(a) == set(b) and all(a[k] == b[k] for k in ("text", "pause_before_ms", "pause_after_ms"))
                    with open(a["wav"], "rb") as f1, open(b["wav"], "rb") as f2:
                        same = same and f1.read() == f2.read()
                else:
                    same = same and a == b
            check("request seed %d, failing decodes %s, %s: %d messages" % (seed, plan or "none", "pipelined" if pipeline == "1" else "in turn", len(msgs)),
                  same, "" if same else "Python %r / Rust %r" % ([m.get("text", m) for m in msgs], [m.get("text", m) for m in got]))


PY = None


def fixed_clock():
    """Re-runs this script with the wall clock fixed (tests/fixed_clock.c), so ONNX Runtime seeds the decoder's noise the same
    in the Python and the Rust process."""
    if os.environ.get("FIXED_REALTIME_NS"):
        return
    so = os.path.join(SCRATCH, "fixed_clock.so")
    subprocess.run(["cc", "-O2", "-shared", "-fPIC", "-o", so, os.path.join(HERE, "fixed_clock.c"), "-ldl"], check=True)
    env = dict(os.environ, LD_PRELOAD=so, FIXED_REALTIME_NS="1762905600123456789")
    os.execve(sys.executable, [sys.executable, "-I"] + sys.argv, env)


if __name__ == "__main__":
    fixed_clock()
    what = sys.argv[1] if len(sys.argv) > 1 else "all"
    if not os.path.exists(BIN):
        sys.exit("build it first: cargo build --release --features parity (in packaging/voiced)")
    PY = python_sources()
    for name, fn in [("numpy", numpy_checks), ("text", text_checks), ("wav", wav_checks), ("pcm16", pcm16_checks), ("engine", engine_checks), ("request", request_checks)]:
        if what in (name, "all"):
            fn()
    print("\n%d failed" % len(FAILS) if FAILS else "\nall the same", "(scratch: %s)" % SCRATCH)
    sys.exit(1 if FAILS else 0)
