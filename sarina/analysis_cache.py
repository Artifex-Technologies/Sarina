"""Generic content-hash-keyed cache for expensive, deterministic analysis.

Persists ``{content-hash: result}`` as JSON on disk so repeated runs over
unchanged input skip recomputing it -- e.g. ``sarina/desktop/propose.py``'s
semantic classifier, which would otherwise re-run the same ONNX embedding
inference on the same package description every time ``desktop-audit`` runs,
even when nothing about that package changed. Stdlib only: this project's
core agent doesn't take on new dependencies for a convenience like this.

Invalidated wholesale, not entry-by-entry, by a ``namespace`` fingerprint the
caller supplies -- if whatever produces the cached values changes shape
(e.g. the category prototype set a classifier scores against), the whole
cache is discarded rather than serving stale results under a namespace that
no longer matches what's being cached.
"""

from __future__ import annotations

import hashlib
import json
from pathlib import Path
from typing import Any

DEFAULT_CACHE_DIR = Path.home() / '.sarina' / 'cache'


class AnalysisCache:
    def __init__(self, path: Path, *, namespace: str = '') -> None:
        self._path = path
        self._namespace = namespace
        self._entries: dict[str, Any] = self._load()

    def _load(self) -> dict[str, Any]:
        if not self._path.is_file():
            return {}
        try:
            payload = json.loads(self._path.read_text(encoding='utf-8'))
        except (OSError, json.JSONDecodeError):
            return {}
        if not isinstance(payload, dict) or payload.get('namespace') != self._namespace:
            return {}
        entries = payload.get('entries')
        return entries if isinstance(entries, dict) else {}

    def get(self, content: str) -> Any | None:
        return self._entries.get(_hash_key(content))

    def set(self, content: str, value: Any) -> None:
        self._entries[_hash_key(content)] = value
        self._save()

    def _save(self) -> None:
        self._path.parent.mkdir(parents=True, exist_ok=True)
        payload = {'namespace': self._namespace, 'entries': self._entries}
        self._path.write_text(json.dumps(payload, ensure_ascii=True), encoding='utf-8')


def _hash_key(content: str) -> str:
    return hashlib.sha256(content.encode('utf-8')).hexdigest()
