"""Text embeddings via ONNX Runtime.

``onnxruntime`` and ``tokenizers`` are optional dependencies (the ``ml`` extra
in ``pyproject.toml``): importing this module never fails, but calling
``EmbeddingModel`` without the extra installed raises ``MLUnavailable`` with an
actionable message.
"""

from __future__ import annotations

import math
from pathlib import Path
from typing import Sequence

try:
    import numpy as np
    import onnxruntime as ort

    _ONNXRUNTIME_AVAILABLE = True
except ImportError:
    np = None  # type: ignore[assignment]
    ort = None  # type: ignore[assignment]
    _ONNXRUNTIME_AVAILABLE = False

try:
    from tokenizers import Tokenizer

    _TOKENIZERS_AVAILABLE = True
except ImportError:
    Tokenizer = None  # type: ignore[assignment]
    _TOKENIZERS_AVAILABLE = False

#: True only when both onnxruntime and tokenizers are importable.
ML_AVAILABLE = _ONNXRUNTIME_AVAILABLE and _TOKENIZERS_AVAILABLE


class MLUnavailable(RuntimeError):
    """The ``ml`` extra is not installed, or a model file is missing."""


def _require_ml() -> None:
    if not ML_AVAILABLE:
        missing = []
        if not _ONNXRUNTIME_AVAILABLE:
            missing.append('onnxruntime')
        if not _TOKENIZERS_AVAILABLE:
            missing.append('tokenizers')
        raise MLUnavailable(
            f'missing {" and ".join(missing)} - install with pip install ".[ml]" '
            '(Arch: python-onnxruntime-cpu, python-tokenizers from the [blackarch] repo)'
        )


class EmbeddingModel:
    """A sentence-embedding model loaded from a local ONNX export.

    Expects ``model_dir`` to contain ``model.onnx`` and ``tokenizer.json`` -
    what ``optimum-cli export onnx --model <sentence-transformer>`` produces.
    No model is downloaded here; the operator provides the directory.
    """

    def __init__(self, model_dir: Path | str, max_length: int = 256):
        _require_ml()
        model_dir = Path(model_dir)
        model_path = model_dir / 'model.onnx'
        tokenizer_path = model_dir / 'tokenizer.json'
        if not model_path.is_file():
            raise MLUnavailable(f'no model.onnx in {model_dir}')
        if not tokenizer_path.is_file():
            raise MLUnavailable(f'no tokenizer.json in {model_dir}')

        self.max_length = max_length
        self._session = ort.InferenceSession(str(model_path), providers=['CPUExecutionProvider'])
        self._tokenizer = Tokenizer.from_file(str(tokenizer_path))
        self._input_names = {inp.name for inp in self._session.get_inputs()}

    def embed(self, texts: Sequence[str]) -> list[list[float]]:
        """Embed a batch of texts. Returns one L2-normalized vector per text."""
        if not texts:
            return []
        self._tokenizer.enable_truncation(max_length=self.max_length)
        self._tokenizer.enable_padding(length=None)
        encodings = self._tokenizer.encode_batch(list(texts))

        input_ids = [encoding.ids for encoding in encodings]
        attention_mask = [encoding.attention_mask for encoding in encodings]
        feed = {
            'input_ids': input_ids,
            'attention_mask': attention_mask,
        }
        if 'token_type_ids' in self._input_names:
            feed['token_type_ids'] = [encoding.type_ids for encoding in encodings]
        feed = {name: value for name, value in feed.items() if name in self._input_names}

        outputs = self._session.run(None, feed)
        token_embeddings = outputs[0]  # (batch, seq_len, hidden)

        return [
            _normalize(_mean_pool(token_embeddings[i], attention_mask[i]))
            for i in range(len(texts))
        ]

    def embed_one(self, text: str) -> list[float]:
        return self.embed([text])[0]


def _mean_pool(token_embeddings, mask: Sequence[int]) -> list[float]:
    """Mean-pool token vectors over real (non-padding) tokens."""
    embeddings = np.asarray(token_embeddings, dtype=np.float64)
    mask_array = np.asarray(mask, dtype=np.float64)
    count = mask_array.sum()
    if count == 0:
        return [0.0] * embeddings.shape[1]
    pooled = (embeddings * mask_array[:, None]).sum(axis=0) / count
    return pooled.tolist()


def _normalize(vector: list[float]) -> list[float]:
    norm = math.sqrt(sum(value * value for value in vector))
    if norm == 0.0:
        return vector
    return [value / norm for value in vector]


def cosine_similarity(a: Sequence[float], b: Sequence[float]) -> float:
    """Cosine similarity between two vectors. Pure stdlib - no ML deps needed."""
    if len(a) != len(b):
        raise ValueError(f'vector length mismatch: {len(a)} vs {len(b)}')
    dot = sum(x * y for x, y in zip(a, b))
    norm_a = math.sqrt(sum(x * x for x in a))
    norm_b = math.sqrt(sum(y * y for y in b))
    if norm_a == 0.0 or norm_b == 0.0:
        return 0.0
    return dot / (norm_a * norm_b)


def rank_by_similarity(
    query: Sequence[float],
    candidates: Sequence[tuple[str, Sequence[float]]],
    top_k: int | None = None,
) -> list[tuple[str, float]]:
    """Rank ``(label, vector)`` candidates by cosine similarity to ``query``.

    Returns ``(label, score)`` pairs, highest similarity first.
    """
    scored = [(label, cosine_similarity(query, vector)) for label, vector in candidates]
    scored.sort(key=lambda pair: pair[1], reverse=True)
    return scored[:top_k] if top_k is not None else scored
