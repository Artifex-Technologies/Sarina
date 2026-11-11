"""Carried over from Labyrinth_OS tests/test_labyrinth_ask_sarina_rules.py: labyrinth-ask-sarina --voice and the rules for a
spoken answer (the caller's LABYRINTH_VOICE_RULES win; a caller that passes none gets the installed ones), with a stand-in for
sarina, now run for the Python original and the Rust port side by side: the same exit status, output and arguments to sarina."""
import os
import unittest

from parity import run_both, skip_without_originals, write_exe

INSTALLED = "/usr/share/labyrinth/sarina-voice-rules.txt"
FAKE_SARINA = """#!/bin/sh
case "$1" in
  route) echo services ;;
  chat|resume) for a in "$@"; do printf '%s\\n' "$a"; done > "$HOME/args.txt"; printf 'It crashed in its renderer.\\n' ;;
esac
"""


@skip_without_originals
class VoiceRules(unittest.TestCase):
    maxDiff = None

    def ask(self, rules=None):
        def setup(d):
            os.makedirs(os.path.join(d, "bin"))
            write_exe(os.path.join(d, "bin", "sarina"), FAKE_SARINA)
        env = {"PATH": "<D>/bin:/usr/bin:/bin", "HOME": "<D>", "XDG_CONFIG_HOME": "<D>/.config", "XDG_RUNTIME_DIR": "<D>",
               "SARINA_SESSION_DIR": "<D>/sessions", "SARINA_VAULT": "<D>/vault", "LABYRINTH_TALK_LOG": "0",
               "SARINA_EMBED_URL": "http://127.0.0.1:1/api/embed", "LABYRINTH_VOICE_STREAM": "0"}
        if rules is not None:
            env["LABYRINTH_VOICE_RULES"] = rules
        out = run_both("labyrinth-ask-sarina", ["--voice", "--cwd", "<D>", "The program game just crashed (SIGSEGV). Look into it."], env, setup)
        self.assertEqual(out["python"], out["rust"], "Python and Rust differ")
        rc, stdout, _, files = out["rust"]
        args = dict((f[0], f[2]) for f in files)["args.txt"]
        return rc, stdout, args

    def test_the_callers_rules_win(self):
        rc, stdout, args = self.ask("Rules the caller passed.")
        self.assertIn("It crashed in its renderer.", stdout)
        self.assertIn("Rules the caller passed.", args)

    @unittest.skipUnless(os.path.isfile(INSTALLED), "the voice rules are not installed here")
    def test_installed_rules_when_none_are_passed(self):
        with open(INSTALLED, encoding="utf-8") as f:
            first = f.read().strip().splitlines()[0][:60]
        rc, stdout, args = self.ask()
        self.assertIn("It crashed in its renderer.", stdout)
        self.assertIn(first, args)

    def test_empty_rules_stay_empty(self):
        # labyrinth-sarina-ask exports what it read, even nothing: that is the caller's choice, not "none passed"
        if not os.path.isfile(INSTALLED):
            self.skipTest("the voice rules are not installed here")
        with open(INSTALLED, encoding="utf-8") as f:
            first = f.read().strip().splitlines()[0][:60]
        _, _, args = self.ask("")
        self.assertNotIn(first, args)


if __name__ == "__main__":
    unittest.main()
