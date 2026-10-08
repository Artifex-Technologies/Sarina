"""Optional local voice I/O: push-to-talk speech-to-text input and
text-to-speech output for ``sarina voice-chat``.

Needs the ``voice`` extra: ``faster-whisper`` (STT) and ``piper-tts`` (TTS).
``sarina.voice.audio_io`` itself has no Python dependency - it shells out to
``arecord``/``aplay``, the same subprocess-over-library precedent as
``sarina/chymaera/firmware.py`` (git) and ``sarina/system_resources.py``
(``nvidia-smi``). The core agent does not import this package.

No model is bundled or downloaded automatically: ``SpeechToText`` takes a
local CTranslate2 model directory, ``TextToSpeech`` takes a local Piper
voice model file, both operator-provided - same convention as
``sarina.ml.EmbeddingModel(model_dir)``. This is also the seam a future
custom cloned voice (produced offline, e.g. via OpenVoice) drops into: a
different file path, same constructor.
"""

from __future__ import annotations

from .audio_io import ALSA_AVAILABLE, RecordedAudio, VoiceUnavailable, play_audio, record_push_to_talk
from .stt import STT_AVAILABLE, SpeechToText, Transcript
from .tts import TTS_AVAILABLE, TextToSpeech

#: True only when ALSA tools, faster-whisper, and piper-tts are all available.
VOICE_AVAILABLE = ALSA_AVAILABLE and STT_AVAILABLE and TTS_AVAILABLE


def _require_voice() -> None:
    """One combined check for the CLI to call up front, so a user missing
    multiple pieces at once (e.g. no ALSA tools *and* no faster-whisper) gets
    one message naming all of them, not a fix-one/rerun/discover-the-next
    loop.
    """
    missing = []
    if not ALSA_AVAILABLE:
        missing.append('alsa-utils (arecord/aplay)')
    if not STT_AVAILABLE:
        missing.append('faster-whisper')
    if not TTS_AVAILABLE:
        missing.append('piper-tts')
    if missing:
        raise VoiceUnavailable(
            f'missing {", ".join(missing)} - install alsa-utils via your package '
            'manager and the rest with pip install -e ".[voice]"'
        )


__all__ = [
    'ALSA_AVAILABLE',
    'RecordedAudio',
    'STT_AVAILABLE',
    'SpeechToText',
    'TTS_AVAILABLE',
    'TextToSpeech',
    'Transcript',
    'VOICE_AVAILABLE',
    'VoiceUnavailable',
    'play_audio',
    'record_push_to_talk',
]
