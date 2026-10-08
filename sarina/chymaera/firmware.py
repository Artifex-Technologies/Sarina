"""Read-only advisory helpers for the Chymaera Zero firmware fork.

Implements the first step the firmware project asked Sarina for (see its
``CLAUDE.md``, "Notes for other Chymaera instances"): parse ``device_info`` and
report what a merge from ``upstream/dev`` would touch. Nothing here writes to a
device — bricking a Flipper is expensive to recover, so flashing stays a human
action.
"""

from __future__ import annotations

import re
import subprocess
from dataclasses import dataclass
from pathlib import Path

#: ``device_info`` fields that must hold a specific value on a healthy Chymaera
#: build, mapped to the value expected. Sourced from the firmware repo's
#: CLAUDE.md "Verifying what is actually on the device" section.
DEVICE_INFO_EXPECTATIONS: dict[str, str] = {
    'firmware_origin_fork': 'Chymaera',
    'enclave_valid': 'true',
    'radio_alive': 'true',
}

#: Path prefixes mapped to (area, why), most specific first. This is a
#: **descriptive** label only — what a change touches, not how risky it is.
#: Merge risk comes from the upstream-existence test in ``merge_impact``.
RISK_RULES: tuple[tuple[str, str, str], ...] = (
    ('assets/protobuf/', 'wire-format',
     'RPC wire format - a unilateral change breaks stock qFlipper compatibility'),
    ('applications_user/', 'external-app',
     'external app built against the published API; unaffected by upstream refactors'),
    ('furi/', 'core', 'OS core'),
    ('targets/', 'core', 'platform code'),
    ('applications/', 'core', 'firmware app tree'),
    ('lib/', 'core', 'vendored library'),
    ('site_scons/', 'build', 'build system'),
    ('scripts/', 'build', 'build system'),
    ('SConstruct', 'build', 'build system'),
    ('fbt', 'build', 'build system'),
    ('documentation/', 'docs', 'documentation'),
)

#: Areas in descending order of how much attention a change there usually needs.
TIER_ORDER: tuple[str, ...] = ('wire-format', 'core', 'build', 'external-app', 'docs', 'other')


def classify_path(path: str) -> tuple[str, str]:
    """Return the descriptive ``(area, why)`` for one repo-relative path."""
    for prefix, area, why in RISK_RULES:
        if path.startswith(prefix):
            return area, why
    if path.endswith('.md'):
        return 'docs', 'documentation'
    return 'other', 'uncategorized'


# ---------------------------------------------------------------------------
# device_info
# ---------------------------------------------------------------------------

def parse_device_info(text: str) -> dict[str, str]:
    """Parse ``device_info`` serial output into a field mapping.

    The device emits ``key<spaces>: value`` lines. Unparseable lines are
    skipped rather than raising - serial output routinely carries banner text.
    """
    fields: dict[str, str] = {}
    for line in text.splitlines():
        key, separator, value = line.partition(':')
        if not separator:
            continue
        key = key.strip()
        if not key or ' ' in key:
            continue
        fields[key] = value.strip()
    return fields


#: Current firmware version format (2026-09-12 onwards): the upstream release
#: the fork tracks, our tag, and ``+N`` commits since it on a development build
#: - ``1.4.3-CHY-0.1.0``, ``1.4.3-CHY-0.1.0+8``, or ``1.4.3-CHY-dev`` when no
#: ``chy-*`` tag exists yet. The commit is no longer in this string; the
#: separate ``firmware_commit`` field is exact.
CHYMAERA_VERSION = re.compile(
    r'^(?P<upstream>\d+\.\d+\.\d+)-CHY-(?P<ours>dev|\d[\d.]*)(?:\+(?P<ahead>\d+))?$'
)

#: The previous format, from ``git describe --tags --always``
#: (``chy-0.1.0-2-g540d03c7``). Kept because a device may still be running
#: firmware flashed before the format changed.
LEGACY_DEV_BUILD = re.compile(r'-\d+-g[0-9a-f]{7,}$')


