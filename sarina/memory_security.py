"""Security scanning for agent-curated persistent memory.

Curated memory (``memory_runtime.MemoryRuntime``) is written by the agent
itself and re-injected into the system prompt of every future session. That
makes a single poisoned entry a persistent, self-reinforcing prompt
injection: unlike a one-off tool result, it comes back every time. Every
write goes through :func:`scan_memory_text` before it is accepted, whatever
the source -- an interactive ``memory_add``/``memory_replace`` tool call or an
automated review proposal (``memory_review.py``). Both call sites route
through the same function so there is exactly one gate to reason about.

After the checks in this file run, :func:`scan_memory_text` also consults
Chymaera Sentry (``sentry_governance.consult_sentry``) if configured --  an
out-of-process second opinion running the same three checks independently,
in a separate OS process this one can't alter. Purely additive: with no
Sentry configured (the default), behavior here is unchanged.

Stdlib only: ``re``, ``unicodedata``, and (via ``sentry_governance``)
``urllib.request``.
"""

from __future__ import annotations

import re
import unicodedata
from dataclasses import dataclass

from .sentry_governance import consult_sentry

#: Zero-width, bidi-override, and Unicode "tag" characters have no benign use
#: in a short memory note and are a known technique for hiding instructions
#: from a human skimming the file while an LLM still reads them.
_INVISIBLE_CODEPOINTS = {
    0x200B,  # zero width space
    0x200C,  # zero width non-joiner
    0x200D,  # zero width joiner
    0x2060,  # word joiner
    0xFEFF,  # BOM / zero width no-break space
    0x00AD,  # soft hyphen
}
_BIDI_OVERRIDE_RANGE = range(0x202A, 0x202F)  # LRE, RLE, PDF, LRO, RLO
_ISOLATE_RANGE = range(0x2066, 0x206A)  # LRI, RLI, FSI, PDI
_TAG_RANGE = range(0xE0000, 0xE0080)  # Unicode tag block

#: Phrases that indicate the entry is trying to act as an instruction rather
#: than a fact worth remembering. Matched case-insensitively as substrings --
#: precision is less important than not letting an obvious one through.
_INJECTION_PATTERNS = tuple(
    re.compile(pattern, re.IGNORECASE)
    for pattern in (
        r'ignore (all|any|the)?\s*(previous|prior|above)\s*instructions',
        r'disregard (all|any|the)?\s*(previous|prior|above)',
        r'you are now\b',
        r'new (system )?instructions?\s*:',
        r'act as if\b',
        r'override (your|the|all) instructions',
        r'do not (tell|inform|notify) the user',
        r'without (informing|telling|notifying) the user',
        r'system prompt\s*:',
    )
)

#: Credential / secret shapes that do not belong in a memory note. Hermes'
#: own guidance is the model here: "credentials belong in .env, not memory."
_CREDENTIAL_PATTERNS = tuple(
    re.compile(pattern)
    for pattern in (
        r'-----BEGIN (RSA |EC |OPENSSH |DSA )?PRIVATE KEY-----',
        r'ssh-(rsa|ed25519|dss) AAAA[0-9A-Za-z+/]{10,}',
        r'AKIA[0-9A-Z]{16}',  # AWS access key id
        r'ghp_[0-9A-Za-z]{36}',  # GitHub personal access token
        r'xox[baprs]-[0-9A-Za-z-]{10,}',  # Slack token
        r'(?i)api[_-]?key\s*[:=]\s*[\'"]?[0-9A-Za-z_\-]{16,}',
        r'(?i)password\s*[:=]\s*\S+',
        r'(?i)secret[_-]?(key|token)?\s*[:=]\s*[\'"]?[0-9A-Za-z_\-]{12,}',
    )
)


@dataclass(frozen=True)
class MemoryScanResult:
    blocked: bool
    reason: str | None = None


def scan_memory_text(text: str) -> MemoryScanResult:
    """Scan a proposed memory entry for injection/exfiltration risk.

    Returns the first matching reason rather than an exhaustive list --
    callers reject on any block, so one reason is enough to act on.
    """
    invisible = _find_invisible_character(text)
    if invisible is not None:
        return MemoryScanResult(
            blocked=True,
            reason=(
                f'contains an invisible/formatting Unicode character '
                f'(U+{ord(invisible):04X}), which is not permitted in memory entries'
            ),
        )

    for pattern in _INJECTION_PATTERNS:
        match = pattern.search(text)
        if match:
            return MemoryScanResult(
                blocked=True,
                reason=f'looks like an instruction rather than a fact ("{match.group(0)}")',
            )

    for pattern in _CREDENTIAL_PATTERNS:
        if pattern.search(text):
            return MemoryScanResult(
                blocked=True,
                reason='looks like it contains a credential or secret; those belong outside memory',
            )

    verdict = consult_sentry(text, kind='memory_proposal')
    if verdict is not None and verdict.denied:
        return MemoryScanResult(
            blocked=True,
            reason='Chymaera Sentry flagged this independently: ' + '; '.join(verdict.reasons),
        )

    return MemoryScanResult(blocked=False)


def _find_invisible_character(text: str) -> str | None:
    for char in text:
        codepoint = ord(char)
        if codepoint in _INVISIBLE_CODEPOINTS:
            return char
        if codepoint in _BIDI_OVERRIDE_RANGE or codepoint in _ISOLATE_RANGE:
            return char
        if codepoint in _TAG_RANGE:
            return char
        if unicodedata.category(char) == 'Cf':
            # Any other Unicode "format" character not already named above --
            # catches variants without hardcoding every codepoint.
            return char
    return None
