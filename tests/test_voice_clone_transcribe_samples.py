"""Tests for tools/voice_clone/transcribe_samples.py.

No faster-whisper, no pydub audio decoding, no real model: transcribe_segments()
and build_real_corpus() take a duck-typed model (and, for the corpus builder,
an injected audio loader) so the filtering/glue logic is testable without any
of it, following build_corpus.py's conventions.
"""

from __future__ import annotations

import csv
import tempfile
import unittest
from dataclasses import dataclass
from pathlib import Path

from tools.voice_clone.transcribe_samples import (
    TARGET_DBFS,
    _filter_usable_segments,
    _merge_close_segments,
    _next_raw_boundary,
    _normalize_loudness,
    _padded_bounds,
    _trim_trailing_low_confidence_words,
    build_real_corpus,
    transcribe_segments,
)


@dataclass
class FakeWord:
    start: float
    end: float
    word: str
    probability: float = 1.0


@dataclass
class FakeSegment:
    start: float
    end: float
    text: str
    no_speech_prob: float = 0.0
    words: list[FakeWord] | None = None


class FakeWhisperModel:
    def __init__(self, segments: list[FakeSegment]):
        self._segments = segments

    def transcribe(self, audio_path, *, vad_filter=True, word_timestamps=True):
        return self._segments, object()


class FakeAudioClip:
    """Stands in for a pydub AudioSegment slice: records what it was asked
    to export, writes nothing real."""

    def __init__(self, recorder: list, start_ms: int, end_ms: int, dBFS: float = TARGET_DBFS):
        self._recorder = recorder
        self._start_ms = start_ms
        self._end_ms = end_ms
        self.dBFS = dBFS

    def apply_gain(self, delta: float) -> 'FakeAudioClip':
        return FakeAudioClip(self._recorder, self._start_ms, self._end_ms, dBFS=self.dBFS + delta)

    def export(self, path, format='wav'):
        self._recorder.append((self._start_ms, self._end_ms, path))
        Path(path).write_bytes(b'RIFF-fake-wav')


class FakeAudio:
    def __init__(self, recorder: list, duration_ms: int = 100_000):
        self._recorder = recorder
        self._duration_ms = duration_ms

    def __len__(self):
        return self._duration_ms

    def __getitem__(self, item: slice):
        return FakeAudioClip(self._recorder, item.start, item.stop)


class TranscribeSegmentsTest(unittest.TestCase):
    def test_keeps_only_usable_segments(self):
        model = FakeWhisperModel([
            FakeSegment(start=0.0, end=3.0, text='A normal sentence.'),
            FakeSegment(start=3.0, end=3.5, text='Too short'),  # < MIN_SEGMENT_SECONDS
            FakeSegment(start=3.5, end=30.0, text='Way too long' * 5),  # > MAX_SEGMENT_SECONDS
            FakeSegment(start=30.0, end=33.0, text='   '),  # blank after strip
            FakeSegment(start=33.0, end=36.0, text='Probably laughter', no_speech_prob=0.9),
            FakeSegment(start=36.0, end=40.0, text='Another good one.'),
        ])
        usable = transcribe_segments(Path('sample.mp3'), model=model)
        self.assertEqual(usable, [(0.0, 3.0, 'A normal sentence.'), (36.0, 40.0, 'Another good one.')])

    def test_no_usable_segments_returns_empty_list(self):
        model = FakeWhisperModel([FakeSegment(start=0.0, end=0.5, text='hi')])
        self.assertEqual(transcribe_segments(Path('sample.mp3'), model=model), [])


@dataclass
class FakeGainClip:
    dBFS: float
    applied_gain: float | None = None

    def apply_gain(self, delta: float) -> 'FakeGainClip':
        return FakeGainClip(dBFS=self.dBFS + delta, applied_gain=delta)


class NormalizeLoudnessTest(unittest.TestCase):
    def test_boosts_quiet_clip_to_target(self):
        clip = _normalize_loudness(FakeGainClip(dBFS=-30.0), target_dbfs=-20.0)
        self.assertAlmostEqual(clip.applied_gain, 10.0)
        self.assertAlmostEqual(clip.dBFS, -20.0)

    def test_attenuates_loud_clip_to_target(self):
        clip = _normalize_loudness(FakeGainClip(dBFS=-10.0), target_dbfs=-20.0)
        self.assertAlmostEqual(clip.applied_gain, -10.0)
        self.assertAlmostEqual(clip.dBFS, -20.0)

    def test_silent_clip_is_left_unchanged(self):
        silent = FakeGainClip(dBFS=float('-inf'))
        self.assertIs(_normalize_loudness(silent, target_dbfs=-20.0), silent)


