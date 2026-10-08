"""Semantic search over saved agent sessions (``sarina memory-search``).

Reads the Go session files (``<dir>/<session_id>.json``, ``messages`` an array of
objects with ``role``/``content``). Re-embeds excerpts on every call: session
directories are single-operator scale, so no index file is kept.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from .embeddings import rank_by_similarity

#: Per-message content cap before embedding: excerpts find the right session,
#: they do not reproduce it.
EXCERPT_CHAR_LIMIT = 800

#: Injected context, not something a user typed. A user turn is sometimes only
#: one of these, with the real ask in the next message, so strip then check
#: what is left.
_SYSTEM_REMINDER = re.compile(r'<system-reminder>.*?</system-reminder>', re.DOTALL)


@dataclass(frozen=True)
class SessionExcerpt:
    session_id: str
    task: str
    outcome: str

    @property
    def text(self) -> str:
        return f'{self.task}\n{self.outcome}'.strip()


def _text(message: dict[str, Any]) -> str:
    content = message.get('content')
    return content if isinstance(content, str) else ''


def excerpt_from_messages(session_id: str, messages: list[Any]) -> SessionExcerpt:
    """First real user ask (the task) and last assistant text (the outcome).

    A user message that is only a ``<system-reminder>`` is skipped, not taken as the task.
    """
    task = outcome = ''
    for message in messages:
        if not isinstance(message, dict):
            continue
        role = message.get('role')
        if role == 'user' and not task:
            task = _SYSTEM_REMINDER.sub('', _text(message)).strip()
        elif role == 'assistant':
            text = _text(message)
            if text:
                outcome = text
    return SessionExcerpt(session_id, task[:EXCERPT_CHAR_LIMIT], outcome[:EXCERPT_CHAR_LIMIT])


def load_excerpt(path: Path) -> SessionExcerpt:
    data = json.loads(path.read_text(encoding='utf-8'))
    messages = data.get('messages')
    if not isinstance(messages, list):
        raise ValueError('no messages array')
    session_id = data.get('session_id')
    return excerpt_from_messages(session_id if isinstance(session_id, str) and session_id else path.stem, messages)


def index_sessions(directory: Path | str) -> list[SessionExcerpt]:
    """Excerpt every session in ``directory``, newest first; unreadable files are skipped."""
    target = Path(directory)
    if not target.is_dir():
        return []
    files = sorted(target.glob('*.json'), key=lambda p: p.stat().st_mtime, reverse=True)
    excerpts = []
    for path in files:
        try:
            excerpts.append(load_excerpt(path))
        except (OSError, ValueError, AttributeError):
            continue
    return excerpts


def search_sessions(
    query: str, model: Any, directory: Path | str, top_k: int = 10,
) -> list[tuple[SessionExcerpt, float]]:
    excerpts = index_sessions(directory)
    if not excerpts:
        return []
    query_vector = model.embed_one(query)
    vectors = model.embed([e.text or e.session_id for e in excerpts])
    by_id = {e.session_id: e for e in excerpts}
    ranked = rank_by_similarity(query_vector, [(e.session_id, v) for e, v in zip(excerpts, vectors)], top_k=top_k)
    return [(by_id[sid], score) for sid, score in ranked]


def format_results(results: list[tuple[SessionExcerpt, float]]) -> str:
    lines = []
    for excerpt, score in results:
        lines.append(f'{score:.3f}  {excerpt.session_id}')
        lines.append(f'  task:    {excerpt.task}')
        lines.append(f'  outcome: {excerpt.outcome}')
    return '\n'.join(lines)
