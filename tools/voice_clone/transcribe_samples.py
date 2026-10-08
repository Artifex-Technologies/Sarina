"""Build a training corpus directly from real reference-sample speech,
instead of build_corpus.py's synthetic OpenVoice-converted sentences.

build_corpus.py's approach has a ceiling: every training example is Piper's
own stock voice reading a scripted sentence, with OpenVoice only changing
the vocal timbre afterward -- the rhythm, pacing, and pitch contour all
still come from Piper's own synthetic reading. Fine-tuning on that teaches
the model to reproduce Piper's own prosody in a different timbre, no matter
how much of it there is. Given enough real reference speech (which multiple
samples now provide), transcribing it directly and fine-tuning on genuine
(audio, text) pairs carries real prosody through instead. OpenVoice becomes
unnecessary for this: it was only ever a workaround for not having real
transcribed speech from the target voice.

Uses faster-whisper (already a dependency of OpenVoice's own se_extractor in
this environment) to transcribe each sample with segment-level timestamps,
filters out segments too short/long/uncertain to be reliable training
labels, and slices each into its own clip -- writing corpus_real/wavs/*.wav
+ corpus_real/metadata.csv in the same `filename.wav|text` format
build_corpus.py already produces, so it drops into the same `train.py fit`
command unchanged.

Usage:
    python transcribe_samples.py --output-dir corpus_real
"""

from __future__ import annotations

import argparse
import csv
import sys
from pathlib import Path
from typing import Callable, Sequence

_SCRIPT_DIR = Path(__file__).parent

#: Segments outside this range are unreliable training labels: too short to
#: carry much prosody, or long enough that a transcription slip anywhere in
#: it mislabels the whole clip.
MIN_SEGMENT_SECONDS = 1.5
MAX_SEGMENT_SECONDS = 20.0
#: faster-whisper's own per-segment estimate of "this might not be speech at
#: all" (silence, laughter, noise) -- skip anything above this rather than
#: trust a transcription for it.
MAX_NO_SPEECH_PROB = 0.4

#: Whisper's segment timestamps are tuned for transcription accuracy, not
#: clean audio boundaries -- they routinely clip a fraction of a second
#: early, cutting off the natural decay of the last word. Slicing exactly at
#: those timestamps teaches a TTS model that utterances end abruptly, since
#: that's what every training clip would show it. A little padding on each
#: side (clamped to the audio's own bounds and to the neighboring segment,
#: so it never bleeds into adjacent speech) fixes that without touching the
#: transcript or the duration-based filtering above, which still operates on
#: the original, unpadded timestamps.
LEADING_PAD_SECONDS = 0.05
TRAILING_PAD_SECONDS = 0.25

#: faster-whisper's per-*word* confidence. A chuckle or giggle right after
#: real speech sometimes gets transcribed as an actual word (e.g. "laugh")
#: instead of its own low-confidence segment -- the segment-level
#: no_speech_prob check above doesn't catch that, because the segment as a
#: whole is mostly real, confident speech. The per-word probability does
#: catch it: a hallucinated/misheard trailing word is reliably scored lower
#: than genuine speech. Trailing words below this are dropped from both the
#: text and the usable audio span.
MIN_WORD_PROBABILITY = 0.5

#: Reference samples come from different recordings (different interviews,
#: different microphones/contexts) at inconsistent volumes. Without
#: normalizing, the model has to somehow reconcile "quiet clip" vs "loud
#: clip" as if that were expressive prosody rather than just recording
#: variance -- a real, well-established contributor to inconsistent,
#: choppy-sounding TTS output. Every clip is loudness-normalized to this
#: target (RMS-based dBFS, matching pydub's own convention) before export.
TARGET_DBFS = -20.0


def _normalize_loudness(clip, target_dbfs: float = TARGET_DBFS):
    """Adjust `clip`'s gain so its average loudness matches `target_dbfs`.

    `clip` needs only `.dBFS` and `.apply_gain(delta) -> clip` (matching
    pydub.AudioSegment) so this is testable without real audio. Silent clips
    (dBFS == -inf) are returned unchanged rather than blown up to the target.
    """
    if clip.dBFS == float('-inf'):
        return clip
    return clip.apply_gain(target_dbfs - clip.dBFS)


def _padded_bounds(
    start: float, end: float, *, next_start: float | None, audio_duration: float,
) -> tuple[float, float]:
    """Widen [start, end] by the pad amounts above, clamped to the audio's
    own bounds and to `next_start` (if any) so padding never pulls in
    whatever comes next. A previous segment's own trailing pad is
    independently clamped the same way, so any overlap between two adjacent
    clips is at most LEADING_PAD_SECONDS -- negligible.

    Callers must pass the start of the next *raw* Whisper segment here, not
    just the next *usable* (kept) one -- a laugh or noise burst that was
    filtered out as unreliable is still real audio sitting in the gap, and
    padding must not bleed into it just because it didn't become its own
    training example. See `_next_raw_boundary`.
    """
    padded_start = max(0.0, start - LEADING_PAD_SECONDS)
    padded_end = min(audio_duration, end + TRAILING_PAD_SECONDS)
    if next_start is not None:
        padded_end = min(padded_end, next_start)
    return padded_start, padded_end


