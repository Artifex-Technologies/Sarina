"""``python -m sarina_helpers <serve|memory-search|voice-chat> ...``"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path


def _serve(argv: list[str]) -> int:
    from .embeddings import EmbeddingModel
    from .serve import serve

    p = argparse.ArgumentParser(prog='sarina_helpers serve')
    p.add_argument('--model-dir', required=True)
    args = p.parse_args(argv)
    return serve(lambda: EmbeddingModel(args.model_dir), sys.stdin, sys.stdout)


def _memory_search(argv: list[str]) -> int:
    from .embeddings import EmbeddingModel, MLUnavailable
    from .memory_search import format_results, index_sessions, search_sessions

    p = argparse.ArgumentParser(prog='sarina memory-search')
    p.add_argument('query', nargs='+')
    p.add_argument('--model-dir', required=True)
    p.add_argument('--session-dir')
    p.add_argument('--cwd', default='.')
    p.add_argument('--top-k', type=int, default=10)
    args = p.parse_args(argv)
    directory = Path(args.session_dir) if args.session_dir else Path(args.cwd) / '.port_sessions' / 'agent'
    try:
        model = EmbeddingModel(args.model_dir)
    except MLUnavailable as exc:
        print(str(exc))
        return 1
    if not index_sessions(directory):
        print('No saved sessions found.')
        return 0
    print(format_results(search_sessions(' '.join(args.query), model, directory, args.top_k)))
    return 0


def main(argv: list[str]) -> int:
    if argv and argv[0] == 'serve':
        return _serve(argv[1:])
    if argv and argv[0] == 'memory-search':
        return _memory_search(argv[1:])
    if argv and argv[0] == 'voice-chat':
        from .voice_bridge import main as voice_main
        return voice_main(argv[1:])
    print('usage: python -m sarina_helpers {serve|memory-search|voice-chat} ...', file=sys.stderr)
    return 2


if __name__ == '__main__':
    sys.exit(main(sys.argv[1:]))
