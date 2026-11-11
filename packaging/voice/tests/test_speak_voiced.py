"""labyrinth-sarina-speak-voiced, the Python original and the Rust port side by side, against a stand-in for the voice service
(labyrinth-voiced) on a Unix socket in the test's folder and a stand-in player that writes down what it was asked to play and the
loudness file (speech.env) it found. No sound is made. Exit status, error output, what was sent to the service, what was played,
the loudness numbers and the files left behind must be identical (the start time in speech.env is blanked)."""
import json
import math
import os
import re
import socket
import struct
import threading
import unittest

from parity import run_both, skip_without_originals, write_exe

PROG = "labyrinth-sarina-speak-voiced"
PLAYER = """#!/bin/sh
{ echo "played $1"; cat "$XDG_RUNTIME_DIR/labyrinth-sarina/speech.env" 2>/dev/null || echo "no speech.env"; } >> "$PLAYLOG"
"""


def wav(path, rate=22050, channels=1, width=2, frames=None, extensible=False, odd=False):
    """A wav file with a voice-like rise and fall (and a pause), written by hand so every header form can be made."""
    n = frames if frames is not None else rate // 2
    samples = []
    for i in range(n):
        env = 0.0 if n // 3 < i < n // 2 else abs(math.sin(i / n * math.pi))
        v = int(9000 * env * math.sin(i * 0.07) + 300 * math.sin(i * 0.9))
        for c in range(channels):
            if width == 2:
                samples.append(struct.pack("<h", v))
            else:
                samples.append(struct.pack("<B", (v >> 8) + 128 & 0xff))
    data = b"".join(samples)
    if odd:
        data += b"\x01"
    if extensible:
        fmt = struct.pack("<HHIIHH", 0xFFFE, channels, rate, rate * channels * width, channels * width, width * 8)
        fmt += struct.pack("<HHI", 22, width * 8, 4) + bytes([1, 0, 0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xaa, 0, 0x38, 0x9b, 0x71])
    else:
        fmt = struct.pack("<HHIIHH", 1, channels, rate, rate * channels * width, channels * width, width * 8)
    body = b"WAVE" + b"fmt " + struct.pack("<I", len(fmt)) + fmt + b"LIST" + struct.pack("<I", 3) + b"abc\x00"
    body += b"data" + struct.pack("<I", len(data)) + data
    with open(path, "wb") as f:
        f.write(b"RIFF" + struct.pack("<I", len(body)) + body)


class Voiced(threading.Thread):
    """Accepts one client, writes down its request, makes the wav files and sends the scripted lines."""

    def __init__(self, sock_path, run, script, log):
        super().__init__(daemon=True)
        self.s = socket.socket(socket.AF_UNIX)
        self.s.bind(sock_path)
        self.s.listen(1)
        self.s.settimeout(30)
        self.run_dir, self.script, self.log = run, script, log

    def run(self):
        try:
            c, _ = self.s.accept()
        except OSError:
            return
        f = c.makefile("rwb")
        with open(self.log, "ab") as lg:
            lg.write(b"request " + f.readline())
        for item in self.script:
            if isinstance(item, bytes):
                f.write(item)
                f.flush()
                continue
            if "make" in item:
                path = os.path.join(self.run_dir, item["make"][0])
                item["make"][1](path)
                msg = dict(item["msg"], wav=path)
            else:
                msg = item["msg"]
            f.write((json.dumps(msg) + "\n").encode())
            f.flush()
        try:
            f.close()
            c.close()
        except OSError:
            pass
        self.s.close()