def _next_raw_boundary(
    end: float, boundary_starts: Sequence[float], next_usable_start: float | None,
) -> float | None:
    """The start of whatever real audio content begins next after `end` --
    or `next_usable_start` if nothing else starts before it.

    `boundary_starts` mixes two kinds of "something real starts here, don't
    pad into it" markers for this sample: every raw Whisper segment's start
    (including ones `_filter_usable_segments` dropped entirely -- a laugh, a
    cough, background noise) and the start of every trailing word trimmed
    off a *kept* segment for low confidence (see MIN_WORD_PROBABILITY) --
    both are real audio the trailing pad must not bleed into, even though
    neither becomes its own training example.
    """
    candidates = [start for start in boundary_starts if start >= end]
    if next_usable_start is not None:
        candidates.append(next_usable_start)
    return min(candidates) if candidates else None


#: When two adjacent segments are closer together than this, there usually
#: isn't enough real silence between them for the clamp in _padded_bounds to
#: leave any actual room for the trailing pad -- Whisper split them for
#: transcription convenience, not because the speaker paused.
MERGE_GAP_SECONDS = 0.5


def _merge_close_segments(
    segments: list[tuple[float, float, str]],
    *,
    gap_seconds: float = MERGE_GAP_SECONDS,
    max_duration: float = MAX_SEGMENT_SECONDS,
) -> list[tuple[float, float, str]]:
    """Merge adjacent segments separated by less than `gap_seconds` into one,
    but only while the earlier segment's text isn't already a complete
    sentence.

    A short pause alone turned out not to be a reliable "safe to cut" signal:
    direct measurement showed ~40% of trained clips ending with their final
    60ms louder than the clip's own average (genuinely cut off mid-speech,
    not faded out), and inspecting the worst offenders showed why -- long
    runs of closely-spaced segments were merged all the way to the
    `max_duration` cap regardless of where that landed grammatically,
    producing training labels like "...these little things that" cut off
    mid-clause. Preferring to stop a merge run right after a segment ending
    in '.', '!' or '?' (a real sentence boundary) fixes that at the source:
    it only bridges segments that are genuinely mid-sentence fragments, and
    always leaves a clean stopping point once a sentence is actually
    complete, however small the pause after it was.
    """
    if not segments:
        return []
    merged = [segments[0]]
    for start, end, text in segments[1:]:
        prev_start, prev_end, prev_text = merged[-1]
        gap = start - prev_end
        combined_duration = end - prev_start
        prev_is_complete_sentence = prev_text.rstrip().endswith(('.', '!', '?'))
        if not prev_is_complete_sentence and gap < gap_seconds and combined_duration <= max_duration:
            merged[-1] = (prev_start, end, f'{prev_text} {text}')
        else:
            merged.append((start, end, text))
    return merged


def _run_whisper(audio_path: Path, *, model) -> list:
    """Run the model once and return its raw segment objects, unfiltered.

    `word_timestamps=True` makes faster-whisper attach a `.words` list (each
    with `.start`/`.end`/`.word`/`.probability`) to every segment, which
    `_trim_trailing_low_confidence_words` needs.
    """
    segments, _info = model.transcribe(str(audio_path), vad_filter=True, word_timestamps=True)
    return list(segments)


def _trim_trailing_low_confidence_words(segment) -> tuple[float, str, list[float]] | None:
    """Peel words faster-whisper wasn't confident about off the end of
    `segment`, returning (effective_end, trimmed_text, dropped_word_starts).

    Falls back to the segment's own end/text untouched (no words dropped) if
    it has no `.words` at all -- keeps this safe to call even without
    word_timestamps, and lets tests exercise unrelated behavior without
    constructing fake word lists. Returns None if every word is
    low-confidence, meaning the whole segment is unusable.
    """
    words = getattr(segment, 'words', None)
    if not words:
        return segment.end, segment.text.strip(), []
    words = list(words)
    dropped_starts = []
    while words and words[-1].probability < MIN_WORD_PROBABILITY:
        dropped_starts.append(words[-1].start)
        words.pop()
    if not words:
        return None
    text = ' '.join(w.word.strip() for w in words).strip()
    return words[-1].end, text, dropped_starts


def _filter_usable_segments(raw_segments) -> tuple[list[tuple[float, float, str]], list[float]]:
    """Keep only segments reliable enough to use as training labels -- see
    the module-level MIN/MAX_SEGMENT_SECONDS, MAX_NO_SPEECH_PROB, and
    MIN_WORD_PROBABILITY docs. Also returns the start of every low-confidence
    trailing word dropped along the way, for `_next_raw_boundary` to treat as
    a padding boundary.
    """
    usable = []
    dropped_word_starts: list[float] = []
    for segment in raw_segments:
        trimmed = _trim_trailing_low_confidence_words(segment)
        if trimmed is None:
            continue
        end, text, dropped_starts = trimmed
        dropped_word_starts.extend(dropped_starts)
        duration = end - segment.start
        if duration < MIN_SEGMENT_SECONDS or duration > MAX_SEGMENT_SECONDS:
            continue
        if not text:
            continue
        if segment.no_speech_prob > MAX_NO_SPEECH_PROB:
            continue
        usable.append((segment.start, end, text))
    return usable, dropped_word_starts


