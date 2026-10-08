"""Tests for tools/voice_clone/build_corpus.py.

No OpenVoice, no torch, no GPU, no network: the pure glue logic
(load_sentences, extract_target_embedding, build_corpus) is tested against
hand-written fakes for the TextToSpeech/ToneColorConverter/get_se/average
dependencies it injects, following this repo's own convention for testing
optional heavy-dependency code (sarina/ml/, sarina/voice/).
"""

from __future__ import annotations

import csv
import tempfile
import unittest
import wave
from pathlib import Path

from tools.voice_clone.build_corpus import build_corpus, extract_target_embedding, load_sentences


def _write_wav_bytes(*, num_frames: int = 160, framerate: int = 16000) -> bytes:
    import io

    buffer = io.BytesIO()
    with wave.open(buffer, 'wb') as wav_file:
        wav_file.setnchannels(1)
        wav_file.setsampwidth(2)
        wav_file.setframerate(framerate)
        wav_file.writeframes(b'\x00\x00' * num_frames)
    return buffer.getvalue()


class FakeTTS:
    def __init__(self):
        self.synthesized: list[str] = []

    def synthesize(self, text: str) -> bytes:
        self.synthesized.append(text)
        return _write_wav_bytes()


class FakeConverter:
    def __init__(self):
        self.calls: list[dict] = []

    def convert(self, *, audio_src_path, src_se, tgt_se, output_path, message=''):
        self.calls.append(
            {'audio_src_path': audio_src_path, 'src_se': src_se, 'tgt_se': tgt_se, 'output_path': output_path}
        )
        Path(output_path).write_bytes(_write_wav_bytes())


class LoadSentencesTest(unittest.TestCase):
    def test_skips_blank_lines_and_comments(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            path = Path(tmp_dir) / 'sentences.txt'
            path.write_text('# a comment\n\nFirst sentence.\n\nSecond sentence.\n', encoding='utf-8')
            self.assertEqual(load_sentences(path), ['First sentence.', 'Second sentence.'])

    def test_empty_file_returns_empty_list(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            path = Path(tmp_dir) / 'sentences.txt'
            path.write_text('', encoding='utf-8')
            self.assertEqual(load_sentences(path), [])


class ExtractTargetEmbeddingTest(unittest.TestCase):
    def test_averages_embeddings_across_samples(self):
        converter = FakeConverter()
        calls = []

        def _fake_get_se(audio_path, converter_arg, *, vad):
            calls.append((audio_path, vad))
            return (float(len(audio_path)), 'name')

        embedding = extract_target_embedding(
            [Path('sample1.mp3'), Path('sample2.mp3')],
            converter,
            get_se=_fake_get_se,
            average=lambda embeddings: sum(embeddings) / len(embeddings),
        )

        self.assertEqual(len(calls), 2)
        self.assertTrue(all(vad is True for _path, vad in calls))
        self.assertAlmostEqual(embedding, (float(len('sample1.mp3')) + float(len('sample2.mp3'))) / 2)

    def test_raises_without_any_samples(self):
        with self.assertRaises(ValueError):
            extract_target_embedding([], FakeConverter(), get_se=lambda *a, **k: (0.0, ''), average=lambda e: e)


class BuildCorpusTest(unittest.TestCase):
    def test_writes_one_wav_and_metadata_row_per_sentence(self):
        tts = FakeTTS()
        converter = FakeConverter()
        sentences = ['The birch canoe slid on the smooth planks.', "It's easy to tell the depth of a well."]

        with tempfile.TemporaryDirectory() as tmp_dir:
            output_dir = Path(tmp_dir) / 'corpus'
            metadata_path = build_corpus(
                sentences, tts=tts, converter=converter,
                source_se='source-embedding', target_se='target-embedding',
                output_dir=output_dir,
            )

            self.assertEqual(metadata_path, output_dir / 'metadata.csv')
            self.assertTrue((output_dir / 'wavs' / '0000.wav').is_file())
            self.assertTrue((output_dir / 'wavs' / '0001.wav').is_file())

            with metadata_path.open(encoding='utf-8', newline='') as metadata_file:
                rows = list(csv.reader(metadata_file, delimiter='|'))
            self.assertEqual(rows, [['0000.wav', sentences[0]], ['0001.wav', sentences[1]]])

        self.assertEqual(tts.synthesized, sentences)
        self.assertEqual(len(converter.calls), 2)
        self.assertEqual(converter.calls[0]['src_se'], 'source-embedding')
        self.assertEqual(converter.calls[0]['tgt_se'], 'target-embedding')

    def test_empty_sentence_list_writes_empty_metadata(self):
        with tempfile.TemporaryDirectory() as tmp_dir:
            output_dir = Path(tmp_dir) / 'corpus'
            metadata_path = build_corpus(
                [], tts=FakeTTS(), converter=FakeConverter(),
                source_se=None, target_se=None, output_dir=output_dir,
            )
            self.assertEqual(metadata_path.read_text(encoding='utf-8'), '')


if __name__ == '__main__':
    unittest.main()
