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
next; the caller played sentence N while the service made N+1. Here, once the first sentence is out, a writer thread makes
the tokens of N+1 while this thread decodes N, writes its file and sends it, with at most one finished sentence waiting
between them. The first sentence is made alone and sent as soon as it is ready, as before: on the processor, writing the
second while the first was decoded made the first one 1.7 s later (measured, below), and the first one is what I wait for. The random draws stay in the Python's order: the writer draws for each sentence in turn,
and when the decoder finds invalid audio (the Python's ValueError, tried up to three times), the writer is sent back to that
sentence with the generator as it stood right after that sentence's tokens, which is where the Python's retry drew from. So
a seed gives the same speech with the pipeline as without it (checked, with forced failures, below).
It is on with the card and off on the processor, where it made the sentences later (both halves want every core; measured
below); `LABYRINTH_VOICE_PIPELINE=1` or `=0` decides instead. Whether it helps on the card is the morning's step 3.

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

Results (2026-11-11, Labyrinth_OS `b7cdb5d`, everything the same):

| Group | Cases | Result |
| --- | --- | --- |
| NumPy's float32 `exp` | every float32 from -0 to -104 (1,120,927,744 values, the range sampling uses), plus 4 M random ones | bit for bit |
| NumPy's float32 sum (pairwise) | 912 arrays of 1 to 20,000 values | bit for bit |
| `default_rng`: `random()`, `integers`, `choice(list)`, `choice(n, p=q)`, and the engine's `sample()` on model-shaped logits at every delivery's temperature | 2,400 draws, 8 seeds | the same |
| text: tags, cleaning, pronunciations, sentences, delivery, `dress()` with its draws | 1,500 generated texts each (tags in any case, markup, emoji, "İ", "ı", "ſ", the Kelvin sign, `\x1c`-`\x1f`, no-break spaces, «marks», 300-character words) | the same, except 8 texts where the Python never returns (below) |
| tokenizer ids (Rust `tokenizers` 0.23.2 against Python's 0.23.3) | 400 cleaned texts with and without `[chuckle]` | the same |
| pronunciation files (comments, CRLF, `=` in the spelling, empty sides, a byte that is not UTF-8) | 4 | the same |
| reference recording: PCM 16/24/32/8-bit, float, double, 1 to 3 channels | 7 | bit for bit at 24 kHz; resampled (8, 16, 22.05, 44.1, 48 kHz): within 2.4e-7 (SciPy works in double precision in its own order) |
| sentence files (16-bit PCM, libsndfile's conversion) | 786,780 samples, including every 16-bit level and its neighbours | byte for byte |
| the engine: 8 sentences, seeds and temperatures of every delivery | 59 to 101 tokens each | **tokens the same, float samples bit for bit, wav files byte for byte** |
| the same with a sentence ending in `[chuckle]` | 1 | both fail with the same ONNX Runtime error (below) |
| whole requests through the service (four sentences, every delivery tag), in turn and pipelined | 4 seeds, with decodes forced to fail at chosen (sentence, attempt) pairs: retried twice, and failed for good | every message, file and error the same |

The engine's numbers are only reachable because the sampling repeats NumPy's arithmetic exactly; libm's `expf` instead of
NumPy's differs in a third of all inputs. One known theoretical difference remains: NumPy's `argsort` is not stable, the
Rust's is, so if two tokens had exactly the same probability right at the top-p cut, they could be kept differently. It did
not happen in any case here.

## Why Rust: measured

Measured on 2026-11-11 on my desktop (16 threads, 64 GB), at night with my session idle, by the method of Yggdrasil's
`docs/PORT_NUMBERS.md` (`tests/bench.py`): the Python from git (Labyrinth_OS `b7cdb5d`, the files as the image has them),
the Rust release build, the same inputs, two warm-up runs, then alternating runs (old, new, new, old ...), 5 of each,
everything at `nice -n 19`, one process at a time. **On the processor only**, in full precision (fp32), with ONNX Runtime
1.31.0's CPU build for both: the card was nearly full (the language model and the running voice service) and was not to be
touched, so the card's numbers are still to come (the morning check below measures the two services there). Medians, with
the 10th to 90th percentile in brackets; change is (new − old) / old, so a minus is a gain.

| Case | Python | Rust | Change |
| --- | --- | --- | --- |
| Six sentences (2.5 to 4.2 s of speech each), the sum of each sentence's median | 20.90 s | 20.73 s | −0.8 %, 0.17 s sooner |
| per sentence: 4013, 3054, 3144, 3889, 3099, 3703 ms against | | 3992, 3037, 3124, 3848, 3070, 3661 ms | −0.5 to −1.1 % each |
| Loading (graphs, tokenizer, the reference recording's conditioning) | 5.82 s (5.79-5.85) | 5.87 s (5.81-5.93) | **+0.9 %, 0.05 s later** |
| Peak memory of a process that loads the voice and makes the six | 2,480 MB | 2,206 MB | −11 %, 274 MB less |
| The service, a five-sentence answer (about 16 s of speech), voice loaded: time to the first sentence | 4.50 s (4.39-6.32) | 4.30 s (4.19-5.60) | −4 %, 0.20 s sooner |
| ... gaps a listener would have heard between the sentences (the processor makes speech slower than it is spoken) | 1.51 s (1.39-4.45) | 1.49 s (1.42-3.98) | −1 %, the same |
| ... the end of the last sentence | 22.45 s (22.20-27.27) | 22.28 s (22.03-25.16) | −0.8 %, 0.17 s sooner |
| ... memory with the voice loaded (RSS) | 2,274 MB | 1,676 MB | −26 %, 598 MB less |
| The service with nothing loaded, 60 s alone: CPU, wake-ups | 0 ms, 2 | 0 ms, 2 | the same (both look every 30 s whether to unload) |
| ... its memory (RSS / PSS) | 13.3 / 6.6 MB | 4.2 / 2.0 MB | −68 % / −70 %, 9 MB / 4.6 MB less |
| On disk | 19 KB of scripts, plus Python and numpy, SciPy, soundfile, tokenizers in the venv (225 MB) | 4.2 MB binary | the venv keeps only onnxruntime and the NVIDIA libraries, which both need |

What this says:

- **Speed: about the same, as expected.** Nearly all of a sentence's time is ONNX Runtime running the graphs, the same
  library in both; what Python spent around it (the sampling loop, NumPy, copying the KV cache out and back in at every one of
  the ~80 steps; the Rust passes the cache from one step to the next without copying) is under 1 % of it. The port is not
  where the voice gets faster; the card is.
- **Memory: less.** 274 MB less at the peak, and less held while loaded (above); 9 MB less while waiting with nothing loaded.
- **Worse in Rust:** loading takes 0.05 s longer (within the noise of two runs' spread, but it is the median); and the binary
  is 4.2 MB against 19 KB of scripts (the scripts' interpreter and libraries are much larger, but stay installed for other
  things until the venv is cut down).
- **Safer:** a bounded request line, no interpreter importing modules at run time (it still loads ONNX Runtime and the CUDA
  libraries from the venv, as the Python did), and three hangs and lockups of the Python fixed ("Not quite 1:1").

**The pipeline on the processor: worse, so it is off there.** Same answer, same runs (Rust only; the service table above is
the 7-run round with the defaults, in turn on the processor):

| Rust, the five-sentence answer | first sentence | gaps | last sentence ends |
| --- | --- | --- | --- |
| in turn (as the Python; the default on the processor) | 4.30 s | 1.49 s | 22.28 s |
| pipelined, the second sentence written while the first is decoded (5 runs, an earlier round) | 6.28 s | 0.78 s | 23.49 s |
| pipelined after the first sentence (the default with the card) | 4.23 s | 3.17 s | 23.82 s |

Both halves use every core (ONNX Runtime's thread pools), so running them side by side slows each: the overlap took the
gaps down only by making the first sentence 2 s later, and once the first sentence goes alone, the overlap only makes the
rest later. On the card the language model's many small steps (one per speech token) leave it idle much of the time, which is where the
overlap should pay; that is the morning's step 3, and if it does not, `LABYRINTH_VOICE_PIPELINE=0` (or a change of the
default) is all it takes.

Noise: another helper's tests ran on the computer during part of the service rounds (one core at 100 %, nice 15); the
alternating order spreads that over all three, and it shows in the 90th percentiles. The engine round was quiet.

## Not quite 1:1

Kept on purpose (the Python's behaviour, so nothing Sarina says changes):

- **A joke's chuckle at the end of a sentence fails on the processor.** `dress()` appends ` [chuckle]` to a joking sentence
  six times in ten. The embedding graph (`embed_tokens.onnx`) embeds the last two input ids as *speech* tokens (it maps the
  final `<|endoftext|>` to START), so `[chuckle]` (text id 50274) indexes past the 6,563-row speech table. On the processor
  ONNX Runtime refuses it (`INVALID_ARGUMENT ... idx=50274`), the request ends with that error and the rest of the answer is
  not said; both versions do exactly this (checked). On the card the CUDA Gather does not check indices as far as I know, so
  it probably reads a row outside the table and the chuckle sounds like whatever is there. Not changed here: it changes what
  Sarina says, so it is my decision. A fix would be to put the chuckle before the sentence's last word or punctuation, or to
  drop it.
- The decoder's noise comes from the clock (above), as before.

Better than the Python (a bug the port fixes; none changes what is said):

- **A long part without a comma or a space hung the service.** `split_sentences` cuts a sentence over 240 characters at the
  last comma (after the 40th character) or space; with neither, the Python's loop never ended, holding the voice's lock, so
  no request was answered again until the service was restarted (8 of the generated texts in `tests/parity.py` do this). The
  Rust cuts at 240 characters.
- **A failed load in the background kept the voice locked.** `warm` took the lock and loaded; when loading raised, the lock
  was never released and every later request waited for ever. The Rust releases it and logs why.
- **A voice whose reference recording failed to load stayed half loaded.** The Python kept the engine without a voice, so
  every later request failed with "set_voice first" until the idle unload. The Rust keeps only a voice that loaded completely,
  so the next request tries again.
- **No CUDA provider means the processor in full precision.** When the card cannot be used, the Python fell back to fp32 on
  the processor, but a missing CUDA provider (onnxruntime without CUDA) only gave a warning and ran the fp16 files on the
  processor. The Rust asks for the CUDA provider strictly, so it takes the intended fallback.
- **The unused half-precision speech encoder is never loaded.** The Python loaded it on the card at every load and dropped it
  unused (the full-precision one on the processor does the work): about 0.5 GB of card memory for a moment, and load time.
- A request line is read up to 4 MiB (the Python had no limit).

Different in rare cases:

- A lone surrogate in the request's text (`\udcXX`, what the client sends for a byte that is not UTF-8) becomes U+FFFD; in
  the Python the tokenizer refused the sentence three times and the request ended with an error.
- A backslash in a pronunciation's spelling: the common escapes (`\n`, `\t`, `\\` ...) are read as `re.sub` reads them,
  any other is kept as written (Python's `re.sub` raised "bad escape" and the request failed).
- A request that is not a JSON object: nothing is answered and the connection is closed, as before, plus one log line
  (the Python printed a traceback).
- Python's ONNX Runtime error texts are kept word for word (`[ONNXRuntimeError] : 2 : INVALID_ARGUMENT : ...`); other
  errors (a missing file, the reference recording) are worded differently.
- The socket's listen backlog is the standard library's (128), not 4.

## The switch, the check on the card, and the way back

On Labyrinth_OS's branch `voiced-rust-switch` (pushed, not merged): `sarina-git` also builds and installs
`/usr/bin/sarina-voiced`, and the image's `labyrinth-voiced.service` starts it; the Python files stay in the image for a
rollback. Nothing on my PC changes until I do the steps below.

**What the card needs:** nothing new. The Rust service loads the voice venv's own `libonnxruntime.so.1.31.0` (onnxruntime-gpu
1.31.0; `libonnxruntime_providers_cuda.so` and `_shared.so` beside it) and, before the first session, the venv's CUDA 13 and
cuDNN 9 libraries from `site-packages/nvidia/` (`cu13/lib`, `cudnn/lib`), as `onnxruntime.preload_dlls()` did. Checked
without the card (`sarina-voiced-parity gpuprobe`, which only runs with `CUDA_VISIBLE_DEVICES` empty): the provider library,
CUDA and the rest load, and the CUDA provider stops where it needs a device ("no CUDA-capable device is detected"). What was
not checked tonight: a session on the card (the card was nearly full and I was asleep), so its speed and memory there.

**Check it on the card (no logout; works over the stream; plays nothing):** a user drop-in, so no root and no package:

```bash
cd ~/Documents/Github/Sarina && git pull --ff-only
cd packaging/voiced && cargo build --release --locked
./target/release/sarina-voiced check            # 1) the Python service as it runs now: times on the card, nothing played
install -Dm755 target/release/sarina-voiced ~/.local/lib/sarina-voiced/sarina-voiced
mkdir -p ~/.config/systemd/user/labyrinth-voiced.service.d
printf '[Service]\nExecStart=\nExecStart=%%h/.local/lib/sarina-voiced/sarina-voiced\n' > ~/.config/systemd/user/labyrinth-voiced.service.d/rust.conf
systemctl --user daemon-reload && systemctl --user restart labyrinth-voiced
./target/release/sarina-voiced check            # 2) the Rust service: same text, compare the times
journalctl --user -u labyrinth-voiced -n 5      # "voice ready in N s (CUDAExecutionProvider,CPUExecutionProvider)"
nvidia-smi --query-compute-apps=pid,process_name,used_memory --format=csv
# 3) the pipeline off, for comparison on the card (it is on with the card by default):
printf '[Service]\nEnvironment=LABYRINTH_VOICE_PIPELINE=0\n' > ~/.config/systemd/user/labyrinth-voiced.service.d/pipeline.conf
systemctl --user daemon-reload && systemctl --user restart labyrinth-voiced
./target/release/sarina-voiced check; ./target/release/sarina-voiced check
rm ~/.config/systemd/user/labyrinth-voiced.service.d/pipeline.conf && systemctl --user daemon-reload && systemctl --user restart labyrinth-voiced
```

Run each `check` twice and take the second (the first one loads the voice). Pipelined against in turn on the card decides
the default: on the processor the pipeline made sentences later (measured below), so it is on only with the card, where the
language model's small steps leave the card idle much of the time; whether that turns into earlier sentences is what step 3
shows.

`check` asks the service to say three sentences, deletes each file unplayed, and prints when each arrived, the time to the
first sentence and the gaps a listener would have heard. If the journal says "graphics card not usable", the card path failed
(the message says why) and the service runs on the processor: undo, or keep it while I look. To hear it: Super+S, or
`labyrinth-sarina-speak "..."`, when I am at the speakers.

**Undo** (back to the Python at once):

```bash
rm -f ~/.config/systemd/user/labyrinth-voiced.service.d/rust.conf ~/.config/systemd/user/labyrinth-voiced.service.d/pipeline.conf
systemctl --user daemon-reload && systemctl --user restart labyrinth-voiced
```

After a week without trouble: merge `voiced-rust-switch`, rebuild `sarina-git`, remove the drop-in and
`~/.local/lib/sarina-voiced`. Later, once the Python is gone, the venv only needs onnxruntime and the NVIDIA packages:
numpy, SciPy, soundfile and tokenizers (about 225 MB of it) are the Python's alone.

## Resume here

Status 2026-11-11, end of the night: done on the processor, not yet run on the card.

- Done and pushed: the crate (clippy clean with `-D warnings`), `tests/parity.py` (every group the same on the processor),
  `tests/bench.py` and its numbers above, `sarina-voiced check`, the switch branch `voiced-rust-switch` in Labyrinth_OS (unit,
  `sarina-git`, `docs/SARINA_VOICE.md`), Sarina's coordination log.
- Next, with me at the PC (or over the stream; nothing needs a logout): the card check above (steps 1-3), then a few days of
  Super+S with the drop-in, then merge the branch. Card numbers go into "Why Rust: measured" (time to first sentence, gaps,
  card memory against the Python's).
- Open questions for me: the chuckle (fix it, or leave it), and the pipeline's default once the card is measured.
- Not done: fp16 on the processor was not compared (the card's dtype; its graphs differ only in the cache's type, which the
  Rust passes through untouched); the 225 MB of the venv only the Python needs stays until the Python is removed.
