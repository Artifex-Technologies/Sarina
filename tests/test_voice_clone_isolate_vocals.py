"""Tests for tools/voice_clone/isolate_vocals.py.

No real Demucs install, no real audio, no network: subprocess.run and
shutil.which are patched, the same technique tests/test_voice_audio_io.py
uses for arecord/aplay.
"""

from __future__ import annotations

import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from tools.voice_clone.isolate_vocals import isolate_vocals


class IsolateVocalsTest(unittest.TestCase):
    def test_raises_when_demucs_is_missing(self):
        with patch('tools.voice_clone.isolate_vocals.shutil.which', return_value=None):
            with self.assertRaises(RuntimeError) as ctx:
                isolate_vocals(Path('input.mp3'), Path('output.wav'))
        self.assertIn('demucs', str(ctx.exception))

    def test_raises_with_demucs_stderr_on_failure(self):
        completed = subprocess.CompletedProcess(args=[], returncode=1, stdout='', stderr='out of memory')
        with patch('tools.voice_clone.isolate_vocals.shutil.which', return_value='/usr/bin/demucs'), \
             patch('tools.voice_clone.isolate_vocals.subprocess.run', return_value=completed):
            with self.assertRaises(RuntimeError) as ctx:
                isolate_vocals(Path('input.mp3'), Path('output.wav'))
        self.assertIn('out of memory', str(ctx.exception))

    def test_copies_vocals_output_to_the_requested_path(self):
        def _fake_run(args, **kwargs):
            # args: [demucs, '--two-stems=vocals', '-n', model, '-o', tmp_dir, str(input_path)]
            model = args[3]
            out_dir = Path(args[5])
            input_stem = Path(args[6]).stem
            vocals_dir = out_dir / model / input_stem
            vocals_dir.mkdir(parents=True, exist_ok=True)
            (vocals_dir / 'vocals.wav').write_bytes(b'RIFF-fake-vocals')
            return subprocess.CompletedProcess(args=args, returncode=0, stdout='', stderr='')

        with tempfile.TemporaryDirectory() as tmp_dir:
            output_path = Path(tmp_dir) / 'clean' / 'Sample06.wav'
            with patch('tools.voice_clone.isolate_vocals.shutil.which', return_value='/usr/bin/demucs'), \
                 patch('tools.voice_clone.isolate_vocals.subprocess.run', side_effect=_fake_run):
                result = isolate_vocals(Path('noisy_input.mp3'), output_path)

            self.assertEqual(result, output_path)
            self.assertEqual(output_path.read_bytes(), b'RIFF-fake-vocals')

    def test_raises_if_demucs_exits_ok_but_output_is_missing(self):
        completed = subprocess.CompletedProcess(args=[], returncode=0, stdout='', stderr='')
        with tempfile.TemporaryDirectory() as tmp_dir:
            with patch('tools.voice_clone.isolate_vocals.shutil.which', return_value='/usr/bin/demucs'), \
                 patch('tools.voice_clone.isolate_vocals.subprocess.run', return_value=completed):
                with self.assertRaises(RuntimeError) as ctx:
                    isolate_vocals(Path('input.mp3'), Path(tmp_dir) / 'output.wav')
        self.assertIn('did not produce', str(ctx.exception))


if __name__ == '__main__':
    unittest.main()
