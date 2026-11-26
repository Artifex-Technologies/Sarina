"""Piper text to speech for build_corpus.py: a stock Piper voice, one call per
sentence, stitched with a short pause.

A Piper voice reads long, multi-sentence text worse than single sentences (its
duration model was trained on short clips), so each sentence is synthesized on
its own and the clips are joined with SENTENCE_PAUSE_SECONDS of silence, as
Sarina's voice does. Needs piper-tts (requirements.txt); imported only when a
corpus is really built, never by the tests.
"""

from __future__ import annotations

import io
import re
import wave
from pathlib import Path

_SENTENCE_BOUNDARY = re.compile(r'(?<=[.!?])\s+')

#: Silence between stitched sentences.
SENTENCE_PAUSE_SECONDS = 0.3


def split_sentences(text: str) -> list[str]:
    """Split on sentence-ending punctuation followed by whitespace (a heuristic:
    "Mr. Smith" becomes two pieces, which costs only a short pause)."""
    return [sentence for sentence in (part.strip() for part in _SENTENCE_BOUNDARY.split(text.strip())) if sentence]


def join_wav_clips(clips: list[bytes], *, pause_seconds: float) -> bytes:
    """Concatenate complete WAV files' bytes with a silence gap between each."""
    frames = []
    params = None
    for clip in clips:
        with wave.open(io.BytesIO(clip), 'rb') as wav_file:
            params = wav_file.getparams()
            frames.append(wav_file.readframes(wav_file.getnframes()))

    silence = b'\x00' * int(params.framerate * pause_seconds) * params.sampwidth * params.nchannels

    buffer = io.BytesIO()
    with wave.open(buffer, 'wb') as out:
        out.setparams(params)
        for i, clip_frames in enumerate(frames):
            out.writeframes(clip_frames)
            if i < len(frames) - 1:
                out.writeframes(silence)
    return buffer.getvalue()


class TextToSpeech:
    """A local Piper voice model (`.onnx`; its config defaults to Piper's own
    `<model>.json` discovery)."""

    def __init__(self, model_path: Path | str, config_path: Path | str | None = None):
        from piper import PiperVoice

        model_path = Path(model_path)
        if not model_path.is_file():
            raise FileNotFoundError(f'no such Piper voice model: {model_path}')
        if config_path is not None and not Path(config_path).is_file():
            raise FileNotFoundError(f'no such Piper voice config: {config_path}')
        self._voice = PiperVoice.load(str(model_path), config_path=str(config_path) if config_path else None)

    def synthesize(self, text: str) -> bytes:
        """A complete WAV file's bytes for `text`, sentence by sentence."""
        sentences = split_sentences(text)
        if len(sentences) <= 1:
            return self._synthesize_one(text)
        return join_wav_clips([self._synthesize_one(sentence) for sentence in sentences], pause_seconds=SENTENCE_PAUSE_SECONDS)

    def _synthesize_one(self, text: str) -> bytes:
        buffer = io.BytesIO()
        with wave.open(buffer, 'wb') as wav_file:
            self._voice.synthesize_wav(text, wav_file)
        return buffer.getvalue()
