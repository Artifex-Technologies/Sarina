"""Periodic memory review: an LLM proposes durable memory entries from a past session.

Mirrors Hermes Agent's "nudge" -- a background pass that replays a
conversation and identifies things worth remembering -- but user-triggered
(``sarina memory-review <session_id>``) rather than fired automatically after
every turn. That's a deliberate simplification, not a missing feature: an
automatic post-session hook would need to alter ``agent_runtime.py``'s hot
path and spawn a detached model call after every single run, for a benefit
(catching things the operator forgot to ask the agent to remember) that a
periodic/cron-triggered review captures just as well with far less surface
area. The `loop`/`schedule` machinery already in this repo's ecosystem is the
natural way to run this on a cadence if that's wanted later.

Every proposal lands in a pending queue (``MemoryReviewQueue``) rather than
being written straight to memory -- matching Hermes' ``write_approval: true``
gate, and required by this project's own instinct about autonomous writes.
Proposals are scanned with ``memory_security.scan_memory_text`` at proposal
time (so a human reviewing the queue never sees a proposal already known to
be unsafe) and scanned *again* at approval time inside
``MemoryRuntime.add``/``replace``/``remove`` -- defense in depth, since those
are the same entry points an interactive tool call uses.
"""

from __future__ import annotations

import json
import re
from dataclasses import asdict, dataclass, field
from datetime import datetime, timezone
from pathlib import Path
from typing import Any, Protocol
from uuid import uuid4

from .memory_runtime import MemoryMutation, MemoryRuntime, MemoryRuntimeError
from .memory_security import scan_memory_text

DEFAULT_REVIEW_QUEUE_PATH = Path('.port_sessions') / 'memory' / 'pending_review.json'
MAX_PROPOSALS_PER_REVIEW = 5
MAX_TRANSCRIPT_CHARS = 8000

REVIEW_SYSTEM_PROMPT = """\
You are reviewing a finished coding-agent session to decide what, if \
anything, is worth remembering for future sessions.

Propose at most 5 durable memory entries. A durable entry is a fact that \
will still be true and useful in a different session: an environment quirk, \
a project convention, a workaround for a specific bug, or a stated user \
preference. Do NOT propose entries for: task-specific details that won't \
recur, anything already obvious from reading the code, or instructions to \
change your own behavior.

Respond with ONLY a JSON array (no prose, no markdown fences). Each item:
{"scope": "agent"|"user", "action": "add"|"replace"|"remove", \
"old_text": string or null, "content": string or null, "rationale": string}

- scope "agent": project-specific facts. scope "user": facts about the \
operator that hold across projects.
- action "add": content is the new entry. action "replace": old_text is a \
substring uniquely identifying an existing entry to update, content is its \
replacement. action "remove": old_text identifies an entry to delete, \
content should be null.
- If nothing is worth remembering, respond with an empty array: []
"""


class ChatCompleter(Protocol):
    def complete(self, messages: list[dict[str, Any]], tools: list[dict[str, Any]]) -> Any: ...


@dataclass(frozen=True)
class ProposedEntry:
    entry_id: str
    session_id: str
    scope: str
    action: str
    old_text: str | None
    content: str | None
    rationale: str
    status: str  # 'pending' | 'approved' | 'rejected' | 'blocked'
    blocked_reason: str | None = None
    created_at: str = field(default_factory=lambda: datetime.now(timezone.utc).isoformat())

    @classmethod
    def from_dict(cls, payload: dict[str, Any]) -> 'ProposedEntry':
        return cls(
            entry_id=str(payload.get('entry_id') or ''),
            session_id=str(payload.get('session_id') or ''),
            scope=str(payload.get('scope') or ''),
            action=str(payload.get('action') or ''),
            old_text=(str(payload['old_text']) if payload.get('old_text') is not None else None),
            content=(str(payload['content']) if payload.get('content') is not None else None),
            rationale=str(payload.get('rationale') or ''),
            status=str(payload.get('status') or 'pending'),
            blocked_reason=(
                str(payload['blocked_reason']) if payload.get('blocked_reason') is not None else None
            ),
            created_at=str(payload.get('created_at') or ''),
        )


def build_transcript(messages: list[dict[str, Any]], max_chars: int = MAX_TRANSCRIPT_CHARS) -> str:
    lines: list[str] = []
    for message in messages:
        role = message.get('role')
        content = message.get('content')
        if not isinstance(role, str) or not isinstance(content, str) or not content.strip():
            continue
        lines.append(f'{role}: {content.strip()}')
    transcript = '\n\n'.join(lines)
    if len(transcript) <= max_chars:
        return transcript
    # Keep the most recent content -- it's more likely to reflect the
    # session's actual outcome than its opening turns.
    return '...(truncated)...\n' + transcript[-max_chars:]


def propose_memory_entries(
    session_id: str,
    messages: list[dict[str, Any]],
    client: ChatCompleter,
) -> list[ProposedEntry]:
    transcript = build_transcript(messages)
    if not transcript.strip():
        return []

    api_messages = [
        {'role': 'system', 'content': REVIEW_SYSTEM_PROMPT},
        {'role': 'user', 'content': transcript},
    ]
    turn = client.complete(api_messages, tools=[])
    raw_items = _extract_json_array(turn.content or '')
    if raw_items is None:
        return []

    proposals: list[ProposedEntry] = []
    for item in raw_items[:MAX_PROPOSALS_PER_REVIEW]:
        proposal = _build_proposal(session_id, item)
        if proposal is not None:
            proposals.append(proposal)
    return proposals


