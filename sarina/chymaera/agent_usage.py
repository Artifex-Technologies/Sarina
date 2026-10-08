"""Sarina's usage record for Omarchy's agents panel.

Chymaera_Linux contract item 7, step 1 (see ``docs/ECOSYSTEM.md``). Omarchy's bar
has one panel that shows every AI agent on the machine. The panel is a pure
display: each agent is one JSON file in ``~/.local/state/omarchy/agents/usage/``,
written by an ``omarchy-agent-usage-<agent>`` collector that prints a single
record on stdout, and *any* file that lands in that directory becomes a tab.
This module builds that record for Sarina from her saved sessions.

Everything here is standard library and read-only against the session files; the
only thing it ever writes is the one record file, and only when asked.

Attribution is approximate by design. A session file holds cumulative token
totals, not a per-turn ledger, so all of a session's tokens are attributed to the
local day the file was last written. That is exact for a session that started and
finished the same day and slightly generous to the last day for one that spanned
midnight.
"""

from __future__ import annotations

import json
import os
import tempfile
from dataclasses import dataclass
from datetime import datetime, timedelta
from pathlib import Path
from typing import Any, Iterable, Mapping

AGENT_ID = 'sarina'
AGENT_NAME = 'Sarina'
RECORD_SCHEMA_VERSION = 1
RECENT_DAY_COUNT = 7

# A session file larger than this is not a session Sarina wrote; skip it rather
# than read it into memory.
MAX_SESSION_BYTES = 64 * 1024 * 1024

# Where the service keeps sessions on an installed system
# (packaging/arch/sarina-service.service).
SERVICE_SESSION_DIR = Path('/var/lib/sarina/sessions')

SESSION_DIRS_ENV = 'SARINA_SESSION_DIRS'


@dataclass(frozen=True)
class SessionUsage:
    session_id: str
    day: str
    model: str
    prompts: int
    input_tokens: int
    output_tokens: int
    cache_read_tokens: int
    cache_write_tokens: int

    @property
    def total_tokens(self) -> int:
        return (
            self.input_tokens
            + self.output_tokens
            + self.cache_read_tokens
            + self.cache_write_tokens
        )


def _int(value: Any) -> int:
    """A non-negative int from untrusted JSON; anything else is zero."""
    if isinstance(value, bool):
        return 0
    if isinstance(value, int):
        return max(value, 0)
    if isinstance(value, float) and value == value and value not in (float('inf'), float('-inf')):
        return max(int(value), 0)
    return 0


def usage_directory(env: Mapping[str, str] | None = None) -> Path:
    """Where Omarchy's panel looks for usage records."""
    env = os.environ if env is None else env
    state_home = env.get('XDG_STATE_HOME')
    base = Path(state_home) if state_home else Path.home() / '.local' / 'state'
    return base / 'omarchy' / 'agents' / 'usage'


def default_session_directories(
    env: Mapping[str, str] | None = None,
    *,
    cwd: Path | None = None,
    home: Path | None = None,
) -> tuple[Path, ...]:
    """Every place Sarina is known to save sessions.

    ``SARINA_SESSION_DIRS`` (``os.pathsep``-separated) replaces the defaults
    outright. Directories that do not exist or cannot be read are skipped by the
    reader, so listing all of them is safe.
    """
    env = os.environ if env is None else env
    override = env.get(SESSION_DIRS_ENV)
    if override:
        return tuple(Path(part).expanduser() for part in override.split(os.pathsep) if part)
    cwd = cwd or Path.cwd()
    home = home or Path.home()
    candidates = (
        cwd / '.port_sessions' / 'agent',
        home / '.port_sessions' / 'agent',
        SERVICE_SESSION_DIR,
    )
    seen: list[Path] = []
    for candidate in candidates:
        if candidate not in seen:
            seen.append(candidate)
    return tuple(seen)


def _local_day(timestamp: float) -> str:
    return datetime.fromtimestamp(timestamp).strftime('%Y-%m-%d')


def _read_session(path: Path) -> SessionUsage | None:
    try:
        stat = path.stat()
        if stat.st_size > MAX_SESSION_BYTES:
            return None
        data = json.loads(path.read_text(encoding='utf-8'))
    except (OSError, ValueError):
        return None
    if not isinstance(data, dict):
        return None
    usage = data.get('usage')
    if not isinstance(usage, dict):
        return None
    model_config = data.get('model_config')
    model = model_config.get('model') if isinstance(model_config, dict) else None
    session_id = data.get('session_id')
    return SessionUsage(
        session_id=str(session_id) if session_id else path.stem,
        day=_local_day(stat.st_mtime),
        model=str(model) if model else 'unknown',
        prompts=_int(data.get('turns')),
        input_tokens=_int(usage.get('input_tokens')),
        output_tokens=_int(usage.get('output_tokens')),
        cache_read_tokens=_int(usage.get('cache_read_input_tokens')),
        cache_write_tokens=_int(usage.get('cache_creation_input_tokens')),
    )


