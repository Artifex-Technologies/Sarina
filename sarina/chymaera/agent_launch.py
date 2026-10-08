"""Launching other AI agents: proposal-first, human-confirmed, audited.

Chymaera_Linux contract item 7, step 3. Omarchy starts every agent with its own
spelling of "don't stop to ask" (``claude --permission-mode auto``,
``gemini --yolo``, ``codex --approve-for-me`` ...). A launcher that copied that
would turn a prompt-injected Sarina into a way to run another agent with no human
in the loop, so this one is deliberately different:

- It **never adds an auto-approve flag**, and has no option to. The argument
  forms below are Omarchy's with those flags removed, so each agent asks for
  permission as it normally would.
- It **proposes first**. :func:`plan_launch` is pure; nothing runs until
  :func:`launch` is called with ``confirmed=True``.
- It is **only reachable from the ``agent-launch`` CLI command**, never
  registered as a tool the model can call, so the model cannot trigger it.
- Every launch attempt, confirmed or refused, is appended to an **audit log**
  that Chymaera Sentry can read. The prompt is stored as a hash, a length and a
  short preview, because prompts can contain secrets.

Standard library only.
"""

from __future__ import annotations

import hashlib
import json
import os
import shutil
import subprocess
from dataclasses import dataclass
from datetime import datetime, timezone
from pathlib import Path
from typing import Callable, Mapping

PROMPT_PREVIEW_LENGTH = 60
MAX_PROMPT_LENGTH = 8000

# Per agent: argv up to the prompt, then how the prompt is attached. ``--`` is
# used wherever the agent supports it so a prompt beginning with a dash cannot be
# read as an option. No entry contains an approval-skipping flag.
#   ('positional', head, prefix)  -> head + prefix + [prompt]
#   ('flag', head, flag)          -> head + [flag, prompt]
#   ('joined', head, flag)        -> head + [f'{flag}={prompt}']
_FORMS: Mapping[str, tuple[str, tuple[str, ...], str | tuple[str, ...]]] = {
    'claude': ('positional', ('claude',), ('--',)),
    'codex': ('positional', ('codex',), ('--',)),
    'grok': ('positional', ('grok',), ('--',)),
    'muse': ('positional', ('muse',), ('--',)),
    'omp': ('positional', ('omp',), ('--',)),
    'pi': ('positional', ('pi',), ()),
    'cursor-agent': ('positional', ('cursor-agent',), ('agent', '--')),
    'opencode': ('flag', ('opencode',), '--prompt'),
    'gemini': ('flag', ('gemini',), '--prompt-interactive'),
    'copilot': ('flag', ('copilot',), '--interactive'),
    'hermes': ('joined', ('hermes', 'chat', '--tui'), '--query'),
}

# Agents Omarchy can launch that this module deliberately does not:
#   crush     its only prompt-seeded form is `crush run`, which never asks.
#   openclaw  needs Omarchy's gateway launcher.
UNSUPPORTED: Mapping[str, str] = {
    'crush': 'its only prompt-seeded mode (`crush run`) never asks for approval',
    'openclaw': 'it needs Omarchy\'s own launcher to attach to its gateway',
}

# Agents with no approval prompts of their own; launching one is unattended no
# matter what Sarina passes, so the operator is told.
NO_APPROVAL_PROMPTS = frozenset({'pi'})


class LaunchError(Exception):
    """A proposal that cannot be made or a launch that was not authorised."""


@dataclass(frozen=True)
class LaunchProposal:
    agent: str
    argv: tuple[str, ...]
    cwd: str
    binary_path: str | None
    display: str
    prompt_sha256: str
    prompt_length: int
    warnings: tuple[str, ...]

    def as_text(self) -> str:
        lines = [
            f'Proposed launch: {self.agent}',
            f'  command : {self.display}',
            f'  cwd     : {self.cwd}',
            f'  prompt  : {self.prompt_length} characters',
            '  approval: Sarina passes no auto-approve flag'
            + ('' if self.agent in NO_APPROVAL_PROMPTS else '; the agent will ask before acting'),
        ]
        if self.binary_path is None:
            lines.append(f'  warning : `{self.argv[0]}` was not found on PATH')
        for warning in self.warnings:
            lines.append(f'  warning : {warning}')
        return '\n'.join(lines)


