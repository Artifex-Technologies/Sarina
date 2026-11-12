# Sarina's spoken-question programs in Rust (2026-11-10)

I want as much of the desktop in Rust as possible, and the spoken path was paying about 48 ms of Python start-up per question.
Three programs that lived as Python in Labyrinth's image (`Labyrinth_OS/profile/airootfs/usr/local/bin`) are now one Rust binary,
`sarina-voice`, in this repository (`packaging/voice/`, step 1 of `docs/VOICE_PACKAGE_PLAN.md`):

| Program | What it is | Python | Rust |
| --- | --- | --- | --- |
| `labyrinth-ask-sarina` | the conversation (Super+S, the crash watch's "Ask Sarina", her evals) | 1,017 lines | `src/ask.rs` |
| `labyrinth-sarina-checkup` | the daily check-up and `--held` (what Heimdall holds) | 341 | `src/checkup.rs` |
| `labyrinth-sarina-speak-voiced` | the client of the voice service, per sentence | 91 | `src/speak_voiced.rs` |

`sarina-voice` answers to the name it is started as (a link named `labyrinth-ask-sarina` and so on), or to its first argument
(`sarina-voice ask-sarina ...`). The command lines, standard input and output, files, environment variables and exit statuses are
the Python ones. The voice service (`labyrinth-voiced` and the Chatterbox engine) was ported afterwards, separately:
`packaging/voiced` (`sarina-voiced`, 2026-11-11; its README). The shell scripts
(`labyrinth-sarina-ask`, `-speak`, `-warm`, `-setting`) are not ported yet: see the end.

Standard library plus two crates already in Arch (`regex`, `libc`); `cargo build --offline --release` in `packaging/voice`.
The installed binary is 2.3 MB.

## How it is kept the same

`src/py.rs`, `src/rx.rs` and `src/json.rs` carry the parts of Python's behaviour the scripts depend on, so the output stays byte
for byte the same: Python's whitespace and line breaks (`\x1c`-`\x1f` count, and one of them marks a withdrawn draft), text files
read with universal newlines, `json.dumps` (key order, `ensure_ascii`, float `repr`), `sum()` with Neumaier's compensation and C's
`pow` (the "what she learned" scores), argparse (prefixes, `--opt=value`, `--`, negative numbers, the same errors and exit 2),
Python's look-behinds and back-references written by hand, and Python's `\b`, `\w` and `re.IGNORECASE` (`rx::pyb`: "ı" and "İ"
match "i", a combining mark or U+FE0F after a word is a word boundary).

`packaging/voice/tests/` runs the Python original and the Rust port side by side (`python -m unittest discover -s
packaging/voice/tests`; it finds the Python in `LABYRINTH_OS_DIR` or a `Labyrinth_OS` checkout next to this one):

- every Labyrinth test of these programs, carried over: `test_labyrinth_ask_sarina_companion.py` (including the ten tests that
  sat below `unittest.main()` and never ran), `_rules.py`, `_timing.py`, `test_sarina_checkup_daily.py`, `_security.py`. Each
  test body runs against both; the calls, answers and files left behind must be identical, with the clock fixed;
- the whole programs with stand-ins for everything they start (`fakes.py`; PATH holds nothing else, so the real sarina, model,
  whisper, coredumpctl or speaker can never be reached): 26 cases of `labyrinth-ask-sarina` (typed, spoken, chat with
  embeddings from a stand-in server, jokes and their repeats, conversations over three turns, a withdrawn draft, split UTF-8,
  bad bytes, add-ons, `sarina.env`, a damaged cache, every command-line error), 7 of the check-up (Heimdall's API stood in
  inside a network namespace of the test's own, `unshare -rn`, so the real one is never asked), 4 of the voice client (a
  stand-in voice service on a socket, wav files of each header form, every way the loop ends);
- 700 generated texts through the text functions, 300 replies fed to the streamer a character at a time, JSON and paths.

72 tests, all identical. A changed word in the spoken-answer rule makes 21 of them fail, so they do compare.

## Time saved

`python3 packaging/voice/tests/bench_voice.py` (stand-ins that answer at once, so what is left is each program's own cost;
release build, median of 40, on my PC under load):

| Case | Python | Rust | Saved |
| --- | --- | --- | --- |
| spoken talk (tone, recall, what she knows) | 77.6 ms | 7.5 ms | 70 ms |
| spoken computer question (guides) | 53.2 ms | 5.5 ms | 48 ms |
| typed question (no `--voice`: from a script; the crash watch and the evals use `--voice`) | 55.0 ms | 1.4 ms | 54 ms |
| the voice client, per spoken sentence | 24.3 ms | 0.6 ms | 24 ms |

So a spoken question with a three-sentence answer saves about 50-70 ms before she starts and about 24 ms per sentence spoken
through Chatterbox. Measured again on 2026-11-11 with CPU, memory and more cases (`tests/bench_port.py`): 47-73 ms per
question, 24 ms per sentence; the tables are in [packaging/voice/README.md](../packaging/voice/README.md#why-rust-measured).

`sarina` is still started as often as before (route, vault-search, chat; system-status, daily-report and crash-detail with
`SARINA_SYSTEM=on`), because each start gives an answer the next step depends on, and merging them would change answers:
the guides lookup and the recall lookup run `vault-search` with different environments (the recall one with the merged
`sarina.env` and `SARINA_SEMANTIC=on`), the route decides which one runs, and running embedding requests side by side could change
how Ollama batches them on the card. The one change in order: `sarina-tone --pattern` (it reads its own numbers, no model, no
card) now runs while the vault is searched. It takes about 2 ms, so that is what it saves.

## Not quite 1:1

- **A crash** (an error the Python never caught, exit 1) prints one line `Traceback: <what>` instead of Python's traceback.
  In two rare cases it comes at a different moment: `sarina-tone` printing bytes that are not UTF-8 (noticed after the vault
  lookups, since it now runs alongside), and Heimdall sending fields of the wrong type.
- **`-h`** is argparse's layout for an 80-column terminal; Python 3.14 wraps to the terminal's width and colours help and errors
  on a terminal.
- **The environment given to programs** has the same variables and values, in sorted order (Python kept os.environ's order).
- **HTTP** (Ollama's embeddings, Heimdall) is plain `http://` to the address given: no proxy from `http_proxy`, no redirects.
- **`sarina.env`** is read once per run (Python read it again at each use).
- **Text files** are always read as UTF-8; Python used the locale's encoding (the same on a UTF-8 or C locale).
- **Rare text**: a lone surrogate in a JSON file becomes U+FFFD; a typed question with bytes that are not UTF-8 gets U+FFFD where
  Python kept the bytes (the voice client keeps them exactly); a vault path containing `*`, `?` or `[` is not treated as a
  pattern when the joke logs are listed.
- **A closed output pipe** ends it with status 1, without Python's BrokenPipeError message.
- **The check-up's override table** (a unit Labyrinth copied and changed) is empty in the Python too; its diff code is not ported.
- One Labyrinth test that never ran (`test_list_numbers_are_not_sentences`) fails on the Python original: a streamed " 1. "
  counts as a finished sentence. The port keeps the behaviour; the carried-over test pins it.

## The switch (branch `voice-rust-switch` in Labyrinth_OS)

Nothing I use changes until I switch. On that branch `sarina-git` also builds and installs `/usr/bin/sarina-voice`, and the
image's three files in `/usr/local/bin` are links to it. On my PC (`sarina-git` builds from the local Sarina checkout's `main`,
so that has to be pulled first):

```
cd ~/Documents/Github/Sarina && git switch main && git pull --ff-only
cd ~/Documents/Github/Labyrinth_OS && git fetch && git switch voice-rust-switch
cd packages/pkgbuilds/sarina-git && makepkg -sfi
sudo mkdir -p /usr/local/lib/labyrinth/python-voice
for p in labyrinth-ask-sarina labyrinth-sarina-checkup labyrinth-sarina-speak-voiced; do
  sudo cp -a /usr/local/bin/$p /usr/local/lib/labyrinth/python-voice/$p
  sudo ln -sfn /usr/bin/sarina-voice /usr/local/bin/$p
done
```

Back to Python:

```
for p in labyrinth-ask-sarina labyrinth-sarina-checkup labyrinth-sarina-speak-voiced; do
  sudo rm -f /usr/local/bin/$p
  sudo install -m755 /usr/local/lib/labyrinth/python-voice/$p /usr/local/bin/$p
done
cd ~/Documents/Github/Labyrinth_OS && git switch main
```

(`/usr/bin/sarina-voice` can stay; nothing uses it once the links are gone. `makepkg -sfi` on main builds sarina-git without it.)

What I test after switching:

1. Super+S: a question about the computer ("why is the fan loud"), small talk, a joke and then "another one", a laugh at a joke
   in the same conversation (it should land in `Entities/Inside jokes.md`). The heard line, the answer on the card and her
   voice should be as before, a little sooner. With `LABYRINTH_VOICE_TIMING=1` in `sarina.env`, the `route=` and `prefetch=`
   numbers in `$XDG_RUNTIME_DIR/labyrinth-sarina/timing.log` should drop by about 0.05.
2. A crash notice's "Ask Sarina" (crash a throwaway program, for example `sh -c 'kill -SEGV $$'`): her answer appears.
3. Settings > Assistant: switch "Notice how I sound" off and on and ask once each way; pick a voice and press "Hear her voice"
   (that sentence goes through the voice client); switch the daily check-up on and run it once:
   `systemctl --user start labyrinth-sarina-checkup.service`, then today's note in `Daily/` and
   `journalctl --user -u labyrinth-sarina-checkup -u labyrinth-sarina-held` (no "Traceback").

## After the switch (2026-11-11)

The switch is in Labyrinth_OS main (4ecd342, branch deleted), so the Rust programs are the programs; the Python originals stay
in Labyrinth_OS's history. The side-by-side tests (`packaging/voice/tests`) skip unless `LABYRINTH_OS_DIR` names a checkout from
before the switch (`git worktree add DIR 1c905fe`).

Found while measuring: on my PC `~/.local/bin` comes first in the desktop's PATH, and it still held a Python
`labyrinth-ask-sarina` (and a dev `sarina`) from 2026-11-10. So Super+S ran those, not `/usr/bin/sarina-voice`: the voice test
of 2026-11-11 tried the old ask program. They are to move to a dated backup folder (Labyrinth_OS docs/SARINA_VOICE.md, "The Rust programs").

The first changes that make the Rust differ from the originals on purpose (each Python original changed the same way, the 72
side-by-side tests pass again, except the one that checks the old wording):

- **Short spoken answers.** The spoken brief asks for one or two short sentences (about 30 words): the answer itself or the
  one thing to do, details only when asked. It was "at most three short sentences, about 60 words".
- **No hidden thinking for a spoken question.** `LABYRINTH_VOICE_THINK` now defaults to `off` (it was `computer`). Typed
  questions are as they were. Measured with typed questions and the voice path's `/no_think` (Labyrinth's timing log, median
  of 5, qwen3:8b): "What is the capital of France?" 2.46 s with thinking, 0.66 s without; "Is my system healthy?" (one
  tool) 15.3 s with thinking, 6.8 s without. In October thinking off made companion talk a little worse (A1 in the backlog);
  the answers are short now, and `LABYRINTH_VOICE_THINK=computer` or `on` in `sarina.env` brings it back.

## Not ported yet

- `-warm` runs while I am still talking, and `-setting` is not on the spoken path: no gain. (`labyrinth-sarina-ask` and
  `labyrinth-sarina-speak` are ported in Labyrinth_OS's labyrinth-tools.)