def read_sessions(directories: Iterable[Path]) -> list[SessionUsage]:
    """Sessions found under ``directories``, one per session id.

    The same session can appear in more than one directory (the CLI and the
    service can share an id space); the most recently written copy wins.
    """
    newest: dict[str, tuple[float, SessionUsage]] = {}
    for directory in directories:
        try:
            paths = sorted(directory.glob('*.json'))
        except OSError:
            continue
        for path in paths:
            session = _read_session(path)
            if session is None:
                continue
            try:
                mtime = path.stat().st_mtime
            except OSError:
                continue
            known = newest.get(session.session_id)
            if known is None or mtime > known[0]:
                newest[session.session_id] = (mtime, session)
    return [session for _, session in newest.values()]


def build_record(
    sessions: Iterable[SessionUsage],
    *,
    now: datetime | None = None,
) -> dict[str, Any]:
    """The panel record: same shape Omarchy's own Codex collector prints."""
    now = now or datetime.now()
    today = now.strftime('%Y-%m-%d')
    recent_dates = [
        (now - timedelta(days=offset)).strftime('%Y-%m-%d')
        for offset in range(RECENT_DAY_COUNT - 1, -1, -1)
    ]
    recent = {day: {'date': day, 'messageCount': 0} for day in recent_dates}
    model_usage: dict[str, dict[str, int]] = {}
    today_tokens_by_model: dict[str, int] = {}
    today_sessions: set[str] = set()
    total_sessions: set[str] = set()
    active_days: set[str] = set()
    today_prompts = 0
    today_total_tokens = 0
    total_prompts = 0

    for session in sessions:
        total = session.total_tokens
        total_prompts += session.prompts
        total_sessions.add(session.session_id)
        active_days.add(session.day)

        bucket = model_usage.setdefault(session.model, {
            'inputTokens': 0,
            'outputTokens': 0,
            'cacheReadInputTokens': 0,
            'cacheCreationInputTokens': 0,
        })
        bucket['inputTokens'] += session.input_tokens
        bucket['outputTokens'] += session.output_tokens
        bucket['cacheReadInputTokens'] += session.cache_read_tokens
        bucket['cacheCreationInputTokens'] += session.cache_write_tokens

        if session.day in recent:
            # Omarchy's panel calls this field messageCount but plots tokens.
            recent[session.day]['messageCount'] += total

        if session.day == today:
            today_prompts += session.prompts
            today_sessions.add(session.session_id)
            today_total_tokens += total
            today_tokens_by_model[session.model] = today_tokens_by_model.get(session.model, 0) + total

    return {
        'schemaVersion': RECORD_SCHEMA_VERSION,
        'id': AGENT_ID,
        'name': AGENT_NAME,
        'updatedAt': datetime.now().astimezone().isoformat(),
        'ready': True,
        'hasLocalStats': True,
        'todayPrompts': today_prompts,
        'todaySessions': len(today_sessions),
        'todayTotalTokens': today_total_tokens,
        'todayTokensByModel': today_tokens_by_model,
        'recentDays': [recent[day] for day in recent_dates],
        'totalPrompts': total_prompts,
        'totalSessions': len(total_sessions),
        'activeDays': len(active_days),
        'activeDates': sorted(active_days),
        'modelUsage': model_usage,
    }


def collect_record(
    directories: Iterable[Path] | None = None,
    *,
    now: datetime | None = None,
) -> dict[str, Any]:
    dirs = tuple(directories) if directories is not None else default_session_directories()
    return build_record(read_sessions(dirs), now=now)


def write_record(record: Mapping[str, Any], directory: Path | None = None) -> Path:
    """Write ``sarina.json`` atomically, so the panel never reads a torn file."""
    target_dir = directory or usage_directory()
    target_dir.mkdir(parents=True, exist_ok=True)
    path = target_dir / f'{AGENT_ID}.json'
    fd, tmp_name = tempfile.mkstemp(prefix=f'.{AGENT_ID}.', dir=target_dir)
    tmp = Path(tmp_name)
    try:
        with os.fdopen(fd, 'w', encoding='utf-8') as handle:
            handle.write(json.dumps(record, separators=(',', ':')) + '\n')
        # mkstemp opens 0600; nothing in this record is sensitive.
        tmp.chmod(0o644)
        tmp.replace(path)
    except BaseException:
        tmp.unlink(missing_ok=True)
        raise
    return path