class MergeCloseSegmentsTest(unittest.TestCase):
    def test_merges_mid_sentence_fragments_closer_than_the_gap_threshold(self):
        segments = [(0.0, 2.0, 'and then you watch how she moves,'), (2.2, 4.0, 'it all makes sense.')]
        merged = _merge_close_segments(segments, gap_seconds=0.5, max_duration=20.0)
        self.assertEqual(merged, [(0.0, 4.0, 'and then you watch how she moves, it all makes sense.')])

    def test_does_not_merge_after_a_complete_sentence_even_with_a_small_gap(self):
        # This is the fix for the real bug found by direct measurement: short
        # pauses between separate, already-complete sentences were being
        # merged all the way to the max_duration cap, producing training
        # clips that cut off mid-clause regardless of where that landed.
        # A sentence-ending segment is always a safe place to stop, however
        # small the pause after it is.
        segments = [(0.0, 2.0, 'First sentence.'), (2.2, 4.0, 'Second sentence.')]
        merged = _merge_close_segments(segments, gap_seconds=0.5, max_duration=20.0)
        self.assertEqual(merged, segments)

    def test_keeps_segments_separate_when_gap_is_large_enough(self):
        segments = [(0.0, 2.0, 'and then'), (3.0, 5.0, 'she moved on.')]
        merged = _merge_close_segments(segments, gap_seconds=0.5, max_duration=20.0)
        self.assertEqual(merged, segments)

    def test_does_not_merge_past_the_max_duration_cap(self):
        segments = [(0.0, 15.0, 'a long unfinished clause that goes on'), (15.1, 25.0, 'and finally ends.')]
        merged = _merge_close_segments(segments, gap_seconds=0.5, max_duration=20.0)
        # Combined duration (25.0) would exceed max_duration, so they stay
        # separate even though the gap is small and the text is unfinished.
        self.assertEqual(merged, segments)

    def test_chains_multiple_merges_in_a_row(self):
        segments = [(0.0, 1.0, 'A'), (1.1, 2.0, 'B'), (2.1, 3.0, 'C.')]
        merged = _merge_close_segments(segments, gap_seconds=0.5, max_duration=20.0)
        self.assertEqual(merged, [(0.0, 3.0, 'A B C.')])

    def test_empty_input_returns_empty_list(self):
        self.assertEqual(_merge_close_segments([]), [])

    def test_single_segment_is_unchanged(self):
        segments = [(0.0, 2.0, 'Only one.')]
        self.assertEqual(_merge_close_segments(segments), segments)


class PaddedBoundsTest(unittest.TestCase):
    def test_widens_both_sides_when_room_allows(self):
        start, end = _padded_bounds(10.0, 12.0, next_start=None, audio_duration=100.0)
        self.assertAlmostEqual(start, 9.95)
        self.assertAlmostEqual(end, 12.25)

    def test_leading_pad_clamped_at_zero(self):
        start, _end = _padded_bounds(0.02, 2.0, next_start=None, audio_duration=100.0)
        self.assertEqual(start, 0.0)

    def test_trailing_pad_clamped_to_audio_duration(self):
        _start, end = _padded_bounds(8.0, 9.9, next_start=None, audio_duration=10.0)
        self.assertEqual(end, 10.0)

    def test_trailing_pad_clamped_to_next_segment_start(self):
        _start, end = _padded_bounds(8.0, 9.0, next_start=9.1, audio_duration=100.0)
        self.assertEqual(end, 9.1)


