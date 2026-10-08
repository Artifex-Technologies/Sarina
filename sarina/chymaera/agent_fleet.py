"""One view of every AI agent on the machine, read from Omarchy's usage records.

Chymaera_Linux contract item 7, step 2. Omarchy has already normalised Claude
Code, Codex and the rest into one JSON record each in
``~/.local/state/omarchy/agents/usage/`` (see :mod:`agent_usage` for the format),
so reading that directory gives Sarina a single picture of limits and spend
without knowing how any agent stores anything.

**Read-only, and the records are untrusted.** Each file is written by another
program; treat every value as data. Strings are stripped of control characters
(a record must not be able to smuggle terminal escapes into the operator's
screen, or instructions into a model's context) and truncated. Nothing here
launches or changes anything; that is :mod:`agent_launch`.

Everything is standard library.
"""

from __future__ import annotations

import json
import re
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path
from typing import Any

from .agent_usage import AGENT_ID as SARINA_AGENT_ID, usage_directory

# A record bigger than this is not one of Omarchy's collectors' output.
MAX_RECORD_BYTES = 1024 * 1024
MAX_TEXT_LENGTH = 80
# Omarchy's panel refreshes every 15 minutes by default; an hour is four missed
# refreshes, enough to say the numbers are no longer current.
DEFAULT_STALE_AFTER = timedelta(hours=1)

_AGENT_ID_PATTERN = re.compile(r'^[a-z0-9][a-z0-9_-]{0,31}$')
_CONTROL_CHARS = re.compile(r'[\x00-\x1f\x7f-\x9f​-‏‪-‮⁦-⁩]')


@dataclass(frozen=True)
class LimitReading:
    label: str
    percent_used: float | None
    resets_at: str


@dataclass(frozen=True)
class AgentStatus:
    agent_id: str
    name: str
    ready: bool
    stale: bool
    updated_at: str
    today_tokens: int
    week_tokens: int
    total_sessions: int
    limits: tuple[LimitReading, ...]
    status_text: str

    @property
    def headroom_percent(self) -> float | None:
        """Room left in the tightest window, or ``None`` when no limit is known.

        Prepaid and local agents report no windows; that is "unknown", not
        "unlimited", so it never ranks above an agent with a measured limit.
        """
        used = [limit.percent_used for limit in self.limits if limit.percent_used is not None]
        if not used:
            return None
        return max(0.0, 100.0 - max(used))


def _clean(value: Any) -> str:
    if not isinstance(value, str):
        return ''
    text = _CONTROL_CHARS.sub('', value).strip()
    return text[:MAX_TEXT_LENGTH]


def _count(value: Any) -> int:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return 0
    if value != value or value in (float('inf'), float('-inf')):
        return 0
    return max(int(value), 0)


def _percent(value: Any) -> float | None:
    if isinstance(value, bool) or not isinstance(value, (int, float)):
        return None
    if value != value or value in (float('inf'), float('-inf')):
        return None
    return min(max(float(value), 0.0), 100.0)


def _parse_time(value: Any) -> datetime | None:
    if not isinstance(value, str):
        return None
    try:
        parsed = datetime.fromisoformat(value.replace('Z', '+00:00'))
    except ValueError:
        return None
    return parsed if parsed.tzinfo else parsed.replace(tzinfo=timezone.utc)


