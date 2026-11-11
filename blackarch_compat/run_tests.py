#!/usr/bin/env python3
"""
BlackArch category compatibility test runner.

Installs BlackArch pacman groups one at a time (or, with --combo, together
as a set) inside disposable `archlinux:latest` Docker containers, and
reports which combinations install cleanly.

Requires Docker. See blackarch_compat/README.md for setup and
current verification status.

Usage:
    # Discover every blackarch-* category and test each one alone
    python3 -m blackarch_compat.run_tests --all

    # Test specific categories, each in its own container
    python3 -m blackarch_compat.run_tests --categories webapp,scanner

    # Test specific categories together, in one container
    python3 -m blackarch_compat.run_tests --categories webapp,scanner --combo

    # Save a JSON + Markdown report
    python3 -m blackarch_compat.run_tests --all -o report.json --markdown report.md
"""

from __future__ import annotations

import argparse
import sys

from .categories import list_categories
from .docker_runner import DEFAULT_IMAGE, run_category_test
from .report import CompatibilityReport


def _parse_categories(raw: str) -> list[str]:
    return [c.strip() for c in raw.split(',') if c.strip()]


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(
        prog='blackarch-compat',
        description='Test BlackArch category installs in disposable Docker containers.',
    )
    parser.add_argument('--all', action='store_true', help='Discover and test every blackarch-* category alone.')
    parser.add_argument('--categories', type=_parse_categories, default=None, help='Comma-separated category names (with or without the blackarch- prefix).')
    parser.add_argument('--combo', action='store_true', help='Test --categories together in one container instead of one at a time.')
    parser.add_argument('--image', default=DEFAULT_IMAGE, help=f'Base Docker image (default: {DEFAULT_IMAGE}).')
    parser.add_argument('--timeout-seconds', type=float, default=900.0)
    parser.add_argument('-o', '--output', default=None, help='Write the JSON report to this path.')
    parser.add_argument('--markdown', default=None, help='Write the Markdown report to this path.')

    args = parser.parse_args(argv)

    if not args.all and not args.categories:
        parser.error('pass --all or --categories a,b,c')

    if args.all:
        print(f'Discovering BlackArch categories from {args.image}...')
        categories = list_categories(image=args.image)
        if not categories:
            print('No categories discovered (Docker unavailable, or discovery failed). Nothing to test.')
            return 1
        print(f'Found {len(categories)} categories.')
    else:
        categories = args.categories

    report = CompatibilityReport(image=args.image)

    if args.combo:
        print(f'Testing {len(categories)} categories together: {", ".join(categories)}')
        result = run_category_test(tuple(categories), image=args.image, timeout_seconds=args.timeout_seconds)
        report.results.append(result)
        print(f'  -> {"PASS" if result.ok else "FAIL"}')
    else:
        for category in categories:
            print(f'Testing {category}...')
            result = run_category_test((category,), image=args.image, timeout_seconds=args.timeout_seconds)
            report.results.append(result)
            print(f'  -> {"PASS" if result.ok else "FAIL"}')

    print()
    print(f'Passed: {len(report.passed)}/{len(report.results)}')

    if args.output:
        with open(args.output, 'w', encoding='utf-8') as fh:
            fh.write(report.to_json())
        print(f'JSON report written to {args.output}')

    if args.markdown:
        with open(args.markdown, 'w', encoding='utf-8') as fh:
            fh.write(report.to_markdown())
        print(f'Markdown report written to {args.markdown}')

    return 0 if not report.failed else 1


if __name__ == '__main__':
    sys.exit(main())