class NextRawBoundaryTest(unittest.TestCase):
    def test_uses_a_filtered_out_segment_as_the_boundary(self):
        # A laugh right after this segment (e.g. no_speech_prob too high to
        # be "usable") was dropped by _filter_usable_segments and so is
        # invisible to the next *usable* segment's start -- but the trailing
        # pad must still stop before it, not bleed into it. This is the fix
        # for training clips ending in a stray "ha" that wasn't in the text.
        boundary_starts = [0.0, 3.1, 10.0]
        boundary = _next_raw_boundary(3.0, boundary_starts, next_usable_start=10.0)
        self.assertEqual(boundary, 3.1)

    def test_falls_back_to_next_usable_start_when_nothing_raw_is_closer(self):
        boundary_starts = [0.0, 10.0]
        boundary = _next_raw_boundary(3.0, boundary_starts, next_usable_start=10.0)
        self.assertEqual(boundary, 10.0)

    def test_no_next_usable_segment_still_clamps_to_a_raw_boundary(self):
        boundary_starts = [0.0, 3.1]
        boundary = _next_raw_boundary(3.0, boundary_starts, next_usable_start=None)
        self.assertEqual(boundary, 3.1)

    def test_returns_none_when_nothing_follows(self):
        boundary_starts = [0.0]
        self.assertIsNone(_next_raw_boundary(3.0, boundary_starts, next_usable_start=None))


class TrimTrailingLowConfidenceWordsTest(unittest.TestCase):
    def test_no_words_falls_back_to_segment_bounds_unchanged(self):
        segment = FakeSegment(start=0.0, end=3.0, text=' A normal sentence. ')
        end, text, dropped = _trim_trailing_low_confidence_words(segment)
        self.assertEqual(end, 3.0)
        self.assertEqual(text, 'A normal sentence.')
        self.assertEqual(dropped, [])

    def test_drops_a_single_low_confidence_trailing_word(self):
        # The real case this fixes: Whisper hears a chuckle right after a
        # clean sentence and transcribes it as an actual word ("laugh"),
        # which the segment-level no_speech_prob check doesn't catch because
        # the segment overall is confident, real speech. The word's own
        # probability is reliably much lower, so dropping low-confidence
        # trailing words catches it.
        words = [
            FakeWord(start=0.0, end=0.5, word='you', probability=0.98),
            FakeWord(start=0.5, end=1.0, word='can', probability=0.97),
            FakeWord(start=1.0, end=1.5, word='see', probability=0.95),
            FakeWord(start=1.5, end=2.2, word='laugh', probability=0.12),
        ]
        segment = FakeSegment(start=0.0, end=2.2, text='you can see laugh', words=words)
        end, text, dropped = _trim_trailing_low_confidence_words(segment)
        self.assertEqual(end, 1.5)
        self.assertEqual(text, 'you can see')
        self.assertEqual(dropped, [1.5])

    def test_drops_multiple_consecutive_low_confidence_trailing_words(self):
        words = [
            FakeWord(start=0.0, end=0.5, word='hello', probability=0.99),
            FakeWord(start=0.5, end=0.8, word='ha', probability=0.2),
            FakeWord(start=0.8, end=1.0, word='ha', probability=0.1),
        ]
        segment = FakeSegment(start=0.0, end=1.0, text='hello ha ha', words=words)
        end, text, dropped = _trim_trailing_low_confidence_words(segment)
        self.assertEqual(end, 0.5)
        self.assertEqual(text, 'hello')
        self.assertEqual(dropped, [0.8, 0.5])

    def test_all_words_low_confidence_is_unusable(self):
        words = [FakeWord(start=0.0, end=0.3, word='ha', probability=0.1)]
        segment = FakeSegment(start=0.0, end=0.3, text='ha', words=words)
        self.assertIsNone(_trim_trailing_low_confidence_words(segment))

    def test_leaves_confident_segment_untouched(self):
        words = [
            FakeWord(start=0.0, end=1.0, word='All', probability=0.99),
            FakeWord(start=1.0, end=2.0, word='good.', probability=0.98),
        ]
        segment = FakeSegment(start=0.0, end=2.0, text='All good.', words=words)
        end, text, dropped = _trim_trailing_low_confidence_words(segment)
        self.assertEqual(end, 2.0)
        self.assertEqual(text, 'All good.')
        self.assertEqual(dropped, [])


class FilterUsableSegmentsTest(unittest.TestCase):
    def test_surfaces_dropped_word_starts_for_boundary_clamping(self):
        words = [
            FakeWord(start=0.0, end=2.0, word='A normal sentence.', probability=0.99),
            FakeWord(start=2.0, end=2.3, word='laugh', probability=0.1),
        ]
        segment = FakeSegment(start=0.0, end=2.3, text='A normal sentence. laugh', words=words)
        usable, dropped_word_starts = _filter_usable_segments([segment])
        self.assertEqual(usable, [(0.0, 2.0, 'A normal sentence.')])
        self.assertEqual(dropped_word_starts, [2.0])