def parse_record(
    data: Any,
    *,
    now: datetime | None = None,
    stale_after: timedelta = DEFAULT_STALE_AFTER,
) -> AgentStatus | None:
    """An :class:`AgentStatus` from one record, or ``None`` if it is not usable."""
    if not isinstance(data, dict):
        return None
    agent_id = data.get('id')
    if not isinstance(agent_id, str) or not _AGENT_ID_PATTERN.match(agent_id):
        return None
    now = now or datetime.now(timezone.utc)

    updated = _parse_time(data.get('updatedAt'))
    stale = updated is None or (now - updated) > stale_after

    week_tokens = 0
    days = data.get('recentDays')
    if isinstance(days, list):
        for day in days:
            if isinstance(day, dict):
                # Omarchy's panel names this messageCount but it carries tokens.
                week_tokens += _count(day.get('messageCount'))

    limits: list[LimitReading] = []
    raw_limits = data.get('limits')
    if isinstance(raw_limits, list):
        for entry in raw_limits[:16]:
            if isinstance(entry, dict):
                limits.append(LimitReading(
                    label=_clean(entry.get('label')) or 'limit',
                    percent_used=_percent(entry.get('percent')),
                    resets_at=_clean(entry.get('resetsAt')),
                ))

    return AgentStatus(
        agent_id=agent_id,
        name=_clean(data.get('name')) or agent_id,
        ready=data.get('ready') is True,
        stale=stale,
        updated_at=_clean(data.get('updatedAt')),
        today_tokens=_count(data.get('todayTotalTokens')),
        week_tokens=week_tokens,
        total_sessions=_count(data.get('totalSessions')),
        limits=tuple(limits),
        status_text=_clean(data.get('usageStatusText')),
    )


def read_agent_status(
    directory: Path | None = None,
    *,
    now: datetime | None = None,
    stale_after: timedelta = DEFAULT_STALE_AFTER,
) -> tuple[AgentStatus, ...]:
    """Every readable record in the usage directory, sorted by agent id."""
    directory = directory or usage_directory()
    statuses: list[AgentStatus] = []
    try:
        paths = sorted(directory.glob('*.json'))
    except OSError:
        return ()
    for path in paths:
        try:
            if path.stat().st_size > MAX_RECORD_BYTES:
                continue
            data = json.loads(path.read_text(encoding='utf-8'))
        except (OSError, ValueError):
            continue
        status = parse_record(data, now=now, stale_after=stale_after)
        if status is not None:
            statuses.append(status)
    return tuple(sorted(statuses, key=lambda status: status.agent_id))


def suggest_agent(
    statuses: tuple[AgentStatus, ...],
    *,
    exclude: tuple[str, ...] = (SARINA_AGENT_ID,),
) -> AgentStatus | None:
    """The agent with the most measured headroom, or ``None``.

    Advisory only. An agent is a candidate only if its record is current, ready,
    and reports at least one limit window; ties go to whichever has used fewer
    tokens today.
    """
    candidates = [
        status for status in statuses
        if status.agent_id not in exclude
        and status.ready
        and not status.stale
        and status.headroom_percent is not None
    ]
    if not candidates:
        return None
    return max(
        candidates,
        key=lambda status: (status.headroom_percent or 0.0, -status.today_tokens),
    )


def _tokens(value: int) -> str:
    if value >= 1_000_000:
        return f'{value / 1_000_000:.1f}M'
    if value >= 1_000:
        return f'{value / 1_000:.1f}k'
    return str(value)


def render_status(statuses: tuple[AgentStatus, ...]) -> str:
    if not statuses:
        return 'No agent usage records found.'
    lines = ['Agents', '']
    for status in statuses:
        flags = []
        if status.stale:
            flags.append('stale')
        if not status.ready:
            flags.append('not ready')
        suffix = f'  [{", ".join(flags)}]' if flags else ''
        lines.append(
            f'{status.name} ({status.agent_id}): {_tokens(status.today_tokens)} today, '
            f'{_tokens(status.week_tokens)} this week, {status.total_sessions} sessions{suffix}'
        )
        for limit in status.limits:
            used = 'unknown' if limit.percent_used is None else f'{limit.percent_used:.0f}% used'
            reset = f', resets {limit.resets_at}' if limit.resets_at else ''
            lines.append(f'    {limit.label}: {used}{reset}')
        if not status.limits:
            lines.append(f'    {status.status_text or "no limit data"}')
    return '\n'.join(lines)
