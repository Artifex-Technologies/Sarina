"""Local push-to-talk audio capture and playback via ALSA command-line tools.

The other two optional pieces of ``sarina_helpers`` (STT, TTS) need the
``voice`` extra, but this module itself has no third-party dependency at
all: it shells out to ``arecord``/``aplay`` (``alsa-utils``), a
subprocess-over-library choice. ``VoiceUnavailable`` is
defined here rather than in ``__init__.py`` so ``stt.py``/``tts.py`` can
raise it without an import cycle.
"""

from __future__ import annotations

import shutil
import subprocess
import tempfile
import threading
import wave
from dataclasses import dataclass
from pathlib import Path
from typing import Callable

#: What faster-whisper expects and what Piper produces - one rate throughout
#: the pipeline avoids a resampling step.
DEFAULT_SAMPLE_RATE = 16000
DEFAULT_CHANNELS = 1

_ARECORD = shutil.which('arecord')
_APLAY = shutil.which('aplay')

#: True only when both ALSA command-line tools are on PATH.
ALSA_AVAILABLE = bool(_ARECORD and _APLAY)


class VoiceUnavailable(RuntimeError):
    """The ``voice`` extra isn't installed, a model file is missing, or the
    ALSA command-line tools (``arecord``/``aplay``) are absent."""


def _require_alsa() -> None:
    if not ALSA_AVAILABLE:
        missing = []
        if not _ARECORD:
            missing.append('arecord')
        if not _APLAY:
            missing.append('aplay')
        raise VoiceUnavailable(
            f'missing {" and ".join(missing)} - install alsa-utils (Arch: pacman -S alsa-utils)'
        )


@dataclass(frozen=True)
class RecordedAudio:
    wav_bytes: bytes
    sample_rate: int
    duration_seconds: float


def record_push_to_talk(
    *,
    input_func: Callable[[str], str] = input,
    output_func: Callable[[str], None] = print,
    device: str | None = None,
    sample_rate: int = DEFAULT_SAMPLE_RATE,
    max_seconds: float = 120.0,
) -> RecordedAudio:
    """Record one utterance: start now, stop on the next ``input_func()`` call.

    A terminal only reports completed keystrokes, not key-down/key-up, so
    "push-to-talk" here is a two-Enter-press protocol: print a recording
    prompt, start ``arecord`` in the background, block on ``input_func()``
    for the stop keypress, then terminate the process. A ``max_seconds``
    timer guards against a stop keypress that never comes.
    """
    _require_alsa()
    with tempfile.TemporaryDirectory(prefix='sarina-voice-') as tmp_dir:
        wav_path = Path(tmp_dir) / 'utterance.wav'
        args = [
            _ARECORD, '-q',
            '-f', 'S16_LE',
            '-r', str(sample_rate),
            '-c', str(DEFAULT_CHANNELS),
            '-t', 'wav',
        ]
        if device:
            args += ['-D', device]
        args.append(str(wav_path))

        proc = subprocess.Popen(args)
        timer = threading.Timer(max_seconds, proc.terminate)
        timer.start()
        try:
            output_func('Recording... press Enter to stop.')
            input_func('')
        finally:
            timer.cancel()
            if proc.poll() is None:
                proc.terminate()
            proc.wait()

        wav_bytes = wav_path.read_bytes()
        with wave.open(str(wav_path), 'rb') as wav_file:
            frame_rate = wav_file.getframerate()
            duration_seconds = wav_file.getnframes() / frame_rate if frame_rate else 0.0

    return RecordedAudio(wav_bytes=wav_bytes, sample_rate=frame_rate, duration_seconds=duration_seconds)


def play_audio(wav_bytes: bytes, *, device: str | None = None) -> None:
    """Play a complete WAV file's bytes through ALSA."""
    _require_alsa()
    args = [_APLAY, '-q']
    if device:
        args += ['-D', device]
    args.append('-')
    proc = subprocess.run(args, input=wav_bytes, capture_output=True)
    if proc.returncode != 0:
        raise RuntimeError(f'aplay failed: {proc.stderr.decode(errors="replace").strip()}')
