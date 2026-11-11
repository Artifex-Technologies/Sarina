"""labyrinth-sarina-checkup as a whole program, the Python original and the Rust port side by side, with stand-ins for everything
it starts (nvidia-smi, coredumpctl, systemctl, df, journalctl, sarina, labyrinth-ask-sarina, notify-send) and for Heimdall's API,
which runs in a network namespace of the test's own (unshare -rn) so the real Heimdall is never asked. Inside it this account is
uid 0, which is why the crash lines below name uid 0. Exit status, output, files and calls must be identical."""
import json
import os
import re
import shutil
import subprocess
import sys
import unittest

import fakes
from parity import HERE, run_both, skip_without_originals

CHECKUP = "labyrinth-sarina-checkup"


def namespaces_work():
    try:
        return shutil.which("unshare") and shutil.which("ip") and subprocess.run(["unshare", "-rn", "true"]).returncode == 0
    except OSError:
        return False


AUDIT = {"packages": [{"package": "lua51", "installed": "5.1.5-13.1", "severity": "Low", "advisory": "AVG-1302"},
                      {"package": "openssl", "installed": "3.5.1-1", "severity": "High", "advisory": "AVG-9"}],
         "kernels": [{"kernel": "7.3.0-4-cachyos", "running": True, "count": 0}, {"kernel": "6.18.55-2-cachyos-lts", "running": False, "count": 733}],
         "hardening": [{"id": "kvm-world-writable", "now": "/dev/kvm is 0666 (every account)"}],
         "extra": {"é": 1.5e-7}}
QUARANTINE = [{"sha256": "ab" * 32, "original": "/home/u/.local/bin/mytool", "status": "held", "reason": "unsigned"},
              {"sha256": "cd" * 32, "original": "/opt/old", "status": "released"},
              {"sha256": "ef" * 32, "original": "/gone", "copies": ["/srv/tool"], "status": "held", "reason": "new"}]
CRASHES = ("Mon 2026-11-10 10:00:00 CET 1234 0 0 SIGSEGV present /usr/bin/spotify 1.2M\n"
           "Mon 2026-11-10 10:05:00 CET 1235 0 0 SIGSEGV present /usr/bin/spotify 1.2M\n"
           "Mon 2026-11-10 10:06:00 CET 1236 1000 1000 SIGABRT present /usr/bin/other 1M\n"
           "Mon 2026-11-10 10:07:00 CET 1237 0 0 SIGABRT present /home/u/.local/bin/mytool 1M\n")
DF = "Use% Mounted on\n 45% /\n 93% /home\n100% /mnt/games\n"


