"""Discovers BlackArch's pacman category groups (``blackarch-*``) so the
harness doesn't rely on a hardcoded, easily-stale list.
"""

from __future__ import annotations

import subprocess

from .docker_runner import DEFAULT_IMAGE, STRAP_URL, docker_available

_DISCOVER_SCRIPT = (
    'set -o pipefail; '
    'pacman -Sy --noconfirm curl >/dev/null 2>&1; '
    f'curl -fsSL {STRAP_URL} | bash >/dev/null 2>&1; '
    'pacman -Sg | grep "^blackarch" || true'
)


def parse_category_list(pacman_sg_output: str) -> list[str]:
    """Parse ``pacman -Sg`` output into a sorted, deduped list of
    ``blackarch-*`` group names — excluding the catch-all ``blackarch``
    group itself, which installs everything at once (the thing this
    harness exists to avoid doing blindly).
    """
    seen: set[str] = set()
    for line in pacman_sg_output.splitlines():
        name = line.strip()
        if not name or name == 'blackarch':
            continue
        if name.startswith('blackarch'):
            seen.add(name)
    return sorted(seen)


def list_categories(
    *,
    image: str = DEFAULT_IMAGE,
    docker_bin: str = 'docker',
    timeout_seconds: float = 300.0,
) -> list[str]:
    """Discover category groups by straping BlackArch in a fresh container
    and listing its package groups. Never raises — returns an empty list
    if Docker isn't available or discovery fails; callers should treat an
    empty result as "nothing found", not as success.
    """
    if not docker_available(docker_bin):
        return []
    command = [docker_bin, 'run', '--rm', image, 'bash', '-c', _DISCOVER_SCRIPT]
    try:
        completed = subprocess.run(command, capture_output=True, text=True, timeout=timeout_seconds)
    except (subprocess.TimeoutExpired, OSError):
        return []
    if completed.returncode != 0:
        return []
    return parse_category_list(completed.stdout or '')
