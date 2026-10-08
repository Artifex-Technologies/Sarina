"""JSON-lines server for the Go side. See PROTOCOL.md."""

from __future__ import annotations

import json
from typing import Any, Callable, TextIO

from .classify import Classifier, prototypes_fingerprint
from .embed_group import group_captures

PROTOCOL_VERSION = 1


def handle(request: dict[str, Any], model: Any, classifier: Classifier) -> dict[str, Any]:
    op = request.get('op')
    if op == 'ping':
        return {'ok': True, 'protocol': PROTOCOL_VERSION, 'prototypes': prototypes_fingerprint()}
    if op == 'embed':
        texts = request.get('texts')
        if not isinstance(texts, list) or not all(isinstance(t, str) for t in texts):
            return {'ok': False, 'error': 'embed needs texts: [string]'}
        return {'ok': True, 'vectors': model.embed(texts)}
    if op == 'classify':
        text = request.get('text')
        if not isinstance(text, str):
            return {'ok': False, 'error': 'classify needs text: string'}
        label, score = classifier.classify(text)
        return {'ok': True, 'label': label, 'score': score}
    if op == 'group':
        captures = request.get('captures')
        threshold = request.get('threshold', 0.75)
        if not isinstance(captures, list) or not isinstance(threshold, (int, float)):
            return {'ok': False, 'error': 'group needs captures: [object] and threshold: number'}
        groups = group_captures([c for c in captures if isinstance(c, dict)], model, float(threshold))
        return {'ok': True, 'groups': groups}
    return {'ok': False, 'error': f'unknown op: {op!r}'}


def serve(model_factory: Callable[[], Any], stdin: TextIO, stdout: TextIO) -> int:
    """Load the model, then answer one request per line until EOF.

    A model that cannot load is reported as one ``{"ok": false, "error": ...}``
    line and exit code 1, so the caller can show the reason.
    """

    def emit(obj: dict[str, Any]) -> None:
        stdout.write(json.dumps(obj, separators=(',', ':')) + '\n')
        stdout.flush()

    try:
        model = model_factory()
        classifier = Classifier(model)
    except Exception as exc:  # any load failure is reported, never a traceback on the wire
        emit({'ok': False, 'error': str(exc)})
        return 1
    for line in stdin:
        line = line.strip()
        if not line:
            continue
        try:
            request = json.loads(line)
            if not isinstance(request, dict):
                raise ValueError('request must be an object')
            response = handle(request, model, classifier)
            if 'id' in request:
                response['id'] = request['id']
        except Exception as exc:
            response = {'ok': False, 'error': str(exc)}
        emit(response)
    return 0
