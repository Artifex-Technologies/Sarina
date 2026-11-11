"""Runs one BlackArch category (or an explicit combination) inside a
disposable Docker container and reports whether the install went cleanly.

Mirrors the tolerant-subprocess-wrapping pattern used for the Claude Code
CLI escalation path (``sarina/escalation_runtime.py``):
timeouts and launch failures are caught and turned into a failed result
rather than raised, since this is meant to run unattended across many
categories.

NOTE: none of this has been run against a real Docker daemon yet — this
machine doesn't have Docker installed. See blackarch_compat/README.md.
"""

from __future__ import annotations

import shutil
import subprocess
import time

from .report import CategoryTestResult

DEFAULT_IMAGE = 'archlinux:latest'
STRAP_URL = 'https://blackarch.org/strap.sh'
DEFAULT_TIMEOUT_SECONDS = 900.0

_CONFLICT_MARKERS = (
    'conflicting dependencies',
    'unable to satisfy',
    'exists in filesystem',
    'is invalid or corrupted',
)


def docker_available(docker_bin: str = 'docker') -> bool:
    return shutil.which(docker_bin) is not None


def _normalize_group_name(category: str) -> str:
    stripped = category.strip()
    return stripped if stripped.startswith('blackarch') else f'blackarch-{stripped}'


def _build_test_script(categories: tuple[str, ...]) -> str:
    groups = ' '.join(_normalize_group_name(c) for c in categories)
    return (
        'set -o pipefail; '
        'pacman -Sy --noconfirm curl >/dev/null 2>&1; '
        f'curl -fsSL {STRAP_URL} | bash; '
        'pacman -Syu --noconfirm; '
        f'pacman -S --noconfirm {groups}; '
        'echo "___PACKAGE_COUNT___$(pacman -Q | wc -l)"; '
        'pacman -Qk'
    )


def run_category_test(
    categories: tuple[str, ...],
    *,
    image: str = DEFAULT_IMAGE,
    docker_bin: str = 'docker',
    timeout_seconds: float = DEFAULT_TIMEOUT_SECONDS,
) -> CategoryTestResult:
    if not categories:
        return CategoryTestResult(categories=categories, ok=False, error='no categories given')

    if not docker_available(docker_bin):
        return CategoryTestResult(categories=categories, ok=False, error=f'{docker_bin!r} not found on PATH')

    script = _build_test_script(categories)
    command = [docker_bin, 'run', '--rm', image, 'bash', '-c', script]

    start = time.monotonic()
    try:
        completed = subprocess.run(command, capture_output=True, text=True, timeout=timeout_seconds)
    except subprocess.TimeoutExpired as exc:
        return CategoryTestResult(
            categories=categories,
            ok=False,
            duration_sec=time.monotonic() - start,
            error=f'timed out after {timeout_seconds}s: {exc}',
        )
    except OSError as exc:
        return CategoryTestResult(categories=categories, ok=False, error=f'failed to launch docker: {exc}')

    duration = time.monotonic() - start
    stdout = completed.stdout or ''
    stderr = completed.stderr or ''
    package_count = _extract_package_count(stdout)
    conflicts = _extract_conflicts(stdout + '\n' + stderr)

    return CategoryTestResult(
        categories=categories,
        ok=completed.returncode == 0 and not conflicts,
        exit_code=completed.returncode,
        package_count=package_count,
        duration_sec=duration,
        conflicts=conflicts,
        stderr_tail=stderr[-2000:],
    )


def _extract_package_count(stdout: str) -> int:
    marker = '___PACKAGE_COUNT___'
    for line in stdout.splitlines():
        if line.startswith(marker):
            try:
                return int(line[len(marker):].strip())
            except ValueError:
                return 0
    return 0


def _extract_conflicts(output: str) -> tuple[str, ...]:
    conflicts: list[str] = []
    for line in output.splitlines():
        lowered = line.lower()
        if any(marker in lowered for marker in _CONFLICT_MARKERS):
            conflicts.append(line.strip())
    return tuple(conflicts)