@dataclass(frozen=True)
class DeviceInfoReport:
    """Result of checking parsed ``device_info`` fields against expectations.

    ``warnings`` mean something is wrong and drive the exit code. ``notes`` are
    observations that are not faults - a development build is normal during
    development, so it must not fail a check.
    """

    fields: dict[str, str]
    warnings: tuple[str, ...]
    notes: tuple[str, ...] = ()

    @property
    def ok(self) -> bool:
        return not self.warnings

    def as_text(self) -> str:
        lines = ['# Chymaera Zero device', '']
        if not self.fields:
            lines.append('No device_info fields parsed.')
            return '\n'.join(lines)
        for key in ('firmware_origin_fork', 'firmware_version', 'firmware_commit',
                    'firmware_origin_git', 'hardware_model', 'enclave_valid', 'radio_alive'):
            if key in self.fields:
                lines.append(f'{key}: {self.fields[key]}')
        lines.append('')
        if self.warnings:
            lines.append('## Warnings')
            lines.extend(f'- {warning}' for warning in self.warnings)
        else:
            lines.append('No warnings.')
        if self.notes:
            lines.append('')
            lines.append('## Notes')
            lines.extend(f'- {note}' for note in self.notes)
        return '\n'.join(lines)


def check_device_info(fields: dict[str, str], expected_commit: str | None = None) -> DeviceInfoReport:
    """Check parsed ``device_info`` fields, optionally against an expected commit."""
    warnings: list[str] = []
    for key, expected in DEVICE_INFO_EXPECTATIONS.items():
        actual = fields.get(key)
        if actual is None:
            warnings.append(f'{key} missing from device_info')
        elif actual.lower() != expected.lower():
            warnings.append(f'{key} is {actual!r}, expected {expected!r}')

    notes: list[str] = []
    version = fields.get('firmware_version')
    if version in {'unknown', ''}:
        warnings.append(
            'firmware_version is unknown - the build could not describe itself from git at all'
        )
    elif version is not None:
        match = CHYMAERA_VERSION.match(version)
        if match is None:
            if LEGACY_DEV_BUILD.search(version):
                notes.append(
                    f'{version} uses the pre-2026-09-12 version format - the device '
                    'predates the current firmware'
                )
            else:
                notes.append(f'{version} is not a recognised Chymaera version string')
        else:
            notes.append(f'tracks upstream release {match["upstream"]}')
            if match['ours'] == 'dev':
                notes.append('no chy-* tag exists yet, so this build is not a release')
            elif match['ahead']:
                notes.append(
                    f'development build: {match["ahead"]} commit(s) since chy-{match["ours"]}'
                )

    if expected_commit:
        actual_commit = fields.get('firmware_commit', '')
        if actual_commit and not (
            expected_commit.startswith(actual_commit) or actual_commit.startswith(expected_commit)
        ):
            warnings.append(
                f'firmware_commit {actual_commit!r} does not match expected {expected_commit!r} '
                '- the device is not running this tree'
            )
    return DeviceInfoReport(fields=fields, warnings=tuple(warnings), notes=tuple(notes))


# ---------------------------------------------------------------------------
# upstream merge impact
# ---------------------------------------------------------------------------

#: Risk tiers, worst first. Defined by the firmware project: risk is decided by
#: whether a path exists upstream, not by which directory it sits in.
MERGE_TIER_ORDER: tuple[str, ...] = ('high', 'medium', 'low')

#: More than this many non-additive first-party files means the "add
#: directories, do not edit upstream files" rebase policy is being violated.
#: The firmware project's number: "roughly a dozen".
REBASE_POLICY_BUDGET = 12

MERGE_TIER_DESCRIPTIONS: dict[str, str] = {
    'high': 'changed on both sides - a merge will conflict here',
    'medium': 'ours, and the file exists upstream - conflicts if upstream moves it',
    'low': 'does not exist upstream - additive, cannot conflict',
}


@dataclass(frozen=True)
class FileRisk:
    """One locally-changed path and what it would cost in a merge."""

    path: str
    tier: str
    area: str
    exists_upstream: bool
    changed_upstream: bool


