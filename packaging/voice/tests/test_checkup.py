"""Carried over from Labyrinth_OS tests/test_sarina_checkup_daily.py and test_sarina_checkup_security.py: the check-up's line in
the day's note (above the status `sarina daily-report` keeps), Heimdall's findings turned into keys, Sarina's proposal saved with
one notification (sarina and notify-send are stand-ins), and a held file asked back. Each body runs against the Python original
and the Rust port; the calls, answers and files must be identical."""
import datetime
import os
import stat
import tempfile
import unittest

from parity import ParityCase, RustCheckup, skip_without_originals

AUDIT = {
    "packages": [{"package": "lua51", "installed": "5.1.5-13.1", "severity": "Low", "advisory": "AVG-1302"}],
    "kernels": [{"kernel": "7.3.0-4-cachyos", "running": True, "count": 0}, {"kernel": "6.18.55-2-cachyos-lts", "running": False, "count": 733}],
    "hardening": [{"id": "kvm-world-writable", "now": "/dev/kvm is 0666 (every account)"}],
}


def side_by_side(cls):
    for name in list(vars(cls)):
        if name.startswith("check_"):
            setattr(cls, "test_" + name[6:], (lambda n: lambda self: self.both(getattr(self, n)))(name))
    return cls


class Base(ParityCase):
    SCRIPT, RUST = "labyrinth-sarina-checkup", RustCheckup

    def load(self):
        return self.m


@skip_without_originals
@side_by_side
class DailyNote(Base):
    def check_a_new_note_gets_a_header_and_the_line(self):
        vault = tempfile.mkdtemp()
        self.load().note(vault, "all fine")
        today = datetime.date.today().isoformat()
        text = open(os.path.join(vault, "Daily", today + ".md")).read()
        self.assertTrue(text.startswith("---\ncreated: %s\nsource: labyrinth-sarina-checkup\ntags: [daily]\n---\n\n# %s\n\n- " % (today, today)))
        self.assertIn("check-up: all fine\n", text)

    def check_the_line_goes_above_the_status_sections(self):
        vault = tempfile.mkdtemp()
        today = datetime.date.today().isoformat()
        os.makedirs(os.path.join(vault, "Daily"))
        p = os.path.join(vault, "Daily", today + ".md")
        with open(p, "w") as f:
            f.write("---\ncreated: %s\n---\n\n# %s\n\n- 10:00 check-up: all fine\n\n## Status (12:00)\n\n- Load: 0.5\n\n## Numbers\n\n- 12:00 | disk / 40%% | crashes 0 | failed 0 | memory 30%% | load 0.5\n" % (today, today))
        self.load().note(vault, "spotify crashed 2 times")
        text = open(p).read()
        self.assertLess(text.index("10:00 check-up"), text.index("spotify crashed"))
        self.assertLess(text.index("spotify crashed"), text.index("## Status (12:00)"))
        self.assertTrue(text.endswith("load 0.5\n"))
        self.assertEqual(text.count("## Status"), 1)


