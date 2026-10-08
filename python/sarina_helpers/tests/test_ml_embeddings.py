"""Tests for sarina_helpers.embeddings.

The cosine/ranking math is pure stdlib and tested directly. EmbeddingModel
itself needs a real ONNX model file, which the test suite must not depend on
(no network, no model backend) - so EmbeddingModel construction against a
missing/empty directory is tested for its error path only. A real model is
exercised manually; see docs/ECOSYSTEM.md.
"""

import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from sarina_helpers.embeddings import (
    EmbeddingModel,
    MLUnavailable,
    _mean_pool,
    cosine_similarity,
    rank_by_similarity,
)


class MeanPoolTest(unittest.TestCase):
    def test_averages_only_unmasked_tokens(self) -> None:
        token_embeddings = [
            [1.0, 2.0],
            [3.0, 4.0],
            [100.0, 100.0],  # padding token, must be excluded by the mask
        ]
        mask = [1, 1, 0]
        pooled = _mean_pool(token_embeddings, mask)
        self.assertEqual(len(pooled), 2)
        self.assertAlmostEqual(pooled[0], 2.0)
        self.assertAlmostEqual(pooled[1], 3.0)

    def test_single_unmasked_token_returns_it_unchanged(self) -> None:
        pooled = _mean_pool([[5.0, -1.0, 0.5]], [1])
        self.assertAlmostEqual(pooled[0], 5.0)
        self.assertAlmostEqual(pooled[1], -1.0)
        self.assertAlmostEqual(pooled[2], 0.5)

    def test_all_masked_out_returns_zero_vector_of_hidden_size(self) -> None:
        pooled = _mean_pool([[1.0, 2.0, 3.0], [4.0, 5.0, 6.0]], [0, 0])
        self.assertEqual(pooled, [0.0, 0.0, 0.0])

    def test_returns_plain_python_floats_not_numpy_scalars(self) -> None:
        pooled = _mean_pool([[1.0, 2.0]], [1])
        for value in pooled:
            self.assertIsInstance(value, float)


class CosineSimilarityTest(unittest.TestCase):
    def test_identical_vectors_are_similarity_one(self):
        self.assertAlmostEqual(cosine_similarity([1.0, 2.0, 3.0], [1.0, 2.0, 3.0]), 1.0)

    def test_opposite_vectors_are_similarity_negative_one(self):
        self.assertAlmostEqual(cosine_similarity([1.0, 0.0], [-1.0, 0.0]), -1.0)

    def test_orthogonal_vectors_are_similarity_zero(self):
        self.assertAlmostEqual(cosine_similarity([1.0, 0.0], [0.0, 1.0]), 0.0)

    def test_scale_invariant(self):
        a = cosine_similarity([1.0, 2.0], [3.0, 4.0])
        b = cosine_similarity([10.0, 20.0], [3.0, 4.0])
        self.assertAlmostEqual(a, b)

    def test_zero_vector_is_similarity_zero_not_a_crash(self):
        self.assertEqual(cosine_similarity([0.0, 0.0], [1.0, 1.0]), 0.0)

    def test_mismatched_length_raises(self):
        with self.assertRaises(ValueError):
            cosine_similarity([1.0, 2.0], [1.0, 2.0, 3.0])


class RankBySimilarityTest(unittest.TestCase):
    def test_orders_highest_first(self):
        query = [1.0, 0.0]
        candidates = [
            ('opposite', [-1.0, 0.0]),
            ('same', [1.0, 0.0]),
            ('orthogonal', [0.0, 1.0]),
        ]
        ranked = rank_by_similarity(query, candidates)
        self.assertEqual([label for label, _score in ranked], ['same', 'orthogonal', 'opposite'])

    def test_top_k_truncates(self):
        query = [1.0, 0.0]
        candidates = [(str(i), [1.0, 0.0]) for i in range(5)]
        self.assertEqual(len(rank_by_similarity(query, candidates, top_k=2)), 2)

    def test_empty_candidates(self):
        self.assertEqual(rank_by_similarity([1.0, 0.0], []), [])


class EmbeddingModelErrorPathTest(unittest.TestCase):
    """No real model is loaded here - only the fail-fast paths are checked."""

    def test_missing_ml_extra_names_both_packages(self):
        # Simulates the environment the standard test suite actually runs in:
        # the `ml` extra is optional and not installed by default.
        with patch('sarina_helpers.embeddings._ONNXRUNTIME_AVAILABLE', False), \
             patch('sarina_helpers.embeddings._TOKENIZERS_AVAILABLE', False), \
             patch('sarina_helpers.embeddings.ML_AVAILABLE', False):
            with self.assertRaises(MLUnavailable) as ctx:
                EmbeddingModel(Path('/does/not/matter'))
        self.assertIn('onnxruntime', str(ctx.exception))
        self.assertIn('tokenizers', str(ctx.exception))

    def test_missing_model_file_raises_ml_unavailable(self):
        with patch('sarina_helpers.embeddings.ML_AVAILABLE', True):
            with tempfile.TemporaryDirectory() as tmp:
                with self.assertRaises(MLUnavailable) as ctx:
                    EmbeddingModel(Path(tmp))
        self.assertIn('model.onnx', str(ctx.exception))

    def test_missing_directory_raises_ml_unavailable(self):
        with patch('sarina_helpers.embeddings.ML_AVAILABLE', True):
            with self.assertRaises(MLUnavailable):
                EmbeddingModel(Path('/does/not/exist/at/all'))


if __name__ == '__main__':
    unittest.main()