class BuildRealCorpusTest(unittest.TestCase):
    def test_writes_one_wav_and_metadata_row_per_usable_segment(self):
        model = FakeWhisperModel([
            FakeSegment(start=1.0, end=4.0, text='First clip.'),
            FakeSegment(start=5.0, end=8.0, text='Second clip.'),
        ])
        recorder: list = []

        with tempfile.TemporaryDirectory() as tmp_dir:
            output_dir = Path(tmp_dir) / 'corpus_real'
            metadata_path = build_real_corpus(
                [Path('sampleA.mp3')], model=model, output_dir=output_dir,
                load_audio=lambda path: FakeAudio(recorder),
            )

            self.assertEqual(metadata_path, output_dir / 'metadata.csv')
            with metadata_path.open(encoding='utf-8', newline='') as f:
                rows = list(csv.reader(f, delimiter='|'))
            self.assertEqual(rows, [['0000.wav', 'First clip.'], ['0001.wav', 'Second clip.']])

        # Each clip is widened by LEADING_PAD_SECONDS/TRAILING_PAD_SECONDS
        # (0.05s/0.25s) to avoid training on Whisper's clipped boundaries --
        # clamped so segment 1's trailing pad can't run into segment 2 (its
        # next_start), matching _padded_bounds.
        self.assertEqual(recorder, [
            (950, 4250, str(output_dir / 'wavs' / '0000.wav')),
            (4950, 8250, str(output_dir / 'wavs' / '0001.wav')),
        ])

    def test_trailing_pad_does_not_bleed_into_a_filtered_out_laugh(self):
        # Regression test for the real bug: a giggle right after a sentence
        # has a high no_speech_prob and is correctly dropped as its own
        # training example, but the sentence's own trailing pad used to be
        # clamped only against the next *kept* segment -- which, with the
        # laugh gone, could be seconds away -- so the pad reached straight
        # into the laugh. The clip's audio then had a trailing "ha" with no
        # corresponding text. Clamping against every raw segment (laugh
        # included) fixes this: the pad now stops at 3.1s (where the laugh
        # starts), not 3.25s (0.25s unconditional pad) or 10.0s (next kept
        # segment).
        model = FakeWhisperModel([
            FakeSegment(start=0.0, end=3.0, text='A normal sentence.'),
            FakeSegment(start=3.1, end=3.4, text='', no_speech_prob=0.95),  # the giggle
            FakeSegment(start=10.0, end=13.0, text='Another good one.'),
        ])
        recorder: list = []

        with tempfile.TemporaryDirectory() as tmp_dir:
            output_dir = Path(tmp_dir) / 'corpus_real'
            build_real_corpus(
                [Path('sampleA.mp3')], model=model, output_dir=output_dir,
                load_audio=lambda path: FakeAudio(recorder),
            )

        first_clip_end_ms = recorder[0][1]
        self.assertEqual(first_clip_end_ms, 3100)

    def test_sample_with_no_usable_segments_is_skipped_without_error(self):
        model = FakeWhisperModel([])
        with tempfile.TemporaryDirectory() as tmp_dir:
            output_dir = Path(tmp_dir) / 'corpus_real'
            metadata_path = build_real_corpus(
                [Path('silent.mp3')], model=model, output_dir=output_dir,
                load_audio=lambda path: FakeAudio([]),
            )
            self.assertEqual(metadata_path.read_text(encoding='utf-8'), '')

    def test_indices_continue_across_multiple_samples(self):
        model = FakeWhisperModel([FakeSegment(start=0.0, end=2.0, text='Clip.')])
        with tempfile.TemporaryDirectory() as tmp_dir:
            output_dir = Path(tmp_dir) / 'corpus_real'
            metadata_path = build_real_corpus(
                [Path('a.mp3'), Path('b.mp3')], model=model, output_dir=output_dir,
                load_audio=lambda path: FakeAudio([]),
            )
            with metadata_path.open(encoding='utf-8', newline='') as f:
                rows = list(csv.reader(f, delimiter='|'))
            self.assertEqual([row[0] for row in rows], ['0000.wav', '0001.wav'])


if __name__ == '__main__':
    unittest.main()
