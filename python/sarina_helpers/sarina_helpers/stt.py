"""Speech-to-text via faster-whisper (CTranslate2 backend).

``faster-whisper`` is an optional dependency (the ``voice`` extra in
``pyproject.toml``), following the same pattern as onnxruntime/tokenizers in
``embeddings.py``: importing this module never fails, but
constructing ``SpeechToText`` without the extra installed raises
``VoiceUnavailable`` with an actionable message.
"""

from __future__ import annotations

import tempfile
from dataclasses import dataclass
from pathlib import Path

from .audio_io import VoiceUnavailable

try:
    from faster_whisper import WhisperModel

    _FASTER_WHISPER_AVAILABLE = True
except ImportError:
    WhisperModel = None  # type: ignore[assignment]
    _FASTER_WHISPER_AVAILABLE = False

#: True only when faster-whisper is importable.
STT_AVAILABLE = _FASTER_WHISPER_AVAILABLE


def _require_stt() -> None:
    if not STT_AVAILABLE:
        raise VoiceUnavailable(
            'missing faster-whisper - install with pip install ".[voice]" '
            '(not yet packaged for Arch; see docs/ECOSYSTEM.md)'
        )


@dataclass(frozen=True)
class Transcript:
    text: str
    language: str
    duration_seconds: float


class SpeechToText:
    """Wraps a local faster-whisper model directory.

    No model is downloaded here: ``model_dir`` must be a CTranslate2 model
    directory already on disk. Loaded with ``local_files_only=True`` so a
    missing/misnamed directory fails immediately instead of silently
    reaching out to Hugging Face - deliberate, on a security-focused distro
    where a voice feature should never phone home by default.
    """

    def __init__(self, model_dir: Path | str, *, device: str = 'cpu', compute_type: str = 'int8'):
        _require_stt()
        model_dir = Path(model_dir)
        if not model_dir.is_dir():
            raise VoiceUnavailable(f'no such STT model directory: {model_dir}')
        self._model = WhisperModel(
            str(model_dir), device=device, compute_type=compute_type, local_files_only=True,
        )

    def transcribe(self, wav_bytes: bytes, *, language: str | None = None) -> Transcript:
        with tempfile.NamedTemporaryFile(suffix='.wav') as tmp_file:
            tmp_file.write(wav_bytes)
            tmp_file.flush()
            segments, info = self._model.transcribe(tmp_file.name, language=language)
            text = ' '.join(segment.text.strip() for segment in segments).strip()
        return Transcript(text=text, language=info.language, duration_seconds=info.duration)
