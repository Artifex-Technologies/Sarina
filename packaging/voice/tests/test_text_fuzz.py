"""Many generated texts through the text functions of both implementations (spoken notes, offers, markdown flattening, the
75-word cut, sentence cleaning, routing words, echo and repeat checks, the streamer fed a character at a time, JSON written back),
compared byte for byte. The texts mix the things the functions look for with awkward characters: separators \\x1c-\\x1f, line
breaks of every kind, combining-free accents, emoji, digits of other scripts. Fixed seeds, so a failure repeats."""
import json
import os
import random
import unittest

from parity import Hook, load_py, skip_without_originals

TOKENS = ["the", "cat", "CPU", "drive", "proton drive", "Proton Drive", "my drive", "disk", "Let me check.", "I'll", "Okay,", "ok.",
          "Want me to", "help", "assist", "anything else", "Is there anything", "firewall", "disable", "turn off", "do not", "never",
          "1.", "2.", "12.", "2026.", "3)", "**bold**", "__u__", "`code`", "# Heading", "### H3", "- item", "* item", "• item",
          "<think>", "</think>", "<b>", "</b>", "<x1>", "(They laughed.)", "(Style hint: brief)", "(joking)", "(Gentle)", "[gentle]",
          "[serious]", "😀", "☕", "‍", "️", "é", "Ü", "ß", "’", "…", " ", " ", "\x1e", "\x1c", "\x1f",
          "\x0b", "\x85", "\r", "\r\n", "\n", "\n\n", "\t", "  ", " ", " ", " ", ".", "!", "?", ":", ",", "(", ")",
          "Sorry", "haha", "lol", "password", "token", "A" * 33, "remember", "joke", "\x07", "‮", "game", "steam", "wifi",
          "sudo pacman", "slow", "boot", "sound", "error", "stutters", "what time is it", "tell me a joke", "make me laugh",
          "Would you like more?", "Let me know if you need anything.", "I'm here for you.", "\x1edraft withdrawn\x1e]",
          "(a long aside here)", "It is fine.", "Rebuild it.", "٣.", "Ä", "ı", "K", "drıve", "dİsk", "ſlow", "cpu²", "help‿", "wifi\u0301", "Ⓐ", "game\ufe0f", "\u200cboot", "½"]


def text(rng, n):
    return "".join(rng.choice(TOKENS) + (" " if rng.random() < 0.6 else "") for _ in range(n))


@skip_without_originals
class Fuzz(unittest.TestCase):
    maxDiff = None

    @classmethod
    def setUpClass(cls):
        cls.py = load_py("labyrinth-ask-sarina", "ask_fuzz")
        cls.hook = Hook()

    @classmethod
    def tearDownClass(cls):
        cls.hook.close()

    def same(self, name, py, rust):
        self.assertEqual(py, rust, name)

    def test_text_functions(self):
        rng = random.Random(20261110)
        m, h = self.py, self.hook
        for i in range(700):
            t = text(rng, rng.randint(0, 40))
            t2 = text(rng, rng.randint(0, 12))
            with self.subTest(i=i, text=t):
                self.same("spoken", m.spoken(t), h.call("spoken", t))
                self.same("laugh", bool(m.LAUGH_RX.search(t)), h.call("laugh", t))
                self.same("drop_offers", m.drop_offers(t), h.call("drop_offers", t))
                self.same("flatten", m.flatten(t), h.call("flatten", t))
                self.same("shorten", m.shorten(t), h.call("shorten", t))
                self.same("clean_sentence", m.clean_sentence(t), h.call("clean_sentence", t))
                self.same("keyword_topics", m.keyword_topics(t), h.call("keyword_topics", t))
                self.same("words_of", sorted(m.words_of(t)), h.call("words_of", t))
                self.same("is_echo", m.is_echo(t, t2), h.call("is_echo", t, t2))
                self.same("a_repeat", m.a_repeat(t, [t2, t]), h.call("a_repeat", t, [t2, t]))
                self.same("about", m.about_the_computer([], t), h.call("about_the_computer", [], t))
                self.same("plain", m.plain(t, 50), h.call("plain", t, 50))

    def test_the_streamer(self):
        rng = random.Random(7)
        m, h = self.py, self.hook
        for i in range(300):
            said = text(rng, rng.randint(0, 8))
            reply = text(rng, rng.randint(0, 60))
            chat = rng.random() < 0.5
            with self.subTest(i=i, said=said, reply=reply, chat=chat):
                st = m.Streamer(said, chat)
                sid = h.call("streamer_new", said, chat)
                for ch in reply:
                    self.same("feed", st.feed(ch), h.call("streamer_feed", sid, ch, False))
                self.same("finish", st.finish(), h.call("streamer_finish", sid))
                self.same("text", st.text(), h.call("streamer_text", sid))

    def test_json_written_back(self):
        rng = random.Random(3)
        values = [0, -0.0, 0.1, 1e-5, 1e-4, 1e16, 1e15, 123456789012345678, -2, 1.5e300, 5e-324, float("inf"), float("-inf"),
                  "é \x1f\"\\/", "😀", True, None, [], {}, 0.30000000000000004, 2.5e-7, 1234.5678]
        for i in range(300):
            v = {"k%d" % j: rng.choice(values) for j in range(rng.randint(0, 6))}
            if rng.random() < 0.3:
                v = [v, rng.choice(values), [rng.choice(values)]]
            s = json.dumps(v)
            with self.subTest(s=s):
                self.same("json", json.dumps(json.loads(s)), self.hook.call("json_roundtrip", s))
        for s in ('{"a": 1, "a": 2, "b": -0}', '[1.0, 1E2, -0.0, 0.10, NaN]', '{"x": "\\ud83d\\ude00\\u00e9"}', "  [ ]  ", '"x"'):
            self.same("json " + s, json.dumps(json.loads(s)), self.hook.call("json_roundtrip", s))

    def test_lines_words_and_paths(self):
        rng = random.Random(11)
        for i in range(300):
            t = text(rng, rng.randint(0, 20))
            self.same("splitlines", t.splitlines(), self.hook.call("splitlines", t))
            self.same("split", t.split(), self.hook.call("split_ws", t))
        for p in ("", ".", "/", "//", "///", "//a//b/", "a/../..", "/../a", "./a/./b/..", "a//b", "../../x", "/a/b/../../.."):
            self.same("normpath " + p, os.path.normpath(p), self.hook.call("normpath", p))


if __name__ == "__main__":
    unittest.main()
