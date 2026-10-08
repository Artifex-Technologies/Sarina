"""Optional Python helpers for the Go ``sarina`` binary.

Original code (embedding models, speech I/O) that needs onnxruntime,
faster-whisper or Piper, which Go cannot use. Importing this package never
fails and imports no third-party module: only constructing ``EmbeddingModel``,
``SpeechToText`` or ``TextToSpeech`` without the matching extra (``ml`` /
``voice``) raises ``MLUnavailable`` / ``VoiceUnavailable`` with an install hint.

See ``PROTOCOL.md`` for how the Go side calls in.
"""

from __future__ import annotations

from .audio_io import ALSA_AVAILABLE, VoiceUnavailable
from .embeddings import ML_AVAILABLE, EmbeddingModel, MLUnavailable
from .stt import STT_AVAILABLE
from .tts import TTS_AVAILABLE

__version__ = '0.1.0'

#: True only when ALSA tools, faster-whisper, and piper-tts are all available.
VOICE_AVAILABLE = ALSA_AVAILABLE and STT_AVAILABLE and TTS_AVAILABLE

__all__ = [
    'ALSA_AVAILABLE', 'EmbeddingModel', 'MLUnavailable', 'ML_AVAILABLE', 'STT_AVAILABLE',
    'TTS_AVAILABLE', 'VOICE_AVAILABLE', 'VoiceUnavailable', '__version__',
]
