"""Optional consultation with Chymaera Sentry's governance API
(``POST /v1/review``) before a memory write proceeds -- an out-of-process
second opinion that can't be silently bypassed by whatever produced the
text, since it runs in a separate OS process this one has no ability to
alter.

Purely additive: if Sentry isn't configured (``SENTRY_REVIEW_URL`` unset)
or isn't reachable, this returns ``None`` and callers fall back to
whatever their own in-process check already decided -- Sentry is not a
hard dependency of this repo, the same way none of the other Chymaera
Ecosystem repos are (see docs/ECOSYSTEM.md). Stdlib only
(``urllib.request``), matching the core agent's zero-dependency rule.

This is Sentry's proposed contract, not yet a settled one -- see
docs/ECOSYSTEM.md and Chymaera_Sentry's own CLAUDE.md Coordination Log.
"""

from __future__ import annotations

import json
import os
import urllib.error
import urllib.request
from dataclasses import dataclass, field

#: Full URL of Sentry's review endpoint, e.g. "http://127.0.0.1:8930/v1/review".
#: Unset (the default) disables this module's only effect entirely.
ENV_VAR = 'SENTRY_REVIEW_URL'

_TIMEOUT_SECONDS = 2.0


@dataclass(frozen=True)
class SentryVerdict:
    denied: bool
    reasons: tuple[str, ...] = field(default_factory=tuple)


def consult_sentry(text: str, *, kind: str, session_id: str = '') -> SentryVerdict | None:
    """Ask Sentry's governance API about ``text``.

    Returns ``None`` when Sentry integration is not configured, or is
    configured but unreachable/misconfigured (fail-open on *absence* of an
    answer) -- this function never raises. Returns a ``SentryVerdict`` only
    when a response was actually received and parsed, so a caller checking
    ``verdict is not None and verdict.denied`` fails *closed* on an actual
    deny.
    """
    url = os.environ.get(ENV_VAR)
    if not url:
        return None

    payload = json.dumps({'kind': kind, 'session_id': session_id, 'text': text}).encode('utf-8')
    request = urllib.request.Request(
        url, data=payload, headers={'Content-Type': 'application/json'}, method='POST'
    )
    try:
        with urllib.request.urlopen(request, timeout=_TIMEOUT_SECONDS) as response:
            body = json.loads(response.read().decode('utf-8'))
    except (urllib.error.URLError, TimeoutError, ValueError, OSError):
        return None

    if not isinstance(body, dict):
        return None
    reasons = tuple(str(r) for r in body.get('reasons') or ())
    return SentryVerdict(denied=body.get('decision') == 'deny', reasons=reasons)