def transcribe_segments(audio_path: Path, *, model) -> list[tuple[float, float, str]]:
    """Return (start, end, text) for each usable segment in audio_path.

    `model` needs only a
    `.transcribe(path, vad_filter=True, word_timestamps=True) -> (segments, info)`
    method (matching faster_whisper.WhisperModel) so this is testable with a
    duck-typed fake -- no real model load required.
    """
    usable, _dropped_word_starts = _filter_usable_segments(_run_whisper(audio_path, model=model))
    return usable


def build_real_corpus(
    sample_paths: Sequence[Path],
    *,
    model,
    output_dir: Path,
    sample_rate: int = 22050,
    load_audio: Callable[[Path], object] = None,
) -> Path:
    """Transcribe each sample, slice out its usable segments, and write the
    Piper-training-format corpus. `load_audio` defaults to loading via pydub
    (resampled to `sample_rate` mono) -- injectable so tests never need a
    real audio file.
    """
    if load_audio is None:
        def load_audio(path: Path):
            from pydub import AudioSegment

            return AudioSegment.from_file(str(path)).set_frame_rate(sample_rate).set_channels(1)

    wavs_dir = output_dir / 'wavs'
    wavs_dir.mkdir(parents=True, exist_ok=True)
    metadata_path = output_dir / 'metadata.csv'
    index = 0

    with metadata_path.open('w', encoding='utf-8', newline='') as metadata_file:
        writer = csv.writer(metadata_file, delimiter='|')
        for sample_path in sample_paths:
            raw_segments = _run_whisper(sample_path, model=model)
            usable, dropped_word_starts = _filter_usable_segments(raw_segments)
            boundary_starts = [s.start for s in raw_segments] + dropped_word_starts
            segments = _merge_close_segments(usable)
            if not segments:
                continue
            audio = load_audio(sample_path)
            audio_duration = len(audio) / 1000.0
            for i, (start, end, text) in enumerate(segments):
                next_usable_start = segments[i + 1][0] if i + 1 < len(segments) else None
                next_start = _next_raw_boundary(end, boundary_starts, next_usable_start)
                padded_start, padded_end = _padded_bounds(
                    start, end, next_start=next_start, audio_duration=audio_duration,
                )
                clip = _normalize_loudness(audio[int(padded_start * 1000):int(padded_end * 1000)])
                clip_name = f'{index:04d}.wav'
                clip.export(str(wavs_dir / clip_name), format='wav')
                writer.writerow([clip_name, text])
                index += 1

    return metadata_path


def _fix_windows_cuda_dll_path() -> None:
    """faster-whisper's CUDA backend (CTranslate2) looks for cuBLAS/cuDNN
    DLLs on the standard Windows DLL search path, but a pip-installed torch
    ships its own private copies under its own `lib/` directory that aren't
    on that path by default -- so CUDA transcription fails with "Library
    cublas64_12.dll is not found" even on a machine with a working
    CUDA-enabled torch (`torch.cuda.is_available()` can be True while this
    still fails). Registering torch's own lib directory fixes it. Harmless
    no-op on non-Windows or if torch isn't installed -- CPU transcription
    doesn't need this.
    """
    if sys.platform != 'win32':
        return
    try:
        import torch
    except ImportError:
        return
    torch_lib = Path(torch.__file__).parent / 'lib'
    if torch_lib.is_dir():
        import os

        os.add_dll_directory(str(torch_lib))


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        '--samples', type=Path, nargs='+',
        default=sorted((_SCRIPT_DIR / 'samples').glob('*')),
        help='reference recordings to transcribe',
    )
    parser.add_argument('--output-dir', type=Path, default=_SCRIPT_DIR / 'corpus_real')
    parser.add_argument(
        '--whisper-model', default='medium',
        help="faster-whisper model size/name -- 'medium' is a meaningful accuracy step up from "
             "'small' (fewer mistranscribed words, better non-speech detection); 'large-v3' is "
             "more accurate still but considerably slower",
    )
    parser.add_argument('--device', default='cuda', choices=['cuda', 'cpu'])
    args = parser.parse_args(argv)

    if args.device == 'cuda':
        _fix_windows_cuda_dll_path()

    from faster_whisper import WhisperModel

    model = WhisperModel(args.whisper_model, device=args.device, compute_type='float16' if args.device == 'cuda' else 'int8')
    metadata_path = build_real_corpus(args.samples, model=model, output_dir=args.output_dir)

    with metadata_path.open(encoding='utf-8') as f:
        count = sum(1 for _ in f)
    print(f'Wrote {count} real utterances to {metadata_path}')
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
