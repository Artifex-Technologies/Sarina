"""labyrinth-ask-sarina as a whole program, the Python original and the Rust port side by side: the same command line, the same
fakes (fakes.py; PATH holds nothing else, so the real sarina, model, coredumpctl or sarina-tone can never be reached), and the
same exit status, output, files, and calls to the fakes (arguments, working folder and the whole environment) must come out.
A fake embedding server answers the meaning lookups with fixed vectors."""
import hashlib
import http.server
import json
import os
import threading
import unittest

import fakes
from parity import run_both, skip_without_originals, write_exe

ASK = "labyrinth-ask-sarina"


class Embeddings(http.server.BaseHTTPRequestHandler):
    """Ollama's /api/embed, with vectors made from each text's hash; some values are whole numbers ("0", "1"), as Go writes them."""

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        vecs = []
        for t in body["input"]:
            h = hashlib.sha256(t.encode()).digest()
            v = []
            for i in range(16):
                x = (h[i] - 128) / 97.0
                v.append("0" if h[i] % 11 == 0 else ("%.7g" % x))
            # a family resemblance between texts that share the word "cat", so a few observations clear the bar
            if "cat" in t:
                v[0], v[1] = "0.9", "0.8"
            vecs.append("[" + ", ".join(v) + "]")
        data = ('{"model": "x", "embeddings": [' + ", ".join(vecs) + "]}").encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, *a):
        pass


SERVER = None


def embed_url():
    global SERVER
    if SERVER is None:
        SERVER = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Embeddings)
        threading.Thread(target=SERVER.serve_forever, daemon=True).start()
    return "http://127.0.0.1:%d/api/embed" % SERVER.server_address[1]


def base_env(**extra):
    env = {"PATH": "<D>/bin", "HOME": "<D>/home", "XDG_CONFIG_HOME": "<D>/home/.config", "XDG_RUNTIME_DIR": "<D>/run",
           "XDG_STATE_HOME": "<D>/home/.local/state", "SARINA_SESSION_DIR": "<D>/sessions", "SARINA_EMBED_URL": "http://127.0.0.1:1/api/embed"}
    env.update(extra)
    return env


def vault_files(d, persona=True, guides=True, about=True):
    v = os.path.join(d, "vault")
    for sub in ("Entities", "Jokes", "Talks", "Guides", "Daily"):
        os.makedirs(os.path.join(v, sub), exist_ok=True)
    if not guides:
        os.rmdir(os.path.join(v, "Guides"))
    if persona:
        with open(os.path.join(v, "Persona.md"), "w") as f:
            f.write("---\ntags: [persona]\n---\n\nWarm, brief, a little dry.\r\nNever a help desk.\n")
    if guides:
        with open(os.path.join(v, "Guides", "Sound.md"), "w") as f:
            f.write("---\nsource: owner\n---\n# Sound\n\nRestart it with systemctl --user restart pipewire.\n")
        with open(os.path.join(v, "Guides", "Games.md"), "w") as f:
            f.write("# Games\n\nProton-CachyOS is installed system-wide.\n")
    with open(os.path.join(v, "Entities", "Inside jokes.md"), "w") as f:
        f.write("# Inside jokes\n\n- 2026-10-17: Slothmothra is Sloth plus Mothra.\n- 2026-10-18: the fan does cardio.\n")
    with open(os.path.join(v, "Jokes", "2026-10.md"), "w") as f:
        f.write("# Jokes\n\n- 2026-10-22 09:09 | they said: a printer joke | I said: Why did the printer go to the doctor? It had a paper cut.\n")
    with open(os.path.join(v, "Jokes", "2026-11.md"), "w") as f:
        f.write("# Jokes\n\n- 2026-11-02 10:00 | they said: a cat joke | I said: My cat is a purr-fectionist about naps.\n")
    if about:
        with open(os.path.join(v, "Entities", "About Sam.md"), "w") as f:
            f.write("# About Sam\n\n## Observations\n\n"
                    "- Sam has a cat named Pixel — seen 3×, last 2026-10-01. Evidence: \"my cat Pixel\"\n"
                    "- Sam is learning to draw — seen 1×, last 2026-11-08.\n"
                    "- Sam likes the cat videos — seen 2×, last 2026-09-01.\n"
                    "- Sam once mentioned rain — seen 1×, last 2026-09-01.\n"
                    "- Sam plays Dead Space at night — seen 5×, last 2026-11-01.\n"
                    "\n## Forgotten\n\n- Sam's sister is called Ana — seen 4×, last 2026-10-21.\n")
    return v