@dataclass(frozen=True)
class MergeImpactReport:
    """What merging ``upstream_ref`` into the local firmware tree would touch."""

    upstream_ref: str
    base: str
    upstream_changed: tuple[str, ...]
    local_changed: tuple[str, ...]
    risks: tuple[FileRisk, ...]

    def by_tier(self) -> dict[str, list[FileRisk]]:
        grouped: dict[str, list[FileRisk]] = {}
        for risk in self.risks:
            grouped.setdefault(risk.tier, []).append(risk)
        return grouped

    @property
    def conflicting(self) -> tuple[FileRisk, ...]:
        """Files that would actually conflict: upstream files both sides moved."""
        return tuple(risk for risk in self.risks if risk.tier == 'high')

    @property
    def non_additive(self) -> tuple[FileRisk, ...]:
        """Files we changed that exist upstream - the rebase policy's real cost."""
        return tuple(risk for risk in self.risks if risk.tier in {'high', 'medium'})

    @property
    def policy_breached(self) -> bool:
        return len(self.non_additive) > REBASE_POLICY_BUDGET

    def as_text(self) -> str:
        grouped = self.by_tier()
        lines = [
            f'# Merge impact: {self.upstream_ref} -> local firmware tree',
            '',
            f'merge base: {self.base}',
            f'upstream changed since base: {len(self.upstream_changed)} file(s)',
            f'local changed since base: {len(self.local_changed)} file(s)',
            f'would conflict (high): {len(grouped.get("high", []))}',
            f'ours, exists upstream (medium): {len(grouped.get("medium", []))}',
            f'additive, cannot conflict (low): {len(grouped.get("low", []))}',
            '',
        ]

        if self.policy_breached:
            lines.append(
                f'## POLICY ALARM: {len(self.non_additive)} first-party files exist '
                f'upstream, over the budget of {REBASE_POLICY_BUDGET}'
            )
            lines.append(
                'The "add directories, do not edit upstream files" rebase policy is '
                'being violated. That matters more than any single file below.'
            )
            lines.append('')

        if not self.risks:
            lines.append(
                'Nothing changed locally since the merge base - '
                'the merge should apply cleanly.'
            )
            return '\n'.join(lines).rstrip()

        for tier in MERGE_TIER_ORDER:
            items = grouped.get(tier)
            if not items:
                continue
            lines.append(f'## {tier} ({len(items)})')
            lines.append(MERGE_TIER_DESCRIPTIONS[tier])
            for risk in sorted(items, key=lambda item: item.path):
                lines.append(f'- {risk.path} [{risk.area}]')
            lines.append('')

        if not self.conflicting:
            lines.append('No file was changed on both sides - the merge should apply cleanly.')
        return '\n'.join(lines).rstrip()


def _git(repo: Path, *args: str) -> str:
    proc = subprocess.run(
        ['git', *args],
        cwd=str(repo), capture_output=True, text=True, timeout=300,
    )
    if proc.returncode != 0:
        raise RuntimeError(f'git {" ".join(args)} failed: {proc.stderr.strip()}')
    return proc.stdout.strip()


def _changed_paths(repo: Path, base: str, ref: str) -> tuple[str, ...]:
    output = _git(repo, 'diff', '--name-only', f'{base}..{ref}')
    return tuple(line.strip() for line in output.splitlines() if line.strip())


def _upstream_paths(repo: Path, ref: str) -> set[str]:
    """Every path in the upstream tree, read once rather than once per file."""
    output = _git(repo, 'ls-tree', '-r', '--name-only', ref)
    return {line.strip() for line in output.splitlines() if line.strip()}


def merge_impact(repo: Path, upstream_ref: str = 'upstream/dev', head: str = 'HEAD') -> MergeImpactReport:
    """Report what merging ``upstream_ref`` would touch, without merging anything.

    Risk is decided by whether a path **exists upstream**, not by which
    directory it sits in - the test the firmware project specified. A file
    absent from upstream cannot conflict no matter where it lives, so a
    path-prefix rule would wrongly flag module-style code that has to sit in an
    upstream directory for its include path to resolve.
    """
    repo = Path(repo).resolve()
    base = _git(repo, 'merge-base', head, upstream_ref)
    upstream_changed = _changed_paths(repo, base, upstream_ref)
    local_changed = _changed_paths(repo, base, head)
    upstream_files = _upstream_paths(repo, upstream_ref)
    moved_upstream = set(upstream_changed)

    risks = []
    for path in sorted(local_changed):
        exists = path in upstream_files
        changed_upstream = path in moved_upstream
        if not exists:
            tier = 'low'
        elif changed_upstream:
            tier = 'high'
        else:
            tier = 'medium'
        risks.append(FileRisk(
            path=path,
            tier=tier,
            area=classify_path(path)[0],
            exists_upstream=exists,
            changed_upstream=changed_upstream,
        ))

    return MergeImpactReport(
        upstream_ref=upstream_ref,
        base=base,
        upstream_changed=upstream_changed,
        local_changed=local_changed,
        risks=tuple(risks),
    )
