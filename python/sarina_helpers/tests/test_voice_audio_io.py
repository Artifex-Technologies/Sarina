"""Tests for sarina_helpers.audio_io.

Never shells out to a real arecord/aplay: subprocess.Popen/.run are patched,
the same technique tests/test_system_resources.py uses for nvidia-smi. A fake
Popen writes a small real WAV fixture (built with stdlib wave) to the target
path, standing in for what arecord would have produced.
"""

from __future__ import annotations

import subprocess
import unittest
import wave
from pathlib import Path
from unittest.mock import patch

from sarina_helpers.audio_io import (
    RecordedAudio,
    VoiceUnavailable,
    _require_alsa,
    play_audio,
    record_push_to_talk,
)


def _write_wav_fixture(path: Path, *, framerate: int = 16000, num_frames: int = 1600) -> None:
    with wave.open(str(path), 'wb') as wav_file:
        wav_file.setnchannels(1)
        wav_file.setsampwidth(2)
        wav_file.setframerate(framerate)
        wav_file.writeframes(b'\x00\x00' * num_frames)


def _make_fake_popen(terminate_calls: list):
    class FakePopen:
        def __init__(self, args, **kwargs):
            self.args = args
            _write_wav_fixture(Path(args[-1]))
            self._done = False

        def poll(self):
            return 0 if self._done else None

        def terminate(self):
            self._done = True
            terminate_calls.append(self.args)

        def wait(self, timeout=None):
            return 0

    return FakePopen


class FakeTimer:
    """Stands in for threading.Timer so tests can fire it manually instead
    of actually waiting max_seconds."""

    instances: list = []

    def __init__(self, interval, function):
        self.interval = interval
        self.function = function
        self.cancelled = False
        FakeTimer.instances.append(self)

    def start(self):
        pass

    def cancel(self):
        self.cancelled = True


class RequireAlsaTest(unittest.TestCase):
    def test_names_both_missing_tools(self):
        with patch('sarina_helpers.audio_io.ALSA_AVAILABLE', False), \
             patch('sarina_helpers.audio_io._ARECORD', None), \
             patch('sarina_helpers.audio_io._APLAY', None):
            with self.assertRaises(VoiceUnavailable) as ctx:
                _require_alsa()
        self.assertIn('arecord', str(ctx.exception))
        self.assertIn('aplay', str(ctx.exception))


class RecordPushToTalkTest(unittest.TestCase):
    def test_stops_and_reads_back_recording_on_input(self):
        terminate_calls: list = []
        with patch('sarina_helpers.audio_io.subprocess.Popen', _make_fake_popen(terminate_calls)), \
             patch('sarina_helpers.audio_io.ALSA_AVAILABLE', True):
            result = record_push_to_talk(input_func=lambda prompt: '', output_func=lambda line: None)

        self.assertIsInstance(result, RecordedAudio)
        self.assertEqual(result.sample_rate, 16000)
        self.assertGreater(result.duration_seconds, 0)
        self.assertEqual(len(terminate_calls), 1)

    def test_max_seconds_timer_terminates_recording(self):
        FakeTimer.instances = []
        terminate_calls: list = []

        def _input_fires_timer(prompt: str) -> str:
            FakeTimer.instances[0].function()
            return ''

        with patch('sarina_helpers.audio_io.subprocess.Popen', _make_fake_popen(terminate_calls)), \
             patch('sarina_helpers.audio_io.threading.Timer', FakeTimer), \
             patch('sarina_helpers.audio_io.ALSA_AVAILABLE', True):
            record_push_to_talk(input_func=_input_fires_timer, output_func=lambda line: None, max_seconds=5.0)

        self.assertEqual(FakeTimer.instances[0].interval, 5.0)
        self.assertEqual(len(terminate_calls), 1)

    def test_raises_without_alsa_tools(self):
        with patch('sarina_helpers.audio_io.ALSA_AVAILABLE', False):
            with self.assertRaises(VoiceUnavailable):
                record_push_to_talk(input_func=lambda prompt: '', output_func=lambda line: None)


class PlayAudioTest(unittest.TestCase):
    def test_pipes_bytes_and_includes_device_flag(self):
        completed = subprocess.CompletedProcess(args=[], returncode=0, stdout=b'', stderr=b'')
        with patch('sarina_helpers.audio_io.subprocess.run', return_value=completed) as run_mock, \
             patch('sarina_helpers.audio_io.ALSA_AVAILABLE', True), \
             patch('sarina_helpers.audio_io._APLAY', 'aplay'):
            play_audio(b'RIFF-fake-wav', device='hw:0,0')

        args, kwargs = run_mock.call_args
        self.assertIn('-D', args[0])
        self.assertIn('hw:0,0', args[0])
        self.assertEqual(kwargs['input'], b'RIFF-fake-wav')

    def test_raises_runtime_error_on_nonzero_exit(self):
        completed = subprocess.CompletedProcess(args=[], returncode=1, stdout=b'', stderr=b'device busy')
        with patch('sarina_helpers.audio_io.subprocess.run', return_value=completed), \
             patch('sarina_helpers.audio_io.ALSA_AVAILABLE', True), \
             patch('sarina_helpers.audio_io._APLAY', 'aplay'):
            with self.assertRaises(RuntimeError) as ctx:
                play_audio(b'RIFF-fake-wav')
        self.assertIn('device busy', str(ctx.exception))

    def test_raises_without_alsa_tools(self):
        with patch('sarina_helpers.audio_io.ALSA_AVAILABLE', False):
            with self.assertRaises(VoiceUnavailable):
                play_audio(b'RIFF-fake-wav')


if __name__ == '__main__':
    unittest.main()
