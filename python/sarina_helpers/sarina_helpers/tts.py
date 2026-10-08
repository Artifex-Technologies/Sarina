"""Text-to-speech via Piper (onnxruntime backend).

``piper-tts`` is an optional dependency (the ``voice`` extra in
``pyproject.toml``). Same guard pattern as faster-whisper in
``stt.py`` and onnxruntime/tokenizers in
``embeddings.py``.
"""

from __future__ import annotations

import io
import re
import wave
from pathlib import Path

from .audio_io import VoiceUnavailable

try:
    from piper import PiperVoice

    _PIPER_AVAILABLE = True
except ImportError:
    PiperVoice = None  # type: ignore[assignment]
    _PIPER_AVAILABLE = False

#: True only when piper-tts is importable.
TTS_AVAILABLE = _PIPER_AVAILABLE

#: A trained Piper voice's duration/prosody modeling is only as good as its
#: training clips' lengths. Feeding a long, multi-sentence response through
#: in one synthesize_wav() call pushes it well outside that length
#: distribution and measurably degrades intelligibility partway through --
#: confirmed by direct A/B comparison during voice-cloning work (identical
#: text and model, one call vs. one call per sentence). Splitting per
#: sentence and stitching the results keeps every individual call close to
#: what the model actually saw in training.
_SENTENCE_BOUNDARY = re.compile(r'(?<=[.!?])\s+')

#: Silence between stitched sentences: long enough to read as a natural
#: pause, short enough not to sound like separate, disconnected clips.
SENTENCE_PAUSE_SECONDS = 0.3


def _split_sentences(text: str) -> list[str]:
    """Split on sentence-ending punctuation followed by whitespace.

    A simple heuristic, not a full sentence tokenizer: it will split
    mid-abbreviation ("Mr. Smith") or mid-number ("3.5 seconds"). Accepted
    tradeoff for spoken assistant responses -- getting it wrong means one
    fragment gets its own synthesis call and a brief pause where there
    shouldn't be one, not a wrong or unintelligible result.
    """
    return [sentence for sentence in (part.strip() for part in _SENTENCE_BOUNDARY.split(text.strip())) if sentence]


def _join_wav_clips(clips: list[bytes], *, pause_seconds: float) -> bytes:
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


def _require_tts() -> None:
    if not TTS_AVAILABLE:
        raise VoiceUnavailable(
            'missing piper-tts - install with pip install ".[voice]" '
            '(not yet packaged for Arch; see docs/ECOSYSTEM.md)'
        )


class TextToSpeech:
    """Wraps a local Piper voice model.

    No voice is bundled: ``model_path`` must be a Piper ``.onnx`` voice
    model already on disk. ``config_path`` defaults to Piper's own
    ``<model_path>.json`` discovery when omitted. This is also the seam a
    future custom cloned voice (produced offline, e.g. via OpenVoice) drops
    into - a different ``model_path`` to this same constructor.
    """

    def __init__(self, model_path: Path | str, config_path: Path | str | None = None):
        _require_tts()
        model_path = Path(model_path)
        if not model_path.is_file():
            raise VoiceUnavailable(f'no such TTS voice model: {model_path}')
        if config_path is not None:
            config_path = Path(config_path)
            if not config_path.is_file():
                raise VoiceUnavailable(f'no such TTS voice config: {config_path}')
        try:
            self._voice = PiperVoice.load(
                str(model_path), config_path=str(config_path) if config_path else None,
            )
        except (OSError, ValueError) as exc:
            raise VoiceUnavailable(f'failed to load TTS voice {model_path}: {exc}') from exc

    def synthesize(self, text: str) -> bytes:
        """Return a complete WAV file's bytes for ``text``.

        Splits on sentence boundaries and synthesizes each one separately,
        stitched together with a brief pause -- see ``_split_sentences``'s
        docstring for why.
        """
        sentences = _split_sentences(text)
        if len(sentences) <= 1:
            return self._synthesize_one(text)
        return _join_wav_clips(
            [self._synthesize_one(sentence) for sentence in sentences],
            pause_seconds=SENTENCE_PAUSE_SECONDS,
        )

    def _synthesize_one(self, text: str) -> bytes:
        buffer = io.BytesIO()
        with wave.open(buffer, 'wb') as wav_file:
            self._voice.synthesize_wav(text, wav_file)
        return buffer.getvalue()