def _build_argv(kind: str, head: tuple[str, ...], attach: str | tuple[str, ...], prompt: str) -> list[str]:
    argv = list(head)
    if not prompt:
        return argv
    if kind == 'positional':
        argv += list(attach) + [prompt]  # type: ignore[arg-type]
    elif kind == 'flag':
        argv += [str(attach), prompt]
    else:
        argv += [f'{attach}={prompt}']
    return argv


def supported_agents() -> tuple[str, ...]:
    return tuple(sorted(_FORMS))


def plan_launch(
    agent: str,
    prompt: str = '',
    *,
    cwd: Path | None = None,
    which: Callable[[str], str | None] = shutil.which,
) -> LaunchProposal:
    """Describe a launch without doing it."""
    if agent in UNSUPPORTED:
        raise LaunchError(f'{agent} is not supported: {UNSUPPORTED[agent]}.')
    form = _FORMS.get(agent)
    if form is None:
        raise LaunchError(
            f'unknown agent {agent!r}; supported: {", ".join(supported_agents())}'
        )
    if len(prompt) > MAX_PROMPT_LENGTH:
        raise LaunchError(f'prompt is {len(prompt)} characters; the limit is {MAX_PROMPT_LENGTH}')
    if '\x00' in prompt:
        raise LaunchError('prompt contains a NUL byte')

    kind, head, attach = form
    argv = _build_argv(kind, head, attach, prompt)
    display = ' '.join(_build_argv(kind, head, attach, '<prompt>' if prompt else ''))

    warnings = []
    if agent in NO_APPROVAL_PROMPTS:
        warnings.append(f'{agent} has no approval prompts of its own, so it will run unattended')

    return LaunchProposal(
        agent=agent,
        argv=tuple(argv),
        cwd=str((cwd or Path.cwd()).resolve()),
        binary_path=which(head[0]),
        display=display,
        prompt_sha256=hashlib.sha256(prompt.encode('utf-8')).hexdigest(),
        prompt_length=len(prompt),
        warnings=tuple(warnings),
    )


def audit_log_path(env: Mapping[str, str] | None = None) -> Path:
    env = os.environ if env is None else env
    state_home = env.get('XDG_STATE_HOME')
    base = Path(state_home) if state_home else Path.home() / '.local' / 'state'
    return base / 'sarina' / 'agent-launch.jsonl'


def _audit(
    proposal: LaunchProposal,
    prompt: str,
    outcome: str,
    log_path: Path,
    detail: str = '',
) -> None:
    entry = {
        'time': datetime.now(timezone.utc).isoformat(),
        'agent': proposal.agent,
        'argv0': proposal.argv[0],
        'cwd': proposal.cwd,
        'promptSha256': proposal.prompt_sha256,
        'promptLength': proposal.prompt_length,
        'promptPreview': prompt[:PROMPT_PREVIEW_LENGTH].replace('\n', ' '),
        'outcome': outcome,
        'detail': detail,
    }
    log_path.parent.mkdir(parents=True, exist_ok=True)
    with log_path.open('a', encoding='utf-8') as handle:
        handle.write(json.dumps(entry, separators=(',', ':')) + '\n')


def launch(
    proposal: LaunchProposal,
    prompt: str,
    *,
    confirmed: bool,
    audit_path: Path | None = None,
    run: Callable[..., subprocess.CompletedProcess] = subprocess.run,
) -> int:
    """Run an approved proposal in the foreground and return its exit status.

    Raises :class:`LaunchError` unless ``confirmed`` is true; the refusal is
    audited too. The agent inherits the terminal, so the human can answer its
    permission prompts.
    """
    log_path = audit_path or audit_log_path()
    if hashlib.sha256(prompt.encode('utf-8')).hexdigest() != proposal.prompt_sha256:
        _audit(proposal, prompt, 'refused', log_path, 'prompt differs from the proposal')
        raise LaunchError('prompt does not match the proposal that was shown')
    if not confirmed:
        _audit(proposal, prompt, 'refused', log_path, 'not confirmed by the operator')
        raise LaunchError('launch was not confirmed')
    if proposal.binary_path is None:
        _audit(proposal, prompt, 'refused', log_path, f'{proposal.argv[0]} not on PATH')
        raise LaunchError(f'`{proposal.argv[0]}` was not found on PATH')

    _audit(proposal, prompt, 'launched', log_path)
    result = run(list(proposal.argv), cwd=proposal.cwd, check=False)
    return int(result.returncode)
