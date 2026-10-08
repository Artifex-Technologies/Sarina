"""Tests for the Chymaera_Zero_App console client.

Runs against a real loopback socket server speaking the app's documented
newline-delimited JSON protocol, so the framing is genuinely exercised rather
than mocked away.
"""

import json
import socket
import threading
import unittest

from sarina.chymaera.console import (
    ConsoleClient,
    ConsoleError,
    ConsoleUnavailable,
    render_status,
)


class FakeConsole:
    """Minimal stand-in for the app's Sarina server."""

    def __init__(self, handler=None, token=None, raw_reply=None):
        self.handler = handler or self._default_handler
        self.token = token
        self.raw_reply = raw_reply
        self.requests = []
        self._server = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        self._server.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
        self._server.bind(('127.0.0.1', 0))
        self._server.listen(1)
        self.port = self._server.getsockname()[1]
        self._thread = threading.Thread(target=self._serve, daemon=True)
        self._thread.start()

    def _default_handler(self, request):
        cmd = request.get('cmd')
        if cmd == 'ping':
            return {'ok': True, 'pong': True, 'version': '1.2.3'}
        if cmd == 'status':
            return {'ok': True, 'serverRunning': True, 'serverPort': self.port,
                    'sessionActive': False, 'eventCount': 4}
        if cmd == 'help':
            return {'ok': True, 'commands': ['ping', 'status']}
        if cmd == 'log.tail':
            return {'ok': True, 'entries': [{'message': 'hello', 'severity': 'info'}]}
        if cmd == 'capture.list':
            return {'ok': True, 'captures': [
                {'id': 1, 'type': 'nfc', 'name': 'badge.nfc', 'size': 512}]}
        if cmd == 'session.start':
            return {'ok': True, 'sessionId': 42}
        if cmd in {'log.add', 'session.end'}:
            return {'ok': True}
        return {'ok': False, 'error': 'unknown command'}

    def _serve(self):
        try:
            conn, _addr = self._server.accept()
        except OSError:
            return
        with conn, conn.makefile('r', encoding='utf-8', newline='\n') as reader:
            for line in reader:
                line = line.strip()
                if not line:
                    continue
                try:
                    request = json.loads(line)
                except json.JSONDecodeError:
                    return
                self.requests.append(request)
                if self.raw_reply is not None:
                    try:
                        conn.sendall((self.raw_reply + '\n').encode('utf-8'))
                    except OSError:
                        pass
                    return
                if self.token and request.get('token') != self.token:
                    response = {'ok': False, 'error': 'unauthorized'}
                else:
                    response = self.handler(request)
                if response is None:      # simulate a server that hangs up
                    return
                if 'id' in request and 'id' not in response:
                    response['id'] = request['id']
                try:
                    conn.sendall((json.dumps(response) + '\n').encode('utf-8'))
                except OSError:
                    return

    def close(self):
        self._server.close()


class ConsoleClientTest(unittest.TestCase):
    def setUp(self):
        self.console = FakeConsole()
        self.addCleanup(self.console.close)

    def client(self, **kwargs):
        return ConsoleClient(port=self.console.port, timeout=5.0, **kwargs)

    def test_ping(self):
        with self.client() as client:
            self.assertEqual(client.ping()['version'], '1.2.3')

    def test_status(self):
        with self.client() as client:
            self.assertTrue(client.status()['serverRunning'])

    def test_help_returns_command_list(self):
        with self.client() as client:
            self.assertIn('ping', client.help())

    def test_log_tail_returns_entries(self):
        with self.client() as client:
            self.assertEqual(client.log_tail()[0]['message'], 'hello')

    def test_capture_list_returns_records(self):
        with self.client() as client:
            self.assertEqual(client.capture_list()[0]['name'], 'badge.nfc')

    def test_session_start_returns_id(self):
        with self.client() as client:
            self.assertEqual(client.session_start('engagement-1'), 42)

    def test_request_ids_increment_and_are_sent(self):
        with self.client() as client:
            client.ping()
            client.ping()
        ids = [r['id'] for r in self.console.requests]
        self.assertEqual(ids, [1, 2])

    def test_optional_params_are_omitted_when_none(self):
        with self.client() as client:
            client.session_start('run', notes=None)
        self.assertNotIn('notes', self.console.requests[-1])

    def test_optional_params_are_sent_when_given(self):
        with self.client() as client:
            client.session_start('run', notes='context')
        self.assertEqual(self.console.requests[-1]['notes'], 'context')

    def test_limit_is_forwarded(self):
        with self.client() as client:
            client.capture_list(limit=7)
        self.assertEqual(self.console.requests[-1]['limit'], 7)

    def test_error_response_raises_console_error(self):
        with self.client() as client:
            with self.assertRaises(ConsoleError) as ctx:
                client.request('nope')
        self.assertEqual(ctx.exception.error, 'unknown command')

    def test_no_token_sent_when_unset(self):
        with self.client() as client:
            client.ping()
        self.assertNotIn('token', self.console.requests[-1])


class ConsoleAuthTest(unittest.TestCase):
    def setUp(self):
        self.console = FakeConsole(token='secret')
        self.addCleanup(self.console.close)

    def test_correct_token_is_accepted(self):
        with ConsoleClient(port=self.console.port, token='secret') as client:
            self.assertTrue(client.ping()['pong'])

    def test_wrong_token_raises_unauthorized(self):
        with ConsoleClient(port=self.console.port, token='wrong') as client:
            with self.assertRaises(ConsoleError) as ctx:
                client.ping()
        self.assertEqual(ctx.exception.error, 'unauthorized')


class ConsoleFailureTest(unittest.TestCase):
    def test_no_server_raises_unavailable(self):
        # Bind and immediately close to get a port nothing is listening on.
        probe = socket.socket(socket.AF_INET, socket.SOCK_STREAM)
        probe.bind(('127.0.0.1', 0))
        port = probe.getsockname()[1]
        probe.close()
        with self.assertRaises(ConsoleUnavailable):
            ConsoleClient(port=port, timeout=2.0).ping()

    def test_server_hangup_raises_unavailable(self):
        console = FakeConsole(handler=lambda _request: None)
        self.addCleanup(console.close)
        with self.assertRaises(ConsoleUnavailable):
            ConsoleClient(port=console.port, timeout=5.0).ping()

    def test_malformed_json_from_server_raises_unavailable(self):
        console = FakeConsole(raw_reply='this is not json')
        self.addCleanup(console.close)
        with self.assertRaises(ConsoleUnavailable):
            ConsoleClient(port=console.port, timeout=5.0).ping()

    def test_non_object_response_raises_unavailable(self):
        console = FakeConsole(raw_reply='["a list, not an object"]')
        self.addCleanup(console.close)
        with self.assertRaises(ConsoleUnavailable):
            ConsoleClient(port=console.port, timeout=5.0).ping()


class RenderStatusTest(unittest.TestCase):
    def test_renders_fields_and_captures(self):
        text = render_status(
            {'version': '1.0', 'serverRunning': True, 'eventCount': 2},
            [{'type': 'nfc', 'name': 'badge.nfc', 'size': 512}],
        )
        self.assertIn('version: 1.0', text)
        self.assertIn('[nfc] badge.nfc', text)

    def test_handles_no_captures(self):
        self.assertIn('No captures recorded.', render_status({'version': '1.0'}, []))


if __name__ == '__main__':
    unittest.main()
