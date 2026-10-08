"""Tests for tools/voice_clone/clean_checkpoint.py.

No torch, no piper, no real checkpoint: clean_hyperparameters() takes its
accepted-keys set as a plain argument, so it's testable without loading the
real VitsModel class, following build_corpus.py's lazy-import convention.
"""

from __future__ import annotations

import unittest

from tools.voice_clone.clean_checkpoint import clean_hyperparameters


class CleanHyperparametersTest(unittest.TestCase):
    def test_drops_keys_not_in_accepted_set(self):
        hyper_parameters = {
            'sample_rate': 22050,
            'num_symbols': 256,
            'sample_bytes': 2,  # obsolete: not a VitsModel param
            'accelerator': 'gpu',  # obsolete: an old Trainer-level arg
        }
        cleaned = clean_hyperparameters(hyper_parameters, accepted_keys={'sample_rate', 'num_symbols'})
        self.assertEqual(cleaned, {'sample_rate': 22050, 'num_symbols': 256})

    def test_empty_hyperparameters_returns_empty(self):
        self.assertEqual(clean_hyperparameters({}, accepted_keys={'sample_rate'}), {})

    def test_does_not_mutate_the_input_dict(self):
        hyper_parameters = {'sample_rate': 22050, 'sample_bytes': 2}
        clean_hyperparameters(hyper_parameters, accepted_keys={'sample_rate'})
        self.assertIn('sample_bytes', hyper_parameters)


if __name__ == '__main__':
    unittest.main()
