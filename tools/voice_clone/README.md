# Voice cloning: a custom Sarina TTS voice

Produces one artifact: a fine-tuned Piper voice model (`.onnx` +
`.onnx.json`) that sounds like the reference recording(s) in `samples/`.
That file is then handed to `sarina voice-chat --tts-model` unmodified — see
`sarina/voice/tts.py`. Nothing here is imported by `sarina/`, and none of it
ships as part of the installed package or the Arch build.

**Privacy**: a voice recording and a voice-clone model are both personal
biometric-like data. `samples/`, `corpus/`, `checkpoints/`, and any
`*.onnx`/`*.onnx.json` in this directory are gitignored by default (see the
repo's `.gitignore`). Don't commit them unless you've decided to on purpose.

## How it works

1. [OpenVoice](https://github.com/myshell-ai/OpenVoice) is *not* a
   standalone TTS engine — it's a tone-color conversion step applied to
   another engine's audio. We use it exactly that way: Piper's own stock
   voice generates the base audio for a set of training sentences, and
   OpenVoice reshapes each clip's tone color toward the voice(s) in
   `samples/`.
2. That synthetic (text, cloned-voice-audio) corpus becomes the training
   data for fine-tuning a brand-new Piper voice checkpoint, starting from an
   existing medium-quality Piper checkpoint (much faster than training from
   scratch).
3. The finished checkpoint is exported to Piper's runtime `.onnx` format —
   the same format every other Piper voice uses, so nothing in `sarina/`
   needs to change.

## 1. Set up the environment

Use a dedicated environment (conda or venv) — this pulls in PyTorch and is
not meant to coexist with Sarina's own lightweight `voice` extra.

```bash
conda create -n sarina-voice-clone python=3.9
conda activate sarina-voice-clone

# OpenVoice: git clone + editable install (no PyPI package). Follow their
# own instructions in full -- don't hand-pick a subset of dependencies, see
# requirements.txt for why.
git clone https://github.com/myshell-ai/OpenVoice.git
cd OpenVoice && pip install -e . && cd ..

# OpenVoice V2 checkpoint (converter only -- we don't need their base
# speaker TTS or MeloTTS, since Piper plays that role here). The URL in
# OpenVoice's own README (an S3 bucket) is dead; Hugging Face has it instead:
mkdir -p checkpoints/checkpoints_v2/converter
curl -L -o checkpoints/checkpoints_v2/converter/config.json \
  https://huggingface.co/myshell-ai/OpenVoiceV2/resolve/main/converter/config.json
curl -L -o checkpoints/checkpoints_v2/converter/checkpoint.pth \
  https://huggingface.co/myshell-ai/OpenVoiceV2/resolve/main/converter/checkpoint.pth

# Piper fine-tuning support, on top of OpenVoice's install:
pip install -r requirements.txt
```

Download an existing medium-quality Piper training checkpoint to fine-tune
from, e.g. from `rhasspy/piper-checkpoints` on Hugging Face
(`https://huggingface.co/datasets/rhasspy/piper-checkpoints/tree/main/en/en_US/amy/medium`
has one paired with the Amy voice used above) into `checkpoints/piper/`.

**Windows only -- three real compatibility gaps, verified end-to-end this
session, none of them about the reference audio or the checkpoint's actual
quality:**

- These checkpoints predate PyTorch 2.6's stricter `torch.load` defaults and
  were saved on Linux, so loading one raises either an unpickling error or
  (once that's allowed) `cannot instantiate 'PosixPath' on your system`.
  `train.py`/`export.py` in this directory are drop-in replacements for
  `python -m piper.train`/`python -m piper.train.export_onnx` that handle
  this — use them instead of invoking the modules directly (their
  docstrings explain why). The checkpoint's hyperparameters also predate
  piper1-gpl's current CLI schema and contain keys it no longer recognizes
  (e.g. `sample_bytes`), which crashes the same way with `Subcommand 'fit'
  does not accept option 'model.sample_bytes'` -- run `clean_checkpoint.py`
  on a freshly downloaded checkpoint once, before fine-tuning, to strip
  those.
- `piper-tts[train]`'s wheel is missing the compiled `monotonic_align`
  Cython extension (a core VITS component) entirely -- training crashes on
  the first batch with `ModuleNotFoundError:
  piper.train.vits.monotonic_align.monotonic_align`. Fetch
  `core.pyx` from the same version's GitHub source
  (`src/piper/train/vits/monotonic_align/core.pyx` in OHF-Voice/piper1-gpl)
  into that directory in your `site-packages`, then from `site-packages`
  itself run `python piper/train/vits/monotonic_align/setup.py build_ext
  --inplace` (must run from `site-packages`, not from inside that
  directory, or the built `.pyd` lands in the wrong place) — and because the
  package's own `__init__.py` imports `from .monotonic_align.core import
  ...` (an inherited quirk from the original VITS repo, not a typo), create
  one more nested `monotonic_align/monotonic_align/` folder with an empty
  `__init__.py` and copy the built `.pyd` there too.
- `export.py` fails with `GuardOnDataDependentSymNode` under PyTorch 2.6+'s
  newer default ONNX exporter, which can't handle this model's
  data-dependent control flow (the duration predictor's rational quadratic
  spline). Add `dynamo=False` to the `torch.onnx.export(...)` call in your
  installed `piper/train/export_onnx.py` to force the older, compatible
  exporter — a real incompatibility between the installed package and
  current PyTorch, not anything wrong with your checkpoint.

## 2. Reference samples

Drop one or more clean recordings of the target voice into `samples/`.
Multiple samples are averaged for a more robust clone (both for OpenVoice's
embedding, and directly as real training audio via `transcribe_samples.py`
below) — more/cleaner audio helps, and unlike a fixed-vocabulary sentence
list, real recordings covering a wider range of topics also broaden the
vocabulary the fine-tuned voice can say naturally, not just its identity.

**If a recording has background music (even faint)**, isolate the vocals
first — a VITS-style model learns to reproduce whatever's actually in its
training audio, not just the voice in it, so faint music becomes part of
what the trained voice "sounds like", and it can also throw off
`transcribe_samples.py`'s Whisper transcription accuracy:

```bash
pip install demucs   # Meta, MIT-licensed, ~300MB model on first run
python isolate_vocals.py path/to/noisy_recording.mp3 --output samples/Sample06.wav
```

**Transcribing the samples** (`transcribe_samples.py`, used by step 3 below when
you have enough real speech) defaults to faster-whisper's `medium` model on
GPU -- a real accuracy step up from `small`, including fewer cases of a
laugh/giggle getting mistranscribed as an actual word. It also drops any
trailing word faster-whisper wasn't confident about (`MIN_WORD_PROBABILITY`)
from both the label text and the clip's audio, and never pads a clip's
trailing edge into *any* raw segment Whisper detected next -- including ones
filtered out as unreliable (a laugh, a cough) -- not just the next kept one.
Both were real bugs: a giggle right after a sentence, even though correctly
excluded as its own training example, could still get a few hundred ms of
itself baked onto the end of the *previous*, otherwise-clean clip purely from
the fixed trailing pad, teaching the model that sentences sometimes end with
a trailing vocal burst independent of the text. If CUDA fails with "Library
cublas64_12.dll is not found" on Windows even though `torch.cuda.is_available()`
is True, that's a pip-installed torch's bundled DLLs not being on the search
path CTranslate2 checks -- `transcribe_samples.py` registers them itself now,
no action needed.

## 3. Build the training corpus

```bash
python build_corpus.py \
  --piper-model /path/to/en_US-amy-medium.onnx \
  --converter-config checkpoints/checkpoints_v2/converter/config.json \
  --converter-checkpoint checkpoints/checkpoints_v2/converter/checkpoint.pth
```

Pick a base voice whose gender roughly matches the reference samples --
tone conversion has less distance to cover, and the result tends to sound
more natural. `en_US-amy-medium` (female) is a reasonable default; swap in
whatever's closer if the reference voice differs.

`--samples` defaults to every file in `samples/`; `--sentences` defaults to
the included `sentences.txt` (the Harvard Sentences, phonetically diverse).
Writes `corpus/wavs/*.wav` + `corpus/metadata.csv`.

## 4. Fine-tune (manual, iterative — not automated here)

Clean the downloaded checkpoint once first (see the Windows note above —
harmless to run even where it isn't strictly needed):

```bash
python clean_checkpoint.py checkpoints/piper/amy-medium-base.ckpt checkpoints/piper/amy-medium-base.cleaned.ckpt
```

```bash
python train.py fit \
  --data.voice_name "sarina" \
  --data.csv_path corpus/metadata.csv \
  --data.audio_dir corpus/wavs/ \
  --model.sample_rate 22050 \
  --data.espeak_voice "en-us" \
  --data.cache_dir cache/ \
  --data.config_path cache/config.json \
  --data.batch_size 8 \
  --ckpt_path checkpoints/piper/amy-medium-base.cleaned.ckpt \
  --trainer.max_steps 3000 \
  --trainer.accelerator gpu --trainer.devices 1 \
  --trainer.default_root_dir logs/
```

GPU strongly recommended (verified working on an RTX 2080 Ti, 11GB VRAM;
community reports suggest fine-tunes are viable from as little as 8GB).
`--trainer.max_steps` is a starting point, not a target to hit exactly --
checkpoints land under `logs/lightning_logs/version_N/checkpoints/`
(several tracked by different validation metrics, plus `last.ckpt`); export
a few and listen rather than assuming the highest logged score sounds best,
especially with a small sentence corpus. Increase
`--trainer.max_steps` and resume from `last.ckpt` if it needs more.

## 5. Export and use it

```bash
python export.py \
  --checkpoint logs/lightning_logs/version_0/checkpoints/last.ckpt \
  --output-file exported/sarina-voice.onnx
cp cache/config.json exported/sarina-voice.onnx.json

sarina voice-chat \
  --stt-model-dir /path/to/faster-whisper-model \
  --tts-model exported/sarina-voice.onnx \
  --tts-config exported/sarina-voice.onnx.json
```
