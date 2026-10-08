"""Client for the Chymaera_Zero_App Sarina control API.

Wire format is defined by that repo (``docs/SARINA_API.md``) and it owns the
contract: newline-delimited JSON over loopback TCP, one request object per
line, one response object per line. This module implements the client half
exactly as specified; it does not extend it. New commands get proposed on that
repo's board first.

The server is off by default and must be started from the app (Chymaera Console
UI, or ``Chymaera.startServer(port, token)``). A refused connection normally
means nobody turned it on.
"""

from __future__ import annotations

import json
import socket
from dataclasses import dataclass
from typing import Any

#: Default loopback port the app binds. Registered in docs/ECOSYSTEM.md.
DEFAULT_PORT = 44700
DEFAULT_HOST = '127.0.0.1'
DEFAULT_TIMEOUT = 5.0


class ConsoleError(RuntimeError):
    """The console answered ``{"ok": false, "error": ...}``."""

    def __init__(self, error: str, command: str) -> None:
        super().__init__(f'{command}: {error}')
        self.error = error
        self.command = command


class ConsoleUnavailable(RuntimeError):
    """The console could not be reached at all."""


@dataclass
class ConsoleClient:
    """One connection to the app's control API.

    Usable as a context manager to reuse a single socket across calls, which is
    what the line protocol is designed for::

        with ConsoleClient(token='...') as console:
            console.ping()
            captures = console.capture_list(limit=20)
    """

    host: str = DEFAULT_HOST
    port: int = DEFAULT_PORT
    token: str | None = None
    timeout: float = DEFAULT_TIMEOUT

    def __post_init__(self) -> None:
        self._socket: socket.socket | None = None
        self._reader: Any = None
        self._next_id = 1

    # -- connection ---------------------------------------------------------

    def __enter__(self) -> 'ConsoleClient':
        self.connect()
        return self

    def __exit__(self, *_exc: object) -> None:
        self.close()

    def connect(self) -> None:
        if self._socket is not None:
            return
        try:
            self._socket = socket.create_connection((self.host, self.port), timeout=self.timeout)
        except OSError as exc:
            raise ConsoleUnavailable(
                f'no Chymaera console at {self.host}:{self.port} ({exc}) - '
                'the app must be running with its Sarina server started'
            ) from exc
        self._reader = self._socket.makefile('r', encoding='utf-8', newline='\n')

    def close(self) -> None:
        if self._reader is not None:
            self._reader.close()
            self._reader = None
        if self._socket is not None:
            self._socket.close()
            self._socket = None

    # -- protocol -----------------------------------------------------------

    def request(self, cmd: str, **params: Any) -> dict[str, Any]:
        """Send one command and return its response payload.

        Raises ``ConsoleError`` when the console reports failure, and
        ``ConsoleUnavailable`` when the connection drops or is not there.
        """
        owns_connection = self._socket is None
        self.connect()
        assert self._socket is not None and self._reader is not None

        request_id = self._next_id
        self._next_id += 1
        payload: dict[str, Any] = {'cmd': cmd, 'id': request_id}
        payload.update({key: value for key, value in params.items() if value is not None})
        if self.token:
            payload['token'] = self.token

        try:
            self._socket.sendall((json.dumps(payload) + '\n').encode('utf-8'))
            line = self._reader.readline()
        except OSError as exc:
            self.close()
            raise ConsoleUnavailable(f'{cmd}: connection failed ({exc})') from exc

        if not line:
            self.close()
            raise ConsoleUnavailable(f'{cmd}: console closed the connection without replying')

        try:
            response = json.loads(line)
        except json.JSONDecodeError as exc:
            self.close()
            raise ConsoleUnavailable(f'{cmd}: console sent malformed JSON ({exc})') from exc

        if not isinstance(response, dict):
            self.close()
            raise ConsoleUnavailable(f'{cmd}: expected a JSON object, got {type(response).__name__}')

        if owns_connection:
            self.close()
        if not response.get('ok'):
            raise ConsoleError(str(response.get('error', 'unknown error')), cmd)
        return response

    # -- commands (exactly those in the app's SARINA_API.md) ----------------

    def ping(self) -> dict[str, Any]:
        return self.request('ping')

    def help(self) -> list[Any]:
        return list(self.request('help').get('commands', []))

    def status(self) -> dict[str, Any]:
        return self.request('status')

    def log_tail(self, limit: int = 50) -> list[dict[str, Any]]:
        return list(self.request('log.tail', limit=limit).get('entries', []))

    def log_add(self, message: str, severity: str | None = None) -> None:
        self.request('log.add', message=message, severity=severity)

    def session_start(self, name: str, notes: str | None = None) -> Any:
        return self.request('session.start', name=name, notes=notes).get('sessionId')

    def session_end(self) -> None:
        self.request('session.end')

    def capture_list(self, limit: int = 50) -> list[dict[str, Any]]:
        return list(self.request('capture.list', limit=limit).get('captures', []))

    def capture_import(self, path: str) -> Any:
        return self.request('capture.import', path=path).get('summary')


def render_status(status: dict[str, Any], captures: list[dict[str, Any]]) -> str:
    """Render a console status summary for humans."""
    lines = ['# Chymaera console', '']
    for key in ('version', 'serverRunning', 'serverPort', 'sessionActive',
                'sessionName', 'datastorePath', 'eventCount'):
        if key in status:
            lines.append(f'{key}: {status[key]}')
    lines.append('')
    if not captures:
        lines.append('No captures recorded.')
        return '\n'.join(lines)
    lines.append(f'## Captures ({len(captures)})')
    for capture in captures:
        name = capture.get('name', '?')
        kind = capture.get('type', '?')
        size = capture.get('size', '?')
        lines.append(f'- [{kind}] {name} ({size} bytes)')
    return '\n'.join(lines)