@skip_without_originals
@unittest.skipUnless(namespaces_work(), "needs unshare -rn and ip (a network namespace of the test's own)")
class Checkup(unittest.TestCase):
    maxDiff = None

    def compare(self, args, scenario, audit=None, quarantine=None, state=None, vault=True, extra_env=None):
        def setup(d):
            add = fakes.install(os.path.join(d, "bin"), scenario)
            os.makedirs(os.path.join(d, "home"))
            os.makedirs(os.path.join(d, "requests"))
            if vault:
                os.makedirs(os.path.join(d, "vault"))
            if audit is not None:
                with open(os.path.join(d, "audit.json"), "w") as f:
                    f.write(audit if isinstance(audit, str) else json.dumps(audit))
            if quarantine is not None:
                with open(os.path.join(d, "quarantine.json"), "w") as f:
                    f.write(quarantine if isinstance(quarantine, str) else json.dumps(quarantine))
            if state is not None:
                os.makedirs(os.path.join(d, "state", "sarina"))
                with open(os.path.join(d, "state", "sarina", "checkup.json"), "w") as f:
                    f.write(state if isinstance(state, str) else json.dumps(state))
            return add
        env = {"PATH": "<D>/bin", "HOME": "<D>/home", "XDG_CONFIG_HOME": "<D>/home/.config", "XDG_STATE_HOME": "<D>/state",
               "XDG_DATA_HOME": "<D>/data", "LABYRINTH_HEIMDALL_REQUESTS": "<D>/requests"}
        if vault:
            env["SARINA_VAULT"] = "<D>/vault"
        env.update(extra_env or {})
        inside = [shutil.which("unshare"), "-rn", sys.executable, "-I", os.path.join(HERE, "heimdall_ns.py"), "<D>/audit.json", "<D>/quarantine.json",
                  "<D>/heimdall.log", "--"]
        out = run_both(CHECKUP, args, env, setup, inside=inside)
        # the programs read the real clock: the moment (to the second) is blanked, the rest must be the same
        when = lambda t: re.sub(r"(?m)^- \d\d:\d\d check-up", "- <hm> check-up", re.sub(r"\d{4}-\d\d-\d\d \d\d:\d\d(:\d\d)?", "<time>", t))
        norm = lambda files: [[re.sub(r"-\d+\.json$", "-<pid>.json", f[0]), f[1], when(f[2])] for f in files]
        py, rs = out["python"], out["rust"]
        self.assertEqual(py[0], rs[0], "exit status")
        self.assertEqual(py[1], rs[1], "output")
        self.assertEqual(fakes.calls(norm(py[3])), fakes.calls(norm(rs[3])), "calls to the fakes")
        self.assertEqual(norm(py[3]), norm(rs[3]), "files")
        return rs

    def quiet(self, **more):
        s = {"nvidia-smi": [{"out": "12\n"}], "coredumpctl": [{"out": ""}], "systemctl": [{"out": ""}], "df": [{"out": "Use% Mounted on\n 40% /\n"}],
             "journalctl": [{"out": ""}], "sarina daily-report": [{"out": "status kept\n"}]}
        s.update(more)
        return s

    def test_all_fine(self):
        r = self.compare([], self.quiet())
        self.assertEqual(r[1], "all fine\n")

    def test_the_graphics_card_is_busy(self):
        r = self.compare([], {"nvidia-smi": [{"out": " 85\n3\n"}]})
        self.assertEqual(r[0], 75)

    def test_findings_are_explained_noted_and_notified(self):
        scen = self.quiet(**{"coredumpctl": [{"out": CRASHES}], "systemctl": [{"out": "run-u5.service loaded failed\nbluetooth.service loaded failed failed\n"},
                                                                                {"out": "waybar.service loaded failed\n"}],
                             "df": [{"out": DF}], "journalctl": [{"out": "a\nb\n\nc\nd\ne\n"}],
                             "labyrinth-ask-sarina": [{"out": "Spotify crashed twice.\nRun it from a terminal.\n"}], "notify-send": [{"out": ""}],
                             "sarina skill": [{"out": "Summary: update openssl first.\nDo now: 1. pacman -Syu\n"}]})
        state = {"last": "2026-11-09 08:00:00", "security_seen": ["pkg|lua51|5.1.5-13.1|AVG-1302", 3, "gone"], "kept": {"x": [1, 2.5]}}
        r = self.compare([], scen, audit=AUDIT, quarantine=QUARANTINE, state=state)
        self.assertIn("spotify crashed 2 times", r[1])

    def test_no_answer_from_her_and_no_vault(self):
        scen = self.quiet(**{"df": [{"out": DF}], "labyrinth-ask-sarina": [{"out": "", "rc": 1}], "notify-send": [{"out": ""}],
                             "sarina skill": [{"out": ""}]})
        self.compare([], scen, audit={"packages": [], "kernels": [{"kernel": "k", "count": 2}]}, quarantine="not json", vault=False)

    def test_a_damaged_state_file_and_heimdall_answering_nonsense(self):
        self.compare([], self.quiet(), audit="{oops", state="[1, 2")

    def test_held_files_are_asked_back(self):
        scen = {"systemctl": [{"out": "mytool.service loaded failed failed\n"},
                              {"out": "{ path=/home/u/.local/bin/mytool ; argv[]=/home/u/.local/bin/mytool --serve ; }\n"},
                              {"out": "srv.service loaded failed\n"}, {"out": "/srv/tool\n"}],
                "coredumpctl": [{"out": CRASHES}]}
        r = self.compare(["--held"], scen, quarantine=QUARANTINE)
        self.assertIn("asked Heimdall to release", r[1])

    def test_held_with_nothing_held(self):
        r = self.compare(["--held"], {}, quarantine=[])
        self.assertEqual(r[1], "")


if __name__ == "__main__":
    unittest.main()
