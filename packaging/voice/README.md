# sarina-voice

Sarina's spoken-question programs in Rust, one binary that answers to the name it is started as:

| Program | What it is | Was |
| --- | --- | --- |
| `labyrinth-ask-sarina` | the conversation (Super+S, the crash watch's "Ask Sarina", the evals) | Python, Labyrinth_OS |
| `labyrinth-sarina-checkup` | the daily check-up and `--held` (what Heimdall holds) | Python, Labyrinth_OS |
| `labyrinth-sarina-speak-voiced` | the client of the voice service, once per spoken sentence | Python, Labyrinth_OS |

The command lines, input and output, files, environment and exit status are the Python ones. How it was kept the same, what
is not quite 1:1 and how to switch: [docs/VOICE_RUST_PORT.md](../../docs/VOICE_RUST_PORT.md).

```bash
cd packaging/voice
cargo build --release --locked --offline                       # target/release/sarina-voice
python3 -m unittest discover -s packaging/voice/tests          # from the repository root: Python and Rust side by side
```

## Why Rust: measured

Measured on 2026-11-11 on my desktop (16 threads, 64 GB), Python 3.14.9, Rust 1.99.1. Old: the three Python programs as
Labyrinth_OS had them at `ea449d9` (the last change before the port). New: the release build of Sarina `8d2ec36`. Change is
(old − new) / old, so a minus is a gain. Times are the median of 30 runs each, p10-p90 in brackets. Everything Sarina's
programs start (sarina, the model, the voice service, Heimdall) is a stand-in that answers at once, so what is left is each
program's own cost. Method and script: [How these numbers were measured](#how-these-numbers-were-measured).

| Program and case | Python | Rust | Change | Peak RSS, Python → Rust |
| --- | --- | --- | --- | --- |
| ask-sarina: spoken talk (tone, recall, what Sarina knows), a new question | 80.3 ms (79.2-84.2) | 7.11 ms (6.95-7.38) | −91 %, 73.2 ms sooner | 30.2 → 6.3 MB (−79 %) |
| ask-sarina: spoken computer question (guides), a new question | 52.3 ms (51.3-55.5) | 4.46 ms (4.32-4.65) | −91 %, 47.8 ms sooner | 18.9 → 5.7 MB (−70 %) |
| ask-sarina: the same spoken question again (route cached) | 53.1 ms (51.3-55.5) | 4.16 ms (4.00-4.43) | −92 %, 49.0 ms sooner | 18.9 → 5.7 MB (−70 %) |
| ask-sarina: typed question (no `--voice`) | 47.6 ms (47.2-49.1) | 0.98 ms (0.88-1.07) | −98 %, 46.6 ms sooner | 18.6 → 4.1 MB (−78 %) |
| check-up: all fine | 56.2 ms (55.7-57.0) | 2.90 ms (2.71-3.21) | −95 %, 53.3 ms sooner | 26.3 → 4.6 MB (−82 %) |
| check-up: crashes, a failed unit, a full disk, a known hole; Sarina asked, noted, notified | 56.7 ms (56.0-57.4) | 3.55 ms (3.23-3.73) | −94 %, 53.2 ms sooner | 26.3 → 4.7 MB (−82 %) |
| check-up `--held`: a held program asked back | 54.4 ms (53.7-55.1) | 2.08 ms (1.84-2.24) | −96 %, 52.3 ms sooner | 26.1 → 3.9 MB (−85 %) |
| voice client: one sentence (loudness read, played) | 25.7 ms (25.6-26.1) | 1.07 ms (0.99-1.15) | −96 %, 24.7 ms sooner | 13.7 → 3.8 MB (−72 %) |
| voice client: the service has nothing to play | 23.9 ms (23.5-24.1) | 0.67 ms (0.63-0.74) | −97 %, 23.3 ms sooner | 13.3 → 3.5 MB (−74 %) |
| **on disk** | 74 KB of scripts, plus Python | 2.3 MB for all three | **+2.3 MB** | |

- So a spoken question starts 47-73 ms sooner, and each sentence Sarina speaks through the voice service another 24 ms.
  The first measurement (2026-11-10, `tests/bench_voice.py`, under load) found 48-70 ms per question and 24 ms per sentence.
- CPU follows the time: 24-80 ms of CPU per Python run, 0.5-7.8 ms per Rust run.
- Python's own start (`python3 -c pass`) is 9.3 ms and 9.9 MB of each run; the rest is importing modules and the work.
- Every case printed the same output in both.
- **Worse in Rust: the size on disk.** 2.3 MB against 74 KB of scripts. The scripts need Python, which stays installed for
  the voice service (`labyrinth-voiced`, Chatterbox) either way.

More accurate:

- 72 side-by-side tests, Python and Rust byte for byte the same: the Labyrinth tests of these programs carried over, 26
  whole runs of ask-sarina, 7 of the check-up, 4 of the voice client, 700 generated texts and 300 replies fed in a character
  at a time. A changed word in the spoken-answer rule makes 21 of them fail, so they do compare.
- Found by the port: ten Labyrinth tests of ask-sarina sat below `unittest.main()` and never ran. They run now, against both.
- Found by the port, not changed: one of those ten (`test_list_numbers_are_not_sentences`) fails on the Python too, since a
  streamed " 1. " counts as a finished sentence. The port keeps the Python's behaviour and the test pins it.
- The port fixed no bug in the Python: it keeps the Python's behaviour on purpose, so nothing Sarina says changes.

### How these numbers were measured

```bash
cd packaging/voice && cargo build --release --locked --offline && cd ../..
python3 -I packaging/voice/tests/bench_port.py --json /tmp/bench.json     # about a minute
```

- `tests/bench_port.py` takes the Python programs from Labyrinth_OS's git history (`git show ea449d9:...`, the checkout next
  to this one or `--labyrinth DIR`), so it works after Labyrinth's copies become links. `--py-commit` picks another.
- Old and new run one after the other, alternating which goes first, 30 times each after one unmeasured run of each, one
  process at a time, at nice 19. The Python programs start through their `#!/usr/bin/env python3` line, as installed; the
  Rust one through links with the programs' names.
- A small C launcher (built by the script with `cc`) starts each program and reports its wall time, user + system CPU and
  peak RSS from `wait4`. Started from Python directly, a program's peak would include Python's own 12 MB: Linux counts the
  memory a process had before `exec`. The floor, `/usr/bin/true`: 0.29 ms and 1.6 MB.
- The programs run in a user, mount and network namespace of the script's own, so the check-up's fixed address for Heimdall
  reaches a stand-in, never the real Heimdall. Stand-ins: one small C program for everything the programs start (sarina,
  sarina-tone, the player, coredumpctl, systemctl, df, journalctl, nvidia-smi, notify-send, labyrinth-ask-sarina; about
  0.3 ms and 1.6 MB each), Heimdall's API, and the voice service on its socket (one sentence's wav, 1.5 s long, or nothing).
  CPU and peak RSS include the stand-ins a program starts (the same calls for both). A "new question" has the route cache
  emptied before each run.