@skip_without_originals
class SpeakVoiced(unittest.TestCase):
    maxDiff = None

    def compare(self, args, script, sock=True, env_extra=None):
        servers = []

        def setup(d):
            run = os.path.join(d, "run", "labyrinth-sarina")
            os.makedirs(run)
            os.makedirs(os.path.join(d, "bin"))
            write_exe(os.path.join(d, "bin", "player"), PLAYER)
            if sock:
                v = Voiced(os.path.join(run, "voiced.sock"), run, script, os.path.join(d, "service.log"))
                v.start()
                servers.append(v)
            return {}
        env = {"PATH": "/usr/bin:/bin", "XDG_RUNTIME_DIR": "<D>/run", "LABYRINTH_PLAYER": "<D>/bin/player", "PLAYLOG": "<D>/played.log"}
        env.update(env_extra or {})
        out = run_both(PROG, args, env, setup, compare_stderr=True)
        for v in servers:
            v.join(5)
        blank = lambda files: [[f[0], f[1], re.sub(r"(?m)^\d{13}$", "<ms>", f[2])] for f in files if not f[0].endswith("voiced.sock")]
        py, rs = out["python"], out["rust"]
        self.assertEqual(py[0], rs[0], "exit status")
        self.assertEqual(py[1], rs[1], "output")
        self.assertEqual(re.sub(r"(?s)Traceback.*", "<crash>", py[2]), re.sub(r"(?s)Traceback.*", "<crash>", rs[2]), "error output")
        self.assertEqual(blank(py[3]), blank(rs[3]), "files")
        return rs, blank(rs[3])

    def test_sentences_are_played_with_their_loudness(self):
        script = [{"make": ("s1.wav", wav), "msg": {"pause_before_ms": 10, "pause_after_ms": "20"}},
                  {"msg": {"wav": "/tmp/elsewhere.wav"}},
                  {"make": ("s2.wav", lambda p: wav(p, rate=16000, channels=2)), "msg": {"pause_before_ms": None, "pause_after_ms": 2.7}},
                  {"make": ("s3.wav", lambda p: wav(p, rate=48000, extensible=True)), "msg": {}},
                  {"make": ("s4.wav", lambda p: wav(p, frames=0)), "msg": {}},
                  {"make": ("s5.wav", lambda p: wav(p, rate=8000, width=1, frames=400)), "msg": {}},
                  {"msg": {"done": True}}]
        r, files = self.compare(["Hello", "there, Café ☕ \"quoted\""], script)
        self.assertEqual(r[0], 0)
        played = dict((f[0], f[2]) for f in files)["played.log"]
        self.assertEqual(played.count("played "), 5)
        self.assertNotIn("s1.wav", [f[0] for f in files])  # each sentence's file is deleted after it is played

    def test_an_error_from_the_service(self):
        r, _ = self.compare(["Hi"], [{"msg": {"error": ["model", "failed"]}}])
        self.assertEqual(r[0], 1)
        self.assertIn("voiced: ['model', 'failed']", r[2])

    def test_what_ends_the_loop(self):
        for script in ([{"make": ("a.wav", wav), "msg": {"pause_before_ms": -5}}],
                       [{"make": ("a.wav", lambda p: wav(p, width=1, frames=301, channels=1)), "msg": {}}],
                       [{"make": ("a.wav", wav), "msg": {}}, b"not json\n", {"msg": {"done": True}}],
                       [{"make": ("a.wav", wav), "msg": {"pause_after_ms": "soon"}}],
                       [{"make": ("a.wav", lambda p: open(p, "wb").write(b"RIFF\x04\x00\x00\x00WAVE")), "msg": {}}],
                       [{"make": ("a.wav", lambda p: open(p, "wb").write(b"not a wav file at all")), "msg": {}}],
                       [b"[1, 2]\n"],
                       [{"make": ("a.wav", wav), "msg": {"pause_before_ms": [1]}}],
                       []):
            with self.subTest(script=str(script)[:80]):
                self.compare(["Hi"], script)

    def test_no_service_and_no_text(self):
        r, _ = self.compare(["Hi"], [], sock=False)
        self.assertEqual(r[0], 1)
        r, _ = self.compare([], [], sock=False)
        self.assertEqual(r[0], 2)
        r, _ = self.compare(["Hi"], [], sock=False, env_extra={"LABYRINTH_VOICED_SOCKET": ""})
        self.assertEqual(r[0], 1)


if __name__ == "__main__":
    unittest.main()