def _build_proposal(session_id: str, item: Any) -> ProposedEntry | None:
    if not isinstance(item, dict):
        return None
    scope = item.get('scope')
    action = item.get('action')
    if scope not in ('agent', 'user') or action not in ('add', 'replace', 'remove'):
        return None
    old_text = item.get('old_text')
    content = item.get('content')
    old_text = old_text.strip() if isinstance(old_text, str) and old_text.strip() else None
    content = content.strip() if isinstance(content, str) and content.strip() else None
    rationale = str(item.get('rationale') or '').strip()

    if action in ('add', 'replace') and not content:
        return None
    if action in ('replace', 'remove') and not old_text:
        return None

    blocked_reason = None
    if content is not None:
        scan = scan_memory_text(content)
        if scan.blocked:
            blocked_reason = scan.reason

    return ProposedEntry(
        entry_id=f'mem_{uuid4().hex[:12]}',
        session_id=session_id,
        scope=scope,
        action=action,
        old_text=old_text,
        content=content,
        rationale=rationale,
        status='blocked' if blocked_reason else 'pending',
        blocked_reason=blocked_reason,
    )


def _extract_json_array(text: str) -> list[Any] | None:
    text = text.strip()
    candidates = [text]
    fence_match = re.search(r'```(?:json)?\s*(\[[\s\S]*?\])\s*```', text)
    if fence_match:
        candidates.insert(0, fence_match.group(1))
    start = text.find('[')
    end = text.rfind(']')
    if start != -1 and end != -1 and end > start:
        candidates.append(text[start : end + 1])

    for candidate in candidates:
        try:
            parsed = json.loads(candidate)
        except json.JSONDecodeError:
            continue
        if isinstance(parsed, list):
            return parsed
    return None


@dataclass
class MemoryReviewQueue:
    storage_path: Path

    @classmethod
    def from_workspace(cls, cwd: Path) -> 'MemoryReviewQueue':
        return cls(storage_path=(cwd.resolve() / DEFAULT_REVIEW_QUEUE_PATH).resolve())

    def load(self) -> list[ProposedEntry]:
        if not self.storage_path.is_file():
            return []
        try:
            payload = json.loads(self.storage_path.read_text(encoding='utf-8'))
        except (OSError, json.JSONDecodeError):
            return []
        raw_entries = payload.get('entries')
        if not isinstance(raw_entries, list):
            return []
        return [ProposedEntry.from_dict(item) for item in raw_entries if isinstance(item, dict)]

    def _save(self, entries: list[ProposedEntry]) -> None:
        self.storage_path.parent.mkdir(parents=True, exist_ok=True)
        payload = {'entries': [asdict(entry) for entry in entries]}
        self.storage_path.write_text(json.dumps(payload, ensure_ascii=True, indent=2), encoding='utf-8')

    def add(self, proposals: list[ProposedEntry]) -> None:
        if not proposals:
            return
        entries = self.load()
        entries.extend(proposals)
        self._save(entries)

    def list_pending(self) -> list[ProposedEntry]:
        return [entry for entry in self.load() if entry.status == 'pending']

    def get(self, entry_id: str) -> ProposedEntry | None:
        for entry in self.load():
            if entry.entry_id == entry_id:
                return entry
        return None

    def approve(self, entry_id: str, memory_runtime: MemoryRuntime) -> MemoryMutation:
        entries = self.load()
        for index, entry in enumerate(entries):
            if entry.entry_id != entry_id:
                continue
            if entry.status != 'pending':
                raise MemoryRuntimeError(f'entry {entry_id} is not pending (status={entry.status})')
            if entry.action == 'add':
                mutation = memory_runtime.add(entry.scope, entry.content or '')
            elif entry.action == 'replace':
                mutation = memory_runtime.replace(entry.scope, entry.old_text or '', entry.content or '')
            else:
                mutation = memory_runtime.remove(entry.scope, entry.old_text or '')
            entries[index] = _with_status(entry, 'approved')
            self._save(entries)
            return mutation
        raise MemoryRuntimeError(f'no pending review entry with id {entry_id}')

    def reject(self, entry_id: str) -> None:
        entries = self.load()
        for index, entry in enumerate(entries):
            if entry.entry_id != entry_id:
                continue
            if entry.status != 'pending':
                raise MemoryRuntimeError(f'entry {entry_id} is not pending (status={entry.status})')
            entries[index] = _with_status(entry, 'rejected')
            self._save(entries)
            return
        raise MemoryRuntimeError(f'no pending review entry with id {entry_id}')

    def render_summary(self) -> str:
        entries = self.load()
        if not entries:
            return '# Memory Review Queue\n\nNo review proposals are currently stored.'
        lines = ['# Memory Review Queue', '']
        for entry in entries:
            details = [entry.entry_id, f'status={entry.status}', f'scope={entry.scope}', f'action={entry.action}']
            lines.append('- ' + '; '.join(details))
            if entry.rationale:
                lines.append(f'  rationale: {entry.rationale}')
            if entry.content:
                lines.append(f'  content: {entry.content}')
            if entry.old_text:
                lines.append(f'  old_text: {entry.old_text}')
            if entry.blocked_reason:
                lines.append(f'  blocked: {entry.blocked_reason}')
        return '\n'.join(lines)


def _with_status(entry: ProposedEntry, status: str) -> ProposedEntry:
    return ProposedEntry(
        entry_id=entry.entry_id,
        session_id=entry.session_id,
        scope=entry.scope,
        action=entry.action,
        old_text=entry.old_text,
        content=entry.content,
        rationale=entry.rationale,
        status=status,
        blocked_reason=entry.blocked_reason,
        created_at=entry.created_at,
    )
