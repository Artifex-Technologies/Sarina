"""Semantic grouping of Chymaera_Zero_App captures (for ``chymaera-analyze --model-dir``).

Works only on what ``capture.list`` provides: each record's ``name``, ``type``
and its ``meta`` JSON string. Greedy single-link grouping: simple, deterministic.
"""

from __future__ import annotations

import json
from typing import Any, Sequence

from .embeddings import cosine_similarity

_META_KEYS = ('deviceType', 'protocol', 'preset', 'uid', 'frequency')


def _parse_meta(capture: dict[str, Any]) -> dict[str, Any]:
    raw = capture.get('meta')
    if not raw:
        return {}
    try:
        parsed = json.loads(raw)
    except (json.JSONDecodeError, TypeError):
        return {}
    return parsed if isinstance(parsed, dict) else {}


def capture_text(capture: dict[str, Any]) -> str:
    """Text embedded for one capture: name, type, and the meta fields worth comparing."""
    meta = _parse_meta(capture)
    parts = [str(capture.get('name', '')), str(capture.get('type', ''))]
    for key in _META_KEYS:
        if key in meta:
            parts.append(f'{key}={meta[key]}')
    return ' '.join(part for part in parts if part)


def group_vectors(
    names: Sequence[str], vectors: Sequence[Sequence[float]], threshold: float,
) -> list[list[str]]:
    """Group names whose vectors are within ``threshold`` cosine similarity of a group's first member.

    Only groups of two or more are returned, in first-seen order.
    """
    assigned = [False] * len(names)
    groups: list[list[str]] = []
    for i in range(len(names)):
        if assigned[i]:
            continue
        group = [names[i]]
        assigned[i] = True
        for j in range(i + 1, len(names)):
            if not assigned[j] and cosine_similarity(vectors[i], vectors[j]) >= threshold:
                group.append(names[j])
                assigned[j] = True
        if len(group) > 1:
            groups.append(group)
    return groups


def group_captures(captures: list[dict[str, Any]], model: Any, threshold: float) -> list[list[str]]:
    """Group ``captures`` by semantic similarity using ``model.embed``."""
    if not captures:
        return []
    names = [str(c.get('name', '?')) for c in captures]
    vectors = model.embed([capture_text(c) for c in captures])
    return group_vectors(names, vectors, threshold)