@skip_without_originals
@side_by_side
class Security(Base):
    def check_keys(self):
        keys = self.load().security_keys(AUDIT)
        self.assertEqual(sorted(keys), ["hard|kvm-world-writable|/dev/kvm is 0666 (every account)", "kernel|6.18.55-2-cachyos-lts|733", "pkg|lua51|5.1.5-13.1|AVG-1302"])
        self.assertEqual(keys["kernel|6.18.55-2-cachyos-lts|733"], "kernel 6.18.55-2-cachyos-lts: 733 known holes")

    def check_keys_with_missing_and_odd_fields(self):
        # not in the Labyrinth test: fields missing (None), a running kernel, a held file, a hardening entry without "now"
        audit = {"packages": [{"package": "x"}], "kernels": [{"kernel": "k", "running": True, "count": 2}],
                 "hardening": [{"id": "h"}], "held": [{"sha256": "ab", "original": "/p", "reason": "r"}]}
        self.load().security_keys(audit)

    def check_review_is_saved_in_the_vault_and_notified(self):
        bin_dir, vault = tempfile.mkdtemp(), tempfile.mkdtemp()
        log = os.path.join(bin_dir, "log")
        for name, body in (("sarina", 'echo "$@" >> %s; echo "Summary: nothing urgent."; echo "Do now: 1. fix kvm"' % log),
                           ("notify-send", 'echo "notify $@" >> %s' % log)):
            p = os.path.join(bin_dir, name)
            with open(p, "w") as f:
                f.write("#!/bin/sh\n" + body + "\n")
            os.chmod(p, os.stat(p).st_mode | stat.S_IEXEC)
        mod = self.load()
        os.environ["PATH"] = bin_dir + os.pathsep + os.environ["PATH"]
        out = mod.security_review(dict(os.environ), vault, ["/dev/kvm is 0666 (every account)"])
        self.assertIn("saved in", out)
        sec = os.path.join(vault, "Security")
        text = open(os.path.join(sec, os.listdir(sec)[0])).read()
        self.assertIn("tags: [security]", text)
        self.assertIn("Do now: 1. fix kvm", text)
        calls = open(log).read()
        self.assertIn("skill run security-advisor new since the last check-up: /dev/kvm", calls)
        self.assertIn("notify -a Sarina -i security-high Security review nothing urgent.", calls)
        # a second review the same day goes below the first, without a second header
        out = mod.security_review(dict(os.environ), vault, ["kernel 1: 2 known holes"])
        self.assertEqual(open(os.path.join(sec, os.listdir(sec)[0])).read().count("tags: [security]"), 1)

    def check_review_without_an_answer(self):
        bin_dir = tempfile.mkdtemp()
        p = os.path.join(bin_dir, "sarina")
        with open(p, "w") as f:
            f.write("#!/bin/sh\nexit 0\n")
        os.chmod(p, 0o755)
        os.environ["PATH"] = bin_dir + os.pathsep + os.environ["PATH"]
        out = self.load().security_review(dict(os.environ), "", ["a", "b"])
        self.assertIn("could not review them", out)


@skip_without_originals
@side_by_side
class Held(Base):
    def check_a_broken_program_whose_file_is_held_asks_heimdall(self):
        mod = self.load()
        spool = tempfile.mkdtemp()
        mod.REQUESTS = spool
        held = [{"sha256": "ab" * 32, "original": "/home/u/.local/bin/mytool", "status": "held"},
                {"sha256": "cd" * 32, "original": "/home/u/.sarina/skills/evil.md", "status": "held"}]
        broken = [("{ path=/home/u/.local/bin/mytool ; argv[]=/home/u/.local/bin/mytool --serve ; }", "the service mytool.service failed (your session)"),
                  ("/usr/bin/spotify", "spotify crashed")]
        asked = mod.held_check(held, broken)
        self.assertEqual(asked, ["/home/u/.local/bin/mytool: the service mytool.service failed (your session)"])
        reqs = os.listdir(spool)
        self.assertEqual(len(reqs), 1)
        body = open(os.path.join(spool, reqs[0])).read()
        self.assertIn('"release_if_proven": "' + "ab" * 32, body)
        # the request's name carries the asking process's id: made the same for the comparison
        os.rename(os.path.join(spool, reqs[0]), os.path.join(spool, "request.json"))

    def check_copies_and_an_exact_command_line(self):
        mod = self.load()
        spool = tempfile.mkdtemp()
        mod.REQUESTS = spool
        held = [{"sha256": "ef" * 32, "original": "/gone", "copies": ["/opt/tool"], "status": "held"}]
        self.assertEqual(mod.held_check(held, [("/opt/tool", "tool crashed")]), ["/opt/tool: tool crashed"])
        for n in os.listdir(spool):
            os.rename(os.path.join(spool, n), os.path.join(spool, "request.json"))

    def check_nothing_held_nothing_asked(self):
        self.assertEqual(self.load().held_check([], [("x", "y")]), [])


if __name__ == "__main__":
    unittest.main()
