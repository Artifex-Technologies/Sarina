# sarina-voiced

Sarina's voice service in Rust: the port of Labyrinth OS's `labyrinth-voiced` (Python, 272 lines) and its Chatterbox Turbo
engine `chatterbox_engine.py` (140 lines). Same socket, same requests and answers, same files, same environment variables,
and for the same seed the same speech, bit for bit (measured on the processor, below).

```bash
cd packaging/voiced
cargo build --release                       # target/release/sarina-voiced
cargo build --release --features parity     # also the side-by-side driver, sarina-voiced-parity (never installed)
# from the repository root, on the processor only (the card is never touched), nothing played:
CUDA_VISIBLE_DEVICES= nice -n 19 ~/.local/share/labyrinth/tts/cb-venv/bin/python -I packaging/voiced/tests/parity.py
CUDA_VISIBLE_DEVICES= nice -n 19 ~/.local/share/labyrinth/tts/cb-venv/bin/python -I packaging/voiced/tests/bench.py
target/release/sarina-voiced check "Some text."   # asks the running service (Rust or Python) to speak, plays nothing
```

## Where it lives, and why here

Sarina's repository, next to `packaging/voice` (`sarina-voice`, the client `speak_voiced` and the other spoken-question
programs), because the voice belongs to Sarina and not to Labyrinth's image: LykanOS uses Sarina too
(`docs/VOICE_PACKAGE_PLAN.md`). It shares `packaging/voice`'s Python-compatible pieces (JSON as Python writes it,
`str.isspace`, `str.lower`, `os.path.expanduser`) as a path dependency, so both sides of the socket write and read the same
bytes. Labyrinth's `sarina-git` package builds it (branch `voiced-rust-switch`, not merged).

## Design

| Module | What |
| --- | --- |
| `text` | the words as the voice says them: emoji and tags dropped, `[joking]`-style delivery tags, sentences (short ones joined, long ones cut), pronunciations (`~/.config/labyrinth/pronunciations`), the small sounds a delivery adds. Python's `re` semantics by hand: `\s` is `str.isspace` (`\x1c`-`\x1f` too), `\w` is a letter, number or `_`, `re.IGNORECASE` is the simple lower case plus Python's `_casefix` table ("ı", "İ" and "i" match; "ſ" and "s"; the Kelvin sign and "k") |
| `npy` | the NumPy 2.5 the sampling depends on: `default_rng(seed)` (SeedSequence, PCG64, `random()`, `choice(list)`, `choice(n, p=q)` with its checks), NumPy's own float32 `exp` (its AVX2/FMA3 Cody-Waite and rational polynomial, which is not libm's and differs from it in a third of all inputs) and NumPy's pairwise float32 sum |
| `engine` | Chatterbox through ONNX Runtime: `Writer` (token embeddings and the language model with its KV cache, top-k 1000 / top-p 0.95 / temperature sampling, repetition penalty 1.2) and `Decoder` (speech tokens to 24 kHz, NaN refused, peak kept under 0.95, silence at the end cut to 0.2 s); the speech encoder runs once in `set_voice` |
| `wav` | the reference recording read as soundfile reads it (PCM 8/16/24/32, float, any channels, SciPy's `resample_poly` when not 24 kHz); each sentence written as libsndfile writes 16-bit PCM |
| `voice` | one request: the sentences, the retries, the files, the pipeline |
| `server` | the socket and the idle unload; `client`: `sarina-voiced check` |

**ONNX Runtime is loaded at run time** (`ort` 2.0.0-rc.15 with `load-dynamic`): nothing is downloaded or linked at build time.
By default it is the library in the voice venv, `~/.local/share/labyrinth/tts/gpu-venv/lib/python3.*/site-packages/onnxruntime/capi/libonnxruntime.so.*`
(onnxruntime-gpu 1.31.0 ships it next to `libonnxruntime_providers_cuda.so`), so the Rust service runs the very build the
Python used, and the CUDA and cuDNN libraries of the venv's NVIDIA packages are loaded first in the order `preload_dlls()`
loads them. `LABYRINTH_VOICE_VENV` names another venv, `LABYRINTH_VOICE_ORT` a library file; without a venv it falls back to
the system's `/usr/lib/libonnxruntime.so.1` (`onnxruntime-cpu` 1.29, processor only). The C API asked for is version 17, so any
onnxruntime from 1.17 on loads; leaving out `ort`'s newer API features also keeps it from setting an execution-provider
selection policy, so a session is made exactly like Python's.

**No gaps between sentences: the pipeline.** The Python made each sentence whole (tokens, then decoding) before starting the
next; the caller played sentence N while the service made N+1. Here a writer thread makes the tokens of N+1 while this thread
decodes N, writes its file and sends it, with at most one finished sentence waiting between them. The first sentence is sent
as soon as it is ready, as before. The random draws stay in the Python's order: the writer draws for each sentence in turn,
and when the decoder finds invalid audio (the Python's ValueError, tried up to three times), the writer is sent back to that
sentence with the generator as it stood right after that sentence's tokens, which is where the Python's retry drew from. So
a seed gives the same speech with the pipeline as without it (checked, with forced failures, below).
`LABYRINTH_VOICE_PIPELINE=0` turns it off.

**Same files, same environment:** `$XDG_RUNTIME_DIR/labyrinth-sarina/voiced.sock` (`LABYRINTH_VOICED_SOCKET`), mode 0600 in
a 0700 folder; `voiced-PID-N.wav` beside it; `LABYRINTH_VOICE_DIR`, `_DTYPE` (fp16 by default; the processor fallback uses
fp32), `_IDLE` (600 s), `_REFERENCE`. The decoder stays fp32, the speech encoder runs once in fp32 on the processor when the
fp32 file is there, as in the Python. New, for tests: `LABYRINTH_VOICE_SEED` (one fixed seed for every request).

## Proof: the same speech for the same seed

`tests/parity.py` runs the Python (read from Labyrinth_OS with `git show`) and the Rust side by side, both on the processor,
both on ONNX Runtime 1.31.0's CPU build (the Python through its module, the Rust through the shared library of the same
release), the card never touched (`CUDA_VISIBLE_DEVICES` empty), nothing played.

One thing the Python never controlled: the decoder graph draws noise inside itself (three `RandomNormalLike` and one
`RandomUniformLike` node without a seed), and ONNX Runtime seeds those from the wall clock when the library loads. So two
Python runs with the same seed already give different audio (measured: up to 1.6 in sample value). The check therefore starts
both processes with the wall clock fixed (`tests/fixed_clock.c`, `LD_PRELOAD`, monotonic clocks untouched): then their
decoders draw the same noise. The service itself does not change this; its noise comes from the clock as the Python's did.

RESULTS_TBD

## Why Rust: measured

WHY_TBD

## Not quite 1:1

NOT11_TBD

## Resume here

- Done and pushed: the crate, `tests/parity.py` (all groups the same on the processor), `tests/bench.py`, the switch branch
  `voiced-rust-switch` in Labyrinth_OS (unit + `sarina-git`).
- In progress (2026-11-11 night): the measurements and this README's tables.
