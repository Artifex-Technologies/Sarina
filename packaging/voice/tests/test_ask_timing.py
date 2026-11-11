"""Carried over from Labyrinth_OS tests/test_labyrinth_ask_sarina_timing.py: labyrinth-ask-sarina's timing line
(LABYRINTH_VOICE_TIMING=1) with a stand-in for sarina, for the Python original and the Rust port side by side. The seconds differ
from run to run, so the two are compared with the numbers after the whisper value blanked."""
import os
import re
import unittest

from parity import run_both, skip_without_originals, write_exe

FAKE_SARINA = """#!/bin/sh
case "$1" in
  route) echo services ;;
  chat|resume) printf 'Nothing is slowing it down. The load is low.\\n' ;;
esac
"""


def blank_times(files):
    out = []
    for f in files:
        if f[0].endswith("timing.log"):
            f = [f[0], f[1], re.sub(r"^\S+ ", "<when> ", re.sub(r"(?<!whisper)=\d+\.\d\d", "=<s>", f[2]), flags=re.M)]
        out.append(f)
    return out


@skip_without_originals
class Timing(unittest.TestCase):
    maxDiff = None

    def run_it(self, timing, whisper=None):
        def setup(d):
            os.makedirs(os.path.join(d, "bin"))
            write_exe(os.path.join(d, "bin", "sarina"), FAKE_SARINA)
        env = {"PATH": "<D>/bin:/usr/bin:/bin", "HOME": "<D>", "XDG_CONFIG_HOME": "<D>/.config", "XDG_RUNTIME_DIR": "<D>",
               "SARINA_SESSION_DIR": "<D>/sessions", "SARINA_VAULT": "<D>/vault", "LABYRINTH_TALK_LOG": "0",
               "SARINA_EMBED_URL": "http://127.0.0.1:1/api/embed"}
        if timing:
            env["LABYRINTH_VOICE_TIMING"] = "1"
        if whisper:
            env["LABYRINTH_VOICE_WHISPER_S"] = whisper
        out = run_both("labyrinth-ask-sarina", ["--voice", "--cwd", "<D>", "is my computer running slow right now"], env, setup)
        py, rs = out["python"], out["rust"]
        self.assertEqual(py[:3], rs[:3], "exit status or output differ")
        self.assertEqual(blank_times(py[3]), blank_times(rs[3]), "files differ")
        log = dict((f[0], f[2]) for f in rs[3]).get("labyrinth-sarina/timing.log")
        return rs, log

    def test_one_line_with_every_stage_in_order(self):
        r, log = self.run_it(True, whisper="1.25")
        self.assertIn("Nothing is slowing it down.", r[1])
        self.assertIsNotNone(log)
        lines = log.splitlines()
        self.assertEqual(len(lines), 1, log)
        stages = re.findall(r"(\w+)=(\d+\.\d\d)", lines[0])
        self.assertEqual([s for s, _ in stages], ["whisper", "route", "prefetch", "first_token", "first_sentence", "done"], lines[0])
        self.assertEqual(stages[0][1], "1.25")
        after = [float(v) for _, v in stages[1:]]
        self.assertEqual(after, sorted(after), lines[0])

    def test_nothing_written_when_off(self):
        r, log = self.run_it(False)
        self.assertIn("Nothing is slowing it down.", r[1])
        self.assertIsNone(log)


if __name__ == "__main__":
    unittest.main()
