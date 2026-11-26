# sarina-helpers

Sarina's ML and voice helper in Rust: the port of the Python package `sarina_helpers` (about 1,200 lines). Same modes,
same command lines, same JSON-lines protocol ([PROTOCOL.md](PROTOCOL.md), protocol 1), same answers. Sarina's core starts
it as `/usr/lib/sarina/sarina-helpers` (`SARINA_HELPERS_BIN` names another program).

| Mode | Who calls it | What |
| --- | --- | --- |
| `serve --model-dir D` | `desktop-audit --model-dir`, `chymaera-analyze --model-dir` | the embedding server: `ping`, `embed`, `classify` (desktop categories), `group` (Argus captures) |
| `memory-search --model-dir D [...] <query...>` | `sarina memory-search` | semantic search over saved sessions |
| `voice-chat --stt-model-dir M --tts-model V [...]` | `sarina voice-chat` | push-to-talk voice chat that drives `sarina chat` / `resume` one turn at a time |

```bash
cd packaging/helpers
cargo build --release                       # target/release/sarina-helpers (4.5 MB)
cargo test                                  # the unit tests (no model, no ONNX Runtime needed)
cargo build --features parity               # also sarina-helpers-parity, the side-by-side driver (never installed)
# from the repository root (full history: the Python is read from a pinned commit), on the processor, nothing recorded
# or played, no model downloaded:
python3 -I packaging/helpers/tests/differential.py [--only serve|memory|tts|voice|stt]
```

## Design

| Module | What |
| --- | --- |
| `serve` | the server loop and the four requests; error answers worded as the Python's exceptions were |
| `embed` | the embedding model: `model.onnx` and `tokenizer.json` (Hugging Face's `tokenizers` crate, the library Python's `tokenizers` binds), mean pooling and L2 norm in the Python's arithmetic (NumPy's row-by-row `sum(axis=0)`, Python 3.12's compensated `sum()`), cosine, the stable ranking |
| `classify` | the 13 category prototypes and their fingerprint (the core refuses a helper whose set differs) |
| `memory_search` | session excerpts (first real user ask, last assistant answer, `<system-reminder>` blocks out), search, the output |
| `pyjson` | `json.loads` with CPython's exact error messages and positions, `json.dumps(separators=(',', ':'))`, `str()` of values |
| `argparse` | Python 3.14's argparse for these three parsers: prefixes, `--opt=value`, `--`, negative numbers, `int`/`float` as Python reads them, the messages, `-h`, wrapping to the terminal, colours on a terminal |
| `ortrt` | ONNX Runtime loaded at run time (`ort` 2.0.0-rc.15 `load-dynamic`, as packaging/voiced): `ORT_DYLIB_PATH`, else the system's `libonnxruntime.so.1` (Arch: onnxruntime-cpu); processor only, as the Python's `CPUExecutionProvider` |
| `voice` | voice-chat's loop: the prompts, the agent turns (`chat`, then `resume` with the session id found from the session file that changed), the memory review, Ctrl-C (130) |
| `voice::audio` | `arecord` / `aplay` (alsa-utils), push-to-talk as two Enters, the stop timer |
| `voice::stt` | speech to text: whisper.cpp's `whisper-cli` with a ggml model (below) |
| `voice::piper`, `voice::espeak` | text to speech with a Piper voice: espeak-ng's library (dlopen, nothing linked) for the phonemes, the voice through ONNX Runtime, the helper's sentence splitting and 0.3 s pauses |

Nothing is downloaded and nothing of ONNX Runtime, espeak-ng or whisper.cpp is linked at build time, so it builds the same
on x86_64 and aarch64 (the Raspberry Pi; CI builds and tests both). Dependencies: `ort` and `tokenizers` at the versions
packaging/voiced already uses (its lock file was the start of this one), `sha2`, `unicode-normalization`, `libc`, and
`packaging/voice` for Python's `str.isspace`, `repr(float)` and `repr(str)`.

## Speech to text: whisper.cpp instead of faster-whisper

