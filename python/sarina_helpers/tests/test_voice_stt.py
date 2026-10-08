"""Tests for sarina_helpers.stt.

faster-whisper needs a real CTranslate2 model, which the test suite must not
depend on (no network, no model backend) - so SpeechToText construction
against a missing extra/directory is tested for its error path only, and
transcribe()'s glue logic is tested against a hand-written duck-typed fake
model, mirroring tests/test_ml_embeddings.py's conventions.
"""

from __future__ import annotations

import tempfile
import unittest
from pathlib import Path
from unittest.mock import MagicMock, patch

from sarina_helpers.audio_io import VoiceUnavailable
from sarina_helpers.stt import SpeechToText, Transcript


class SpeechToTextErrorPathTest(unittest.TestCase):
    def test_missing_extra_names_faster_whisper(self):
        with patch('sarina_helpers.stt.STT_AVAILABLE', False):
            with self.assertRaises(VoiceUnavailable) as ctx:
                SpeechToText(Path('/does/not/matter'))
        self.assertIn('faster-whisper', str(ctx.exception))

    def test_missing_model_directory_raises(self):
        with patch('sarina_helpers.stt.STT_AVAILABLE', True), \
             patch('sarina_helpers.stt.WhisperModel'):
            with self.assertRaises(VoiceUnavailable):
                SpeechToText(Path('/does/not/exist/at/all'))


class SpeechToTextTranscribeTest(unittest.TestCase):
    def test_transcribe_joins_segments_and_reports_info(self):
        fake_segment_1 = MagicMock(text=' Hello ')
        fake_segment_2 = MagicMock(text='world. ')
        fake_info = MagicMock(language='en', duration=1.23)
        fake_model = MagicMock()
        fake_model.transcribe.return_value = ([fake_segment_1, fake_segment_2], fake_info)

        with patch('sarina_helpers.stt.STT_AVAILABLE', True), \
             patch('sarina_helpers.stt.WhisperModel', return_value=fake_model), \
             tempfile.TemporaryDirectory() as tmp_dir:
            stt = SpeechToText(Path(tmp_dir))
            transcript = stt.transcribe(b'RIFF-fake-wav-bytes')

        self.assertEqual(transcript, Transcript(text='Hello world.', language='en', duration_seconds=1.23))
        fake_model.transcribe.assert_called_once()


if __name__ == '__main__':
    unittest.main()
