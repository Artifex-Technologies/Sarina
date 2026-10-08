from __future__ import annotations

import tempfile
import unittest
from pathlib import Path

from sarina.analysis_cache import AnalysisCache


class AnalysisCacheTests(unittest.TestCase):
    def test_get_on_empty_cache_returns_none(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            cache = AnalysisCache(Path(tmp_dir) / 'cache.json')
            self.assertIsNone(cache.get('anything'))

    def test_set_then_get_round_trips(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            cache = AnalysisCache(Path(tmp_dir) / 'cache.json')
            cache.set('some content', {'label': 'Graphics', 'score': 0.8})
            self.assertEqual(cache.get('some content'), {'label': 'Graphics', 'score': 0.8})

    def test_different_content_is_a_cache_miss(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            cache = AnalysisCache(Path(tmp_dir) / 'cache.json')
            cache.set('content A', {'label': 'A'})
            self.assertIsNone(cache.get('content B'))

    def test_entries_persist_across_instances(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            path = Path(tmp_dir) / 'cache.json'
            AnalysisCache(path).set('some content', {'label': 'Graphics'})
            reloaded = AnalysisCache(path)
            self.assertEqual(reloaded.get('some content'), {'label': 'Graphics'})

    def test_mismatched_namespace_discards_old_entries(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            path = Path(tmp_dir) / 'cache.json'
            AnalysisCache(path, namespace='v1').set('some content', {'label': 'Graphics'})
            reloaded = AnalysisCache(path, namespace='v2')
            self.assertIsNone(reloaded.get('some content'))

    def test_matching_namespace_keeps_old_entries(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            path = Path(tmp_dir) / 'cache.json'
            AnalysisCache(path, namespace='v1').set('some content', {'label': 'Graphics'})
            reloaded = AnalysisCache(path, namespace='v1')
            self.assertEqual(reloaded.get('some content'), {'label': 'Graphics'})

    def test_corrupt_cache_file_is_treated_as_empty_not_an_error(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            path = Path(tmp_dir) / 'cache.json'
            path.write_text('not valid json{{{', encoding='utf-8')
            cache = AnalysisCache(path)
            self.assertIsNone(cache.get('anything'))
            # Still usable afterward -- a corrupt file doesn't brick the cache.
            cache.set('new content', {'label': 'X'})
            self.assertEqual(cache.get('new content'), {'label': 'X'})

    def test_missing_cache_file_is_treated_as_empty(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            cache = AnalysisCache(Path(tmp_dir) / 'does_not_exist.json')
            self.assertIsNone(cache.get('anything'))

    def test_identical_content_hashes_to_the_same_key_regardless_of_insertion_order(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            path = Path(tmp_dir) / 'cache.json'
            AnalysisCache(path).set('alpha', {'v': 1})
            cache = AnalysisCache(path)
            cache.set('beta', {'v': 2})
            self.assertEqual(cache.get('alpha'), {'v': 1})
            self.assertEqual(cache.get('beta'), {'v': 2})


if __name__ == '__main__':
    unittest.main()
