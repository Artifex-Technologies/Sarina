"""Result types and rendering for the BlackArch compatibility matrix.

Pure data + formatting — no subprocess/Docker calls here, so this module
is fully unit-testable without Docker installed.
"""

from __future__ import annotations

import json
from dataclasses import dataclass, field, asdict


@dataclass(frozen=True)
class CategoryTestResult:
    categories: tuple[str, ...]
    ok: bool
    exit_code: int | None = None
    package_count: int = 0
    duration_sec: float = 0.0
    conflicts: tuple[str, ...] = ()
    stderr_tail: str = ''
    error: str | None = None

    @property
    def label(self) -> str:
        return '+'.join(self.categories)


@dataclass
class CompatibilityReport:
    image: str
    results: list[CategoryTestResult] = field(default_factory=list)

    @property
    def passed(self) -> list[CategoryTestResult]:
        return [r for r in self.results if r.ok]

    @property
    def failed(self) -> list[CategoryTestResult]:
        return [r for r in self.results if not r.ok]

    def to_json(self) -> str:
        payload = {
            'image': self.image,
            'total': len(self.results),
            'passed': len(self.passed),
            'failed': len(self.failed),
            'results': [
                {**asdict(r), 'categories': list(r.categories), 'conflicts': list(r.conflicts)}
                for r in self.results
            ],
        }
        return json.dumps(payload, indent=2)

    def to_markdown(self) -> str:
        lines = [
            '# BlackArch Compatibility Report',
            '',
            f'Image: `{self.image}`',
            f'Total: {len(self.results)} | Passed: {len(self.passed)} | Failed: {len(self.failed)}',
            '',
            '| Categories | Result | Packages | Duration (s) | Conflicts |',
            '|---|---|---|---|---|',
        ]
        for r in self.results:
            status = 'PASS' if r.ok else 'FAIL'
            conflicts = ', '.join(r.conflicts) if r.conflicts else '-'
            lines.append(
                f'| {r.label} | {status} | {r.package_count} | {r.duration_sec:.1f} | {conflicts} |'
            )
        if self.failed:
            lines.append('')
            lines.append('## Failures')
            for r in self.failed:
                lines.append('')
                lines.append(f'### {r.label}')
                if r.error:
                    lines.append(f'- error: {r.error}')
                if r.exit_code is not None:
                    lines.append(f'- exit_code: {r.exit_code}')
                if r.stderr_tail:
                    lines.append('```')
                    lines.append(r.stderr_tail)
                    lines.append('```')
        return '\n'.join(lines) + '\n'