SEARCH_OUT = ("Talks/2026-10.md  [meaning]\n  - 2026-10-20 21:00 | they said: my cat knocked the mug over | I said: Gravity tests.\n"
              "Guides/Sound.md  [meaning]\n  - restart pipewire\nGuides/Games.md  [keywords]\n  - proton\n"
              "Entities/Inside jokes.md  [meaning]\n  - 2026-10-17: Slothmothra is Sloth plus Mothra.\nHome.md  [keywords]\n  - 2026-10-01 x\n")

STREAM_DRAFT = ["<think>\nhm.\n</think>\n", "Let me check. ", "The fan is loud because ", "the CPU is busy. ",
                "Disable heimdall to help. Then", "\n[\x1edraft withdrawn\x1e", "]\n", "The fan is loud because the CPU is busy. ",
                "Close the game.\r\n", "It frees four cores. Café ", "☕ time? ", "1. Rebuild it. 2. Run it."]


@skip_without_originals
class Programs(unittest.TestCase):
    maxDiff = None

    def compare(self, args, env, scenario, setup=None, stderr=False, stdin=None):
        def prepare(d):
            add = fakes.install(os.path.join(d, "bin"), scenario)
            os.makedirs(os.path.join(d, "home"), exist_ok=True)
            os.makedirs(os.path.join(d, "work"), exist_ok=True)
            if setup:
                add.update(setup(d) or {})
            return add
        out = run_both(ASK, args, env, prepare, compare_stderr=stderr, stdin=stdin)
        py, rs = out["python"], out["rust"]
        self.assertEqual(py[0], rs[0], "exit status")
        self.assertEqual(py[1], rs[1], "output")
        self.assertEqual(py[2], rs[2], "error output")
        self.assertEqual(fakes.calls(py[3]), fakes.calls(rs[3]), "calls to the fakes")
        self.assertEqual(py[3], rs[3], "files")
        return rs

    # -- typed questions

    def test_typed_question_passes_through(self):
        r = self.compare(["--cwd", "<D>/work", "--write", "--shell", "what", "is", "in", "this", "folder?"], base_env(),
                         {"sarina chat": [{"out": "Three files.\n", "rc": 5}]})
        self.assertEqual(r[0], 5)
        self.assertIn("Three files.", r[1])

    def test_typed_question_in_a_conversation_keeps_its_session(self):
        scen = {"sarina chat": [{"out": "First.\n", "session": True}], "sarina resume": [{"out": "Second.\n"}]}

        def setup(d):
            os.makedirs(os.path.join(d, "sessions"))
            open(os.path.join(d, "sessions", "old.json"), "w").write("{}")
        self.compare(["--conversation", "abc-1", "--cwd", "<D>/work", "hello there"], base_env(), scen, setup)

    def test_an_odd_conversation_id_is_ignored(self):
        self.compare(["--conversation", "../x", "hi"], base_env(), {"sarina chat": [{"out": "Hi.\n"}]})

    # -- spoken questions about the computer

    def test_computer_question_streams_with_guides_and_prefetch(self):
        uid = os.getuid()
        crash = "Mon 2026-11-10 10:00:00 CET 1234 %d %d SIGSEGV present /home/u/.steam/steamapps/common/Game/game.exe 1.2M\n" % (uid, uid)
        scen = {"sarina route": [{"out": "performance,crashes\n"}], "sarina vault-search": [{"out": SEARCH_OUT}],
                "sarina system-status": [{"out": "load 5.2\n"}], "sarina crash-detail": [{"out": "signal 11 in libfoo\n"}],
                "coredumpctl": [{"out": crash}], "sarina chat": [{"stream": STREAM_DRAFT, "rc": 0}]}

        def setup(d):
            vault_files(d)
        env = base_env(SARINA_SYSTEM="on", SARINA_VAULT="<D>/vault", LABYRINTH_VOICE_RULES="Rules for {owner}.{owner_note}",
                       SARINA_OWNER_NAME="Sam", SARINA_OWNER_NOTE="Also called S.")
        r = self.compare(["--voice", "--cwd", "<D>/work", "why is the fan so loud while I play, is something crashing?"], env, scen, setup)
        self.assertIn("Sorry, let me correct that.", r[1])

    def test_a_general_status_question_reads_the_days_note(self):
        scen = {"sarina route": [{"out": "services,performance\n"}], "sarina daily-report": [{"out": "All good today.\n"}],
                "sarina chat": [{"stream": ["Everything is fine. ", "Nothing failed."]}]}
        env = base_env(SARINA_SYSTEM="on", SARINA_VAULT="<D>/nowhere")
        self.compare(["--voice", "how is the system doing today?"], env, scen)

    def test_route_unavailable_falls_back_to_keywords_and_guides(self):
        scen = {"sarina route": [{"out": "", "rc": 1}], "sarina vault-search": [{"out": SEARCH_OUT}],
                "sarina chat": [{"stream": ["Restart pipewire. ", "That fixes it."]}]}

        def setup(d):
            vault_files(d)
        self.compare(["--voice", "how do I fix the sound on my speaker"], base_env(SARINA_VAULT="<D>/vault"), scen, setup)

    def test_route_cache_is_used_and_kept(self):
        scen = {"sarina route": [{"out": "audio\n"}], "sarina chat": [{"stream": ["Fine."]}]}

        def setup(d):
            os.makedirs(os.path.join(d, "run", "labyrinth-sarina"))
            with open(os.path.join(d, "run", "labyrinth-sarina", "route-cache.json"), "w") as f:
                json.dump({"is the network okay now": ["network"], "x y z": "ab", "big": [1.5e300, 1e-7, 0.1, -0.0, 2]}, f)
        self.compare(["--voice", "is the network okay now"], base_env(), scen, setup)
        self.compare(["--voice", "is the sound okay now?"], base_env(), scen, setup)

    def test_a_damaged_route_cache_is_replaced(self):
        scen = {"sarina route": [{"out": "chat\n"}], "sarina chat": [{"stream": ["Hi."]}]}

        def setup(d):
            os.makedirs(os.path.join(d, "run", "labyrinth-sarina"))
            with open(os.path.join(d, "run", "labyrinth-sarina", "route-cache.json"), "w") as f:
                f.write("{not json")
        self.compare(["--voice", "good evening to you"], base_env(), scen, setup)

    # -- spoken talk (companion)

    def test_small_talk_with_what_she_knows_and_embeddings(self):
        scen = {"sarina route": [{"out": "chat\n"}], "sarina vault-search": [{"out": SEARCH_OUT}],
                "sarina-tone": [{"out": "They sound rushed\x07 lately.\n"}],
                "sarina chat": [{"stream": ["Pixel is probably asleep. ", "Want me to remind you to feed her? ", "Is there anything else?"]}]}

        def setup(d):
            vault_files(d)
        env = base_env(SARINA_VAULT="<D>/vault", SARINA_OWNER_NAME="Sam", SARINA_EMBED_URL=embed_url())
        r = self.compare(["--voice", "(Style hint, never mention it: be brief.) how is my cat doing, do you think"], env, scen, setup)
        self.assertEqual(r[1], "Pixel is probably asleep.\n")
        prompt = [c for c in fakes.calls(r[3]) if c["args"][:1] == ["chat"]][0]["args"][1]
        self.assertIn("What you have learned about them that bears on this", prompt)
        self.assertIn("Sam has a cat named Pixel", prompt)
        self.assertIn("They sound rushed  lately.", prompt)
        files = dict((f[0], f[2]) for f in r[3])
        self.assertIn("run/labyrinth-sarina/embed-cache.json", files)

    def test_small_talk_without_a_vault_or_tone(self):
        scen = {"sarina route": [{"out": "chat\n"}], "sarina chat": [{"stream": ["Good evening. ", "Let me know if you need anything."]}]}
        self.compare(["--voice", "good evening, how was your day"], base_env(LABYRINTH_TONE="0", LABYRINTH_VOICE_THINK="ON"), scen)

    def test_asked_to_remember_with_nothing_in_the_notes(self):
        scen = {"sarina route": [{"out": "chat\n"}], "sarina vault-search": [{"out": "Home.md  [keywords]\n  - 2026-10-01 x\n"}],
                "sarina chat": [{"stream": ["I do not remember that."]}]}

        def setup(d):
            vault_files(d, about=False)
        self.compare(["--voice", "do you remember what I said about the boat last time"], base_env(SARINA_VAULT="<D>/vault"), scen, setup)

    def test_a_joke_that_is_a_repeat_is_asked_again_in_the_conversation(self):
        scen = {"sarina route": [{"out": "chat\n"}], "sarina vault-search": [{"out": ""}],
                "sarina chat": [{"out": "<think>x</think>[joking] Why did the printer go to the doctor? It had a paper cut!\n", "session": True}],
                "sarina resume": [{"out": "(joking) Why do printers never win? 🖨️ They always jam under pressure.\n"}]}

        def setup(d):
            vault_files(d)
        self.compare(["--voice", "--conversation", "c1", "tell me a joke about printers"], base_env(SARINA_VAULT="<D>/vault"), scen, setup)

    def test_a_repeated_joke_without_a_conversation_is_asked_anew(self):
        scen = {"sarina route": [{"out": "chat\n"}],
                "sarina chat": [{"out": "Why did the printer go to the doctor? It had a paper cut.\n"},
                                {"out": "A new one about printers.\n"}]}

        def setup(d):
            vault_files(d)
        self.compare(["--voice", "got any jokes about printers?"], base_env(SARINA_VAULT="<D>/vault"), scen, setup)

    def test_a_conversation_laugh_echo_and_turns(self):
        scen = {"sarina route": [{"out": "chat\n"}], "sarina vault-search": [{"out": ""}],
                "sarina chat": [{"stream": ["[joking] It is doing cardio for both of us."], "session": True}],
                "sarina resume": [{"stream": ["Glad that landed."]}]}

        def setup(d):
            vault_files(d)
        r = self.compare(["--voice", "--conversation", "t1", "why is the fan loud tonight"], base_env(SARINA_VAULT="<D>/vault"), scen, setup)
        files = dict((f[0], f[2]) for f in r[3])
        self.assertEqual(files["run/labyrinth-sarina/conv-t1.session"], "s00")
        self.assertIn("why is the fan loud tonight\n", files["run/labyrinth-sarina/conv-t1.session.last"])

    def test_turns_of_one_conversation(self):
        """Three turns, each a separate run of the program in the same folder: the session is kept, a laugh makes an inside joke,
        her own voice coming back is not a question (exit 3)."""
        script = ("set -e\n"
                  "\"$PROG\" --voice --conversation t2 'why is the fan so loud tonight' > \"$OUT/1\"; echo $? >> \"$OUT/rc\"\n"
                  "\"$PROG\" --voice --conversation t2 '(They laughed.) haha okay that was good' > \"$OUT/2\"; echo $? >> \"$OUT/rc\"\n"
                  "\"$PROG\" --voice --conversation t2 'glad that landed with you, it really did' > \"$OUT/3\" || echo $? >> \"$OUT/rc\"\n")
        scen = {"sarina route": [{"out": "chat\n"}], "sarina vault-search": [{"out": ""}],
                "sarina chat": [{"stream": ["[joking] It is doing cardio for both of us."], "session": True}],
                "sarina resume": [{"stream": ["Glad that landed with you."]}]}
        out = {}
        import subprocess
        import tempfile
        import shutil
        from parity import program_cmd, snapshot
        for impl in ("python", "rust"):
            root = os.path.realpath(tempfile.mkdtemp(prefix="parity-turns-"))
            add = fakes.install(os.path.join(root, "bin"), scen)
            vault_files(root)
            os.makedirs(os.path.join(root, "out"))
            env = {k: v.replace("<D>", root) for k, v in base_env(SARINA_VAULT="<D>/vault").items()}
            env.update(add)
            wrapper = os.path.join(tempfile.mkdtemp(prefix="parity-prog-"), "prog")
            write_exe(wrapper, "#!/bin/sh\nexec %s \"$@\"\n" % " ".join("'%s'" % c for c in program_cmd(impl, ASK)))
            env.update({"PROG": wrapper, "OUT": os.path.join(root, "out")})
            subprocess.run(["/bin/sh", "-c", script], env=env, check=False, timeout=60)
            out[impl] = [[x.replace(wrapper, "<PROG>") if isinstance(x, str) else x for x in f] for f in snapshot(root)]
            shutil.rmtree(root, ignore_errors=True)
        self.assertEqual(out["python"], out["rust"])
        files = dict((f[0], f[2]) for f in out["rust"])
        self.assertIn("cardio for both of us", files["vault/Entities/Inside jokes.md"])
        self.assertEqual(files["out/3"], "I did not catch that: it sounded like my own voice.\n")

    # -- the answer, not streamed

    def test_a_long_answer_is_cleaned_and_shortened(self):
        long = ("<think>plan</think>(Style hint: x) What is the weather like for my bike ride. ## Plan\n"
                "1. Check the **tyres** (gentle).\n- Pump them to `4 bar`\n* Oil the chain\n\n\n" +
                " ".join("Sentence number %d is here." % i for i in range(30)) + " 😀 Would you like more help? (That is all from me today.)")
        scen = {"sarina route": [{"out": "chat\n"}], "sarina chat": [{"out": long}]}
        self.compare(["--voice", "what is the weather like for my bike ride"], base_env(LABYRINTH_VOICE_STREAM="0"), scen)

    def test_not_streamed_computer_answer_and_bad_bytes(self):
        scen = {"sarina route": [{"out": "disk\n"}], "sarina chat": [{"bytes": list(b"Disk is 90\xff% full.\r\nClean ~/.cache.\r") }]}
        self.compare(["--voice", "is my disk full or what"], base_env(LABYRINTH_VOICE_STREAM="0"), scen)

    # -- streamed edge cases

    def test_stream_with_split_utf8_and_carriage_returns(self):
        b = "Café is open. Über gut! Ja.\r\nNext line.\r".encode()
        chunks = [list(b[:4]), list(b[4:5]), list(b[5:20]), list(b[20:]), [0xe2, 0x82]]
        scen = {"sarina route": [{"out": "network\n"}], "sarina chat": [{"stream": chunks}]}
        self.compare(["--voice", "is the internet up right now"], base_env(), scen)

    def test_an_empty_answer_is_exit_4(self):
        scen = {"sarina route": [{"out": "network\n"}], "sarina chat": [{"stream": ["<think>only this</think>"]}]}
        r = self.compare(["--voice", "is the internet up right now"], base_env(), scen)
        self.assertEqual(r[0], 4)

    def test_a_failed_model_keeps_its_exit_status(self):
        scen = {"sarina route": [{"out": "network\n"}], "sarina chat": [{"stream": ["Half an ans"], "rc": 7}]}
        r = self.compare(["--voice", "is the internet up right now"], base_env(), scen)
        self.assertEqual(r[0], 7)

    def test_security_advice_and_warnings(self):
        scen = {"sarina route": [{"out": "network\n"}],
                "sarina chat": [{"stream": ["You could turn off the firewall for a moment. ", "Do not disable the firewall though. ",
                                            "Stopping apparmor is never needed. ", "I'll check the logs. ", "It is DNS."]}]}
        self.compare(["--voice", "why is the network so slow tonight"], base_env(), scen)

    # -- add-ons, extra tools, settings from sarina.env

    def test_addon_prefetch_extra_tools_and_the_env_file(self):
        def setup(d):
            vault_files(d, guides=False)
            conf = os.path.join(d, "home", ".config", "labyrinth")
            os.makedirs(conf)
            with open(os.path.join(conf, "sarina.env"), "w") as f:
                f.write("# settings\nSARINA_VAULT=%s/vault\nSARINA_OWNER_NAME = Sam\nLABYRINTH_VOICE_EXTRA_TOOLS=tarot_draw, bad tool;x,tarot-read\n"
                        "LABYRINTH_VOICE_PREFETCH=%s/bin/addon --card ; relative/addon ;%s/bin/nothing\nOPENAI_MODEL=small\nSARINA_TEMPERATURE=0.9\n" % (d, d, d))
        scen = {"sarina route": [{"out": "chat\n"}], "sarina vault-search": [{"out": ""}], "addon": [{"out": "Card drawn: The Tower.\n"}],
                "sarina chat": [{"stream": ["The Tower. ", "Change is coming."]}]}
        self.compare(["--voice", "I drew the tower card just now"], base_env(), scen, setup)

    def test_think_settings(self):
        scen = {"sarina route": [{"out": "chat\n"}], "sarina chat": [{"stream": ["Okay."]}]}
        for think in ("off", "computer", "on", "Off"):
            self.compare(["--voice", "nice weather we are having"], base_env(LABYRINTH_VOICE_THINK=think), scen)

    # -- what can go wrong

    def test_sarina_not_installed(self):
        def setup(d):
            os.remove(os.path.join(d, "bin", "sarina"))
        self.compare(["--voice", "is the internet up right now"], base_env(), {}, setup)

    def test_command_line_errors_and_help(self):
        for args in (["-h"], [], ["--c", "x", "q"], ["--cwd"], ["--write=1", "q"], ["q", "--voice", "q2"], ["-x", "q"],
                     ["--conv"], ["--bogus", "q"], ["-hx"], ["--voice", "--", "-5"], ["--wr", "--sh", "--vo=1"]):
            with self.subTest(args=args):
                self.compare(args, base_env(), {"sarina chat": [{"out": "ok\n"}], "sarina route": [{"out": "chat\n"}]}, stderr=True)

    def test_option_spellings_that_work(self):
        scen = {"sarina chat": [{"out": "ok\n"}], "sarina route": [{"out": "chat\n"}], "sarina resume": [{"out": "ok\n"}]}
        for args in (["--cw=<D>/work", "-5", "degrees"], ["--conversation=abc", "--", "--voice"], ["q", "--cwd", "<D>/work"],
                     ["--writ", "a b", "-c d"], ["--cwd", "-.5", "q"]):
            with self.subTest(args=args):
                self.compare(args, base_env(), scen, stderr=True)


if __name__ == "__main__":
    unittest.main()
