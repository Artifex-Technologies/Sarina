"""Tests for sarina_helpers.tts.

piper-tts needs a real Piper voice model, which the test suite must not
depend on - so TextToSpeech construction against a missing extra/model file
is tested for its error path only, and synthesize()'s glue logic is tested
against a hand-written duck-typed fake voice whose output is verified by
round-tripping it through stdlib wave, mirroring tests/test_ml_embeddings.py's
conventions.
"""

from __future__ import annotations

import io
import tempfile
import unittest
import wave
from pathlib import Path
from unittest.mock import MagicMock, patch

from sarina_helpers.audio_io import VoiceUnavailable
from sarina_helpers.tts import SENTENCE_PAUSE_SECONDS, TextToSpeech, _join_wav_clips, _split_sentences


class TextToSpeechErrorPathTest(unittest.TestCase):
    def test_missing_extra_names_piper_tts(self):
        with patch('sarina_helpers.tts.TTS_AVAILABLE', False):
            with self.assertRaises(VoiceUnavailable) as ctx:
                TextToSpeech(Path('/does/not/matter.onnx'))
        self.assertIn('piper-tts', str(ctx.exception))

    def test_missing_model_file_raises(self):
        with patch('sarina_helpers.tts.TTS_AVAILABLE', True):
            with self.assertRaises(VoiceUnavailable):
                TextToSpeech(Path('/does/not/exist/voice.onnx'))

    def test_missing_explicit_config_raises(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            model_path = Path(tmp_dir) / 'voice.onnx'
            model_path.write_bytes(b'not a real model')
            with patch('sarina_helpers.tts.TTS_AVAILABLE', True):
                with self.assertRaises(VoiceUnavailable):
                    TextToSpeech(model_path, config_path=Path(tmp_dir) / 'missing.json')


class SplitSentencesTest(unittest.TestCase):
    def test_splits_on_sentence_ending_punctuation(self):
        text = "Hello there. How are you? I'm doing great!"
        self.assertEqual(_split_sentences(text), ['Hello there.', 'How are you?', "I'm doing great!"])

    def test_single_sentence_returns_one_item(self):
        self.assertEqual(_split_sentences('hello there'), ['hello there'])

    def test_strips_whitespace_and_drops_empty_fragments(self):
        self.assertEqual(_split_sentences('  One.   Two.  '), ['One.', 'Two.'])

    def test_empty_text_returns_empty_list(self):
        self.assertEqual(_split_sentences(''), [])


def _make_wav_bytes(*, num_frames: int, framerate: int = 22050) -> bytes:
    buffer = io.BytesIO()
    with wave.open(buffer, 'wb') as wav_file:
        wav_file.setnchannels(1)
        wav_file.setsampwidth(2)
        wav_file.setframerate(framerate)
        wav_file.writeframes(b'\x01\x00' * num_frames)
    return buffer.getvalue()


class JoinWavClipsTest(unittest.TestCase):
    def test_joins_clips_with_silence_between_them(self):
        clip_a = _make_wav_bytes(num_frames=10)
        clip_b = _make_wav_bytes(num_frames=20)
        joined = _join_wav_clips([clip_a, clip_b], pause_seconds=0.5)

        with wave.open(io.BytesIO(joined), 'rb') as wav_file:
            self.assertEqual(wav_file.getframerate(), 22050)
            # 10 + 20 real frames plus a 0.5s silence gap (11025 frames at 22050 Hz).
            self.assertEqual(wav_file.getnframes(), 10 + 20 + 11025)

    def test_single_clip_has_no_added_silence(self):
        clip = _make_wav_bytes(num_frames=15)
        joined = _join_wav_clips([clip], pause_seconds=1.0)
        with wave.open(io.BytesIO(joined), 'rb') as wav_file:
            self.assertEqual(wav_file.getnframes(), 15)


class TextToSpeechSynthesizeTest(unittest.TestCase):
    def _build_tts(self, fake_voice) -> TextToSpeech:
        with tempfile.TemporaryDirectory() as tmp_dir:
            model_path = Path(tmp_dir) / 'voice.onnx'
            model_path.write_bytes(b'not a real model')
            with patch('sarina_helpers.tts.TTS_AVAILABLE', True), \
                 patch('sarina_helpers.tts.PiperVoice') as fake_piper_voice_cls:
                fake_piper_voice_cls.load.return_value = fake_voice
                return TextToSpeech(model_path)

    def test_synthesize_returns_bytes_that_round_trip_through_wave(self):
        def _fake_synthesize_wav(text, wav_file):
            wav_file.setnchannels(1)
            wav_file.setsampwidth(2)
            wav_file.setframerate(22050)
            wav_file.writeframes(b'\x00\x00' * 100)

        fake_voice = MagicMock()
        fake_voice.synthesize_wav.side_effect = _fake_synthesize_wav
        tts = self._build_tts(fake_voice)

        wav_bytes = tts.synthesize('hello there')

        self.assertTrue(wav_bytes.startswith(b'RIFF'))
        fake_voice.synthesize_wav.assert_called_once()
        with wave.open(io.BytesIO(wav_bytes), 'rb') as wav_file:
            self.assertEqual(wav_file.getframerate(), 22050)

    def test_multi_sentence_text_synthesizes_each_sentence_separately(self):
        calls = []

        def _fake_synthesize_wav(text, wav_file):
            calls.append(text)
            wav_file.setnchannels(1)
            wav_file.setsampwidth(2)
            wav_file.setframerate(22050)
            wav_file.writeframes(b'\x00\x00' * 50)

        fake_voice = MagicMock()
        fake_voice.synthesize_wav.side_effect = _fake_synthesize_wav
        tts = self._build_tts(fake_voice)

        wav_bytes = tts.synthesize('First sentence. Second sentence.')

        self.assertEqual(calls, ['First sentence.', 'Second sentence.'])
        with wave.open(io.BytesIO(wav_bytes), 'rb') as wav_file:
            expected_pause_frames = int(22050 * SENTENCE_PAUSE_SECONDS)
            self.assertEqual(wav_file.getnframes(), 50 + 50 + expected_pause_frames)


if __name__ == '__main__':
    unittest.main()
