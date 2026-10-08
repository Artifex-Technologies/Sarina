"""Agent-curated persistent memory, modeled on Hermes Agent's MEMORY.md/USER.md.

Two scopes, mirroring Sarina's existing global-vs-project split for CLAUDE.md
discovery (see ``agent_context.py``):

- ``agent`` -- project-specific facts: environment quirks, conventions,
  discovered workarounds. Stored under the workspace, next to task/plan
  runtime state (``.port_sessions/memory/AGENT_MEMORY.md``), because those
  facts don't generalize across repos.
- ``user`` -- cross-project facts about the operator: preferences,
  communication style, workflow habits. Stored globally
  (``~/.sarina/memory/USER_MEMORY.md``), because those do generalize.

Unlike CLAUDE.md (human-curated, read-only to the agent), both files here are
written by the agent itself through the ``memory_add``/``memory_replace``/
``memory_remove`` tools in ``agent_tools.py``. There is deliberately no
``memory_read`` tool: content is already injected into the system prompt at
session start (``render_prompt_block``), so a read tool would just spend
tokens re-fetching what the model can already see -- matching Hermes'
rationale for the same omission.

Every write is bounded (a single entry cannot exceed ``MAX_ENTRY_CHARS``, a
scope cannot exceed ``MAX_SCOPE_CHARS``) and scanned by
``memory_security.scan_memory_text`` before it is accepted, since accepted
content becomes part of every future session's system prompt.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from pathlib import Path

from .memory_security import scan_memory_text

DEFAULT_AGENT_MEMORY_PATH = Path('.port_sessions') / 'memory' / 'AGENT_MEMORY.md'
DEFAULT_USER_MEMORY_PATH = Path.home() / '.sarina' / 'memory' / 'USER_MEMORY.md'

MAX_ENTRY_CHARS = 1500
MAX_SCOPE_CHARS = 4000

#: How many entries (combined across both scopes) render_relevant_prompt_block
#: keeps. A handful of unrelated entries crowding out what's actually
#: relevant to the current task is the problem this exists to fix; this is
#: not a hard cap on how much memory can be stored, only on how much of it
#: competes for attention in any one task's injected context.
DEFAULT_MAX_RELEVANT_ENTRIES = 8

_WORD_RE = re.compile(r"[a-z0-9']+")

_ENTRY_DELIMITER = '\n---\n'
_SCOPES = ('agent', 'user')

MEMORY_PROMPT_HEADER = (
    'The entries below are notes you saved for yourself in earlier sessions '
    '(the "agent" scope) and notes about the operator carried across projects '
    '(the "user" scope). This is a frozen snapshot captured at the start of '
    'this session -- changes you make with memory_add/memory_replace/'
    'memory_remove during this session take effect for future sessions, not '
    'this one.'
)


class MemoryRuntimeError(RuntimeError):
    """Raised for expected memory failures (capacity, duplicate, not-found, blocked)."""


@dataclass(frozen=True)
class MemoryMutation:
    scope: str
    action: str
    path: str
    before_count: int
    after_count: int


@dataclass
class MemoryRuntime:
    agent_path: Path
    user_path: Path

    @classmethod
    def from_workspace(cls, cwd: Path) -> 'MemoryRuntime':
        agent_path = (cwd.resolve() / DEFAULT_AGENT_MEMORY_PATH).resolve()
        return cls(agent_path=agent_path, user_path=DEFAULT_USER_MEMORY_PATH.resolve())

    def _path(self, scope: str) -> Path:
        if scope == 'agent':
            return self.agent_path
        if scope == 'user':
            return self.user_path
        raise MemoryRuntimeError(f"unknown memory scope '{scope}' (expected 'agent' or 'user')")

    def entries(self, scope: str) -> tuple[str, ...]:
        path = self._path(scope)
        if not path.is_file():
            return ()
        try:
            raw = path.read_text(encoding='utf-8')
        except OSError:
            return ()
        return tuple(entry.strip() for entry in raw.split(_ENTRY_DELIMITER) if entry.strip())

    def add(self, scope: str, content: str) -> MemoryMutation:
        content = content.strip()
        if not content:
            raise MemoryRuntimeError('memory entry content must not be empty')
        if len(content) > MAX_ENTRY_CHARS:
            raise MemoryRuntimeError(
                f'entry is {len(content)} chars, over the {MAX_ENTRY_CHARS}-char limit per entry'
            )
        scan = scan_memory_text(content)
        if scan.blocked:
            raise MemoryRuntimeError(f'entry rejected: {scan.reason}')

        existing = self.entries(scope)
        normalized_new = _normalize(content)
        if any(_normalize(entry) == normalized_new for entry in existing):
            raise MemoryRuntimeError('an identical entry is already stored; nothing to add')

        used = sum(len(entry) for entry in existing)
        if used + len(content) > MAX_SCOPE_CHARS:
            raise MemoryRuntimeError(
                f'memory scope "{scope}" is full ({used}/{MAX_SCOPE_CHARS} chars); '
                'replace or remove an existing entry before adding more'
            )

        updated = (*existing, content)
        self._write(scope, updated)
        return MemoryMutation(
            scope=scope,
            action='add',
            path=str(self._path(scope)),
            before_count=len(existing),
            after_count=len(updated),
        )

    def replace(self, scope: str, old_text: str, content: str) -> MemoryMutation:
        content = content.strip()
        if not content:
            raise MemoryRuntimeError('memory entry content must not be empty')
        if len(content) > MAX_ENTRY_CHARS:
            raise MemoryRuntimeError(
                f'entry is {len(content)} chars, over the {MAX_ENTRY_CHARS}-char limit per entry'
            )
        scan = scan_memory_text(content)
        if scan.blocked:
            raise MemoryRuntimeError(f'entry rejected: {scan.reason}')

        existing = self.entries(scope)
        index = _find_unique_match(existing, old_text)

        used_without_match = sum(len(entry) for i, entry in enumerate(existing) if i != index)
        if used_without_match + len(content) > MAX_SCOPE_CHARS:
            raise MemoryRuntimeError(
                f'replacement would grow memory scope "{scope}" past the '
                f'{MAX_SCOPE_CHARS}-char limit'
            )

        updated = tuple(content if i == index else entry for i, entry in enumerate(existing))
        self._write(scope, updated)
        return MemoryMutation(
            scope=scope,
            action='replace',
            path=str(self._path(scope)),
            before_count=len(existing),
            after_count=len(updated),
        )

    def remove(self, scope: str, old_text: str) -> MemoryMutation:
        existing = self.entries(scope)
        index = _find_unique_match(existing, old_text)
        updated = tuple(entry for i, entry in enumerate(existing) if i != index)
        self._write(scope, updated)
        return MemoryMutation(
            scope=scope,
            action='remove',
            path=str(self._path(scope)),
            before_count=len(existing),
            after_count=len(updated),
        )

    def _write(self, scope: str, entries: tuple[str, ...]) -> None:
        path = self._path(scope)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(_ENTRY_DELIMITER.join(entries), encoding='utf-8')

    def render_prompt_block(self) -> str | None:
        blocks: list[str] = []
        for scope in _SCOPES:
            entries = self.entries(scope)
            if not entries:
                continue
            used = sum(len(entry) for entry in entries)
            lines = [f'## {scope} memory ({used}/{MAX_SCOPE_CHARS} chars, {len(entries)} entries)']
            lines.extend(f'- {entry}' for entry in entries)
            blocks.append('\n'.join(lines))
        if not blocks:
            return None
        return '\n\n'.join([MEMORY_PROMPT_HEADER, *blocks])

    def render_relevant_prompt_block(
        self, task_text: str, *, max_entries: int = DEFAULT_MAX_RELEVANT_ENTRIES,
    ) -> str | None:
        """Like `render_prompt_block`, but ranks entries by lexical overlap
        with `task_text` and keeps only the `max_entries` most relevant
        (combined across both scopes), in their original order.

        Stdlib word-overlap, not embeddings: this project's core agent stays
        standard-library-only, and ranking a handful of short entries
        against one task string doesn't need a model. Falls back to
        `render_prompt_block`'s exact output when the full set already fits
        within `max_entries` -- nothing to rank away, and no reordering risk
        for the common case.
        """
        all_entries: list[tuple[str, str]] = [
            (scope, entry) for scope in _SCOPES for entry in self.entries(scope)
        ]
        if not all_entries:
            return None
        if len(all_entries) <= max_entries:
            return self.render_prompt_block()

        task_tokens = _tokenize(task_text)
        ranked = sorted(
            range(len(all_entries)),
            key=lambda i: (-_relevance_score(all_entries[i][1], task_tokens), i),
        )
        kept_indices = sorted(ranked[:max_entries])
        kept = [all_entries[i] for i in kept_indices]
        omitted_count = len(all_entries) - len(kept)

        blocks: list[str] = []
        for scope in _SCOPES:
            scope_entries = [entry for entry_scope, entry in kept if entry_scope == scope]
            if not scope_entries:
                continue
            total_in_scope = sum(1 for entry_scope, _ in all_entries if entry_scope == scope)
            lines = [f'## {scope} memory (showing {len(scope_entries)} of {total_in_scope} most relevant entries)']
            lines.extend(f'- {entry}' for entry in scope_entries)
            blocks.append('\n'.join(lines))
        header = (
            f'{MEMORY_PROMPT_HEADER}\n\n'
            f'{omitted_count} less-relevant entr{"y" if omitted_count == 1 else "ies"} omitted for this '
            'task; ask the operator if you need the full list.'
        )
        return '\n\n'.join([header, *blocks])

    def render_summary(self) -> str:
        lines = ['# Curated Memory', '']
        for scope in _SCOPES:
            entries = self.entries(scope)
            used = sum(len(entry) for entry in entries)
            lines.append(f'## {scope}')
            lines.append(f'- Path: {self._path(scope)}')
            lines.append(f'- Usage: {used}/{MAX_SCOPE_CHARS} chars, {len(entries)} entries')
            for entry in entries:
                lines.append(f'  - {_snapshot_text(entry)}')
            lines.append('')
        return '\n'.join(lines).rstrip()


def _normalize(text: str) -> str:
    return ' '.join(text.split()).lower()


def _tokenize(text: str) -> set[str]:
    return set(_WORD_RE.findall(text.lower()))


def _relevance_score(entry: str, task_tokens: set[str]) -> int:
    return len(_tokenize(entry) & task_tokens)


def _find_unique_match(entries: tuple[str, ...], old_text: str) -> int:
    old_text = old_text.strip()
    if not old_text:
        raise MemoryRuntimeError('old_text must not be empty')
    matches = [i for i, entry in enumerate(entries) if old_text in entry]
    if not matches:
        raise MemoryRuntimeError('no stored entry contains that text')
    if len(matches) > 1:
        raise MemoryRuntimeError(
            f'that text matches {len(matches)} stored entries; give a more specific substring'
        )
    return matches[0]


def _snapshot_text(text: str, limit: int = 140) -> str:
    normalized = ' '.join(text.split())
    if len(normalized) <= limit:
        return normalized
    return normalized[: limit - 3] + '...'