faster-whisper (CTranslate2) has no Rust equivalent, and its models are another format. On this computer neither
faster-whisper nor a CTranslate2 model exists (the Python voice-chat could not run here); whisper.cpp does
(`whisper-cpp` 1.9.5, `whisper-cli`) with the model Super+S uses, `~/.local/share/labyrinth/stt/ggml-base.en.bin`
(142 MB, already on disk). So the Rust helper runs `whisper-cli -m MODEL -f WAV -nt -np -l auto` (`-ng`, no graphics card,
unless `--stt-device cuda`), as Labyrinth's `sarina_voice` does. `--stt-model-dir` names the ggml file or a folder holding
exactly one `ggml-*.bin`; a CTranslate2 folder is refused with a message that says so. `--stt-compute-type` is accepted and
ignored (a ggml model's quantisation is in the file). whisper.cpp's notes such as `[BLANK_AUDIO]` are not words.
Measured: the sentence "Please open the vault and show me the daily note." spoken by the Rust Piper came back word for
word in 0.9 s on the processor.

## Text to speech: Piper without Python

The Python asked piper-tts 1.8 for each sentence; this does what piper-tts did: espeak-ng's phonemes clause by clause
(language-switch flags dropped, the clause's punctuation kept, decomposed to code points, `[[ raw ]]` blocks, vowel
clusters), piper's ids (start, pad, each known phoneme and a pad, end; an unknown phoneme left out with piper's own
warning on stderr), the voice's scales, the audio scaled to its loudest sample and written as NumPy writes int16.
piper-tts calls `espeak_TextToPhonemesWithTerminator` in its own espeak-ng build; the system's espeak-ng 1.52 has only
`espeak_TextToPhonemes`, so the punctuation that ended each clause is read from the text the clause took (checked: the
same terminator for every one of 3,867 clauses of Sarina's docs), and espeak-ng's linking phoneme between a front vowel
and the next vowel, which piper's build writes as "ʲ" and the system's as nothing, is put back. Phonemes are identical to
piper's for 1,221 of 1,228 texts; the rest differ where the two espeak-ng builds pronounce differently ("has been").

## How it was proved (`tests/differential.py`)

The Python package left the repository on 2026-11-26; the script reads it from the last commit that had it (`PY_REV`).

No sentence-embedding model is on this computer, so the server and the search run on two stand-ins: a BERT-shaped model
made by `tests/make_standin_model.py` (WordPiece tokenizer, the three int64 inputs, MatMul, Tanh, LayerNormalization) and
the Chatterbox voice's token-embedding graph with its GPT-2 tokenizer (already on disk). The Rust build loads the very
ONNX Runtime the Python's onnxruntime package ships, so the answers can be compared byte for byte, then the system's
onnxruntime 1.29 with a tolerance of 1e-6 on vector components and scores (measured: 6.8e-8 at most).

| Part | Cases | Result |
| --- | ---: | --- |
| `serve` (62 request lines per model, both models, the load failures, the command line, nesting) | 264 | byte for byte; system onnxruntime within 6.8e-8 |
| `memory-search` (14 session files of every kind, 30 command lines, `COLUMNS=40`) | 34 | stdout, stderr and status byte for byte |
| text to speech (phonemes of 1,228 texts; six WAVs; a phoneme the voice lacks) | 9 | WAVs byte for byte (noise off: with noise neither side repeats itself) |
| `voice-chat` (32 sessions with stand-ins for arecord, aplay, the recogniser and the agent; the real Piper voice) | 32 | stdout, stderr, status, every agent command line and every WAV played, byte for byte |
| speech to text, Rust only | 1 | whisper.cpp heard the sentence word for word |

## Differences on purpose

- A request line that is not UTF-8 is answered with the Python's UnicodeDecodeError text; the Python died (outside its
  loop). A request line over 64 MiB is answered with an error (the Python read any length).
- JSON nested deeper than 50,000 levels: "maximum recursion depth exceeded while decoding ..."; Python 3.14's limit is its
  C stack (about 52,000 levels) and its message names the stack used.
- A lone UTF-16 surrogate in a request is read as U+FFFD (a Rust string cannot hold one); embedding such a text fails
  with the Python's tokenizer error, as before; an `id` holding one comes back as U+FFFD.
- memory-search skips a session file that is not a regular file (a named pipe held the Python for ever), one that cannot
  be stat'ed (a dangling link ended the Python with a traceback) and one over 256 MiB. Where the Python printed a
  traceback (a model ONNX Runtime cannot load), the Rust prints the exception's text alone (status 1 both).
- voice-chat: speech to text and its model (above); the "missing" message names the Rust helper's pieces
  (`alsa-utils (arecord/aplay), whisper-cpp (whisper-cli), espeak-ng, onnxruntime`).
