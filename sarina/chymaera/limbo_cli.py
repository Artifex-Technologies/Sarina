"""``sarina limbo-profiles`` and ``sarina limbo-validate``.

Kept out of ``main.py`` (largely derived from an unlicensed upstream; see
``docs/PROVENANCE.md``), which carries only the hooks that call
:func:`add_limbo_commands` and :func:`run_limbo_command`.
"""

from __future__ import annotations

import argparse

from . import limbo as limbo_client

LIMBO_COMMANDS = ('limbo-profiles', 'limbo-validate')


def add_limbo_commands(subparsers: argparse._SubParsersAction) -> None:
    subparsers.add_parser(
        'limbo-profiles',
        help='list the validation profiles Limbo offers, with what each allows (needs SARINA_LIMBO_SOCKET)',
    )
    validate = subparsers.add_parser(
        'limbo-validate',
        help='run a command in a disposable Limbo guest and show the result',
        usage='%(prog)s [--timeout SECONDS] PROFILE COMMAND [ARG ...]',
    )
    validate.add_argument('--timeout', type=int, default=300, help='seconds for the command; Limbo caps this per profile')
    validate.add_argument('profile')
    # dest must not be 'command': main() dispatches on args.command, which is the
    # subcommand's name, and a positional called 'command' overwrote it. REMAINDER
    # so that dashes inside the guest command (pacman -Syu --noconfirm) are not
    # parsed as options of this program; options for this program go before PROFILE.
    validate.add_argument('guest_command', nargs=argparse.REMAINDER, metavar='COMMAND')


def run_limbo_command(args: argparse.Namespace) -> int:
    if args.command == 'limbo-profiles':
        try:
            profiles = limbo_client.describe_profiles()
        except limbo_client.LimboError as exc:
            print(exc)
            return 1
        if not profiles:
            print('No validation profiles. A human adds them under /etc/limbo/validation/.')
        for profile in profiles:
            print(profile.as_text())
        return 0
    if args.command == 'limbo-validate':
        parts = list(args.guest_command)
        if parts[:1] == ['--']:
            parts = parts[1:]
        if not parts:
            print('limbo-validate needs a command to run, e.g.: sarina limbo-validate PROFILE uname -r')
            return 2
        try:
            run = limbo_client.validate(args.profile, ' '.join(parts), timeout_seconds=args.timeout)
        except limbo_client.LimboError as exc:
            print(exc)
            return 1
        print(run.as_text())
        return 0 if run.succeeded else 1
    raise ValueError(f'not a Limbo command: {args.command}')
