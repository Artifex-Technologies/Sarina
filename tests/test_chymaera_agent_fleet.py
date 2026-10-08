"""Tests for reading other agents' usage records.

Fixtures are shaped after the records Omarchy's own collectors print
(``limits`` entries of ``label``/``percent``/``resetsAt``; ``recentDays`` of
``date``/``messageCount``). The reader treats every record as untrusted data, so
much of this file is hostile input.
"""

from __future__ import annotations

import json
import tempfile
import unittest
from datetime import datetime, timedelta, timezone
from pathlib import Path

from sarina.chymaera.agent_fleet import (
    MAX_RECORD_BYTES,
    parse_record,
    read_agent_status,
    render_status,
    suggest_agent,
)

NOW = datetime(2026, 9, 26, 18, 0, 0, tzinfo=timezone.utc)
FRESH = (NOW - timedelta(minutes=5)).isoformat()


def record(agent_id: str, **overrides) -> dict:
    base = {
        'schemaVersion': 1,
        'id': agent_id,
        'name': agent_id.title(),
        'updatedAt': FRESH,
        'ready': True,
        'todayTotalTokens': 1200,
        'totalSessions': 4,
        'recentDays': [{'date': '2026-09-25', 'messageCount': 800}, {'date': '2026-09-26', 'messageCount': 1200}],
        'limits': [
            {'label': 'Session (5-hour)', 'percent': 30, 'resetsAt': '2026-09-26T20:00:00Z'},
            {'label': 'Weekly (7-day)', 'percent': 55, 'resetsAt': '2026-09-30T00:00:00Z'},
        ],
    }
    base.update(overrides)
    return base


class ParseTests(unittest.TestCase):
    def test_a_real_shaped_record_parses(self) -> None:
        status = parse_record(record('claude'), now=NOW)
        assert status is not None
        self.assertEqual(status.agent_id, 'claude')
        self.assertEqual(status.today_tokens, 1200)
        self.assertEqual(status.week_tokens, 2000)
        self.assertEqual(len(status.limits), 2)
        self.assertFalse(status.stale)

    def test_headroom_is_the_tightest_window(self) -> None:
        status = parse_record(record('claude'), now=NOW)
        assert status is not None
        self.assertEqual(status.headroom_percent, 45.0)

    def test_no_limits_means_unknown_headroom_not_unlimited(self) -> None:
        status = parse_record(record('local', limits=[]), now=NOW)
        assert status is not None
        self.assertIsNone(status.headroom_percent)

    def test_old_record_is_stale(self) -> None:
        old = (NOW - timedelta(hours=3)).isoformat()
        status = parse_record(record('claude', updatedAt=old), now=NOW)
        assert status is not None
        self.assertTrue(status.stale)

    def test_missing_or_bad_timestamp_is_stale(self) -> None:
        for value in (None, 'yesterday', 12345):
            status = parse_record(record('claude', updatedAt=value), now=NOW)
            assert status is not None
            self.assertTrue(status.stale, value)

    def test_rejects_non_records_and_bad_ids(self) -> None:
        self.assertIsNone(parse_record([], now=NOW))
        self.assertIsNone(parse_record('x', now=NOW))
        for bad in (None, '', 'Claude', '../etc', 'a b', 'x' * 40, '-lead'):
            self.assertIsNone(parse_record(record('ok', id=bad), now=NOW), bad)

    def test_control_characters_are_stripped_from_text(self) -> None:
        status = parse_record(
            record('claude', name='Cl\x1b[31maude\x07‮', usageStatusText='ok\x00\nline2'),
            now=NOW,
        )
        assert status is not None
        self.assertEqual(status.name, 'Cl[31maude')
        self.assertNotIn('\x1b', status.name)
        self.assertNotIn('\n', status.status_text)

    def test_text_is_truncated(self) -> None:
        status = parse_record(record('claude', name='N' * 500), now=NOW)
        assert status is not None
        self.assertEqual(len(status.name), 80)

    def test_hostile_numbers_are_neutralised(self) -> None:
        status = parse_record(
            record(
                'claude',
                todayTotalTokens=float('inf'),
                totalSessions=-3,
                recentDays=[{'messageCount': 'lots'}, {'messageCount': True}, 'junk', {'messageCount': 5}],
                limits=[{'label': 'x', 'percent': 9999}, {'label': 'y', 'percent': 'high'}, 'junk'],
            ),
            now=NOW,
        )
        assert status is not None
        self.assertEqual(status.today_tokens, 0)
        self.assertEqual(status.total_sessions, 0)
        self.assertEqual(status.week_tokens, 5)
        self.assertEqual(status.limits[0].percent_used, 100.0)
        self.assertIsNone(status.limits[1].percent_used)

    def test_limit_list_is_capped(self) -> None:
        status = parse_record(record('claude', limits=[{'label': str(i), 'percent': 1} for i in range(500)]), now=NOW)
        assert status is not None
        self.assertEqual(len(status.limits), 16)


class DirectoryTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.dir = Path(self._tmp.name)

    def put(self, name: str, data) -> None:
        text = data if isinstance(data, str) else json.dumps(data)
        (self.dir / name).write_text(text, encoding='utf-8')

    def test_reads_sorted_and_skips_junk(self) -> None:
        self.put('codex.json', record('codex'))
        self.put('claude.json', record('claude'))
        self.put('broken.json', '{nope')
        self.put('list.json', [1])
        self.put('notes.txt', 'ignored')
        statuses = read_agent_status(self.dir, now=NOW)
        self.assertEqual([s.agent_id for s in statuses], ['claude', 'codex'])

    def test_oversized_file_is_skipped(self) -> None:
        self.put('big.json', ' ' * (MAX_RECORD_BYTES + 1))
        self.assertEqual(read_agent_status(self.dir, now=NOW), ())

    def test_missing_directory_is_empty(self) -> None:
        self.assertEqual(read_agent_status(self.dir / 'nope', now=NOW), ())


class SuggestTests(unittest.TestCase):
    def status(self, agent_id: str, percent: float | None, **kwargs):
        limits = [] if percent is None else [{'label': 'w', 'percent': percent}]
        parsed = parse_record(record(agent_id, limits=limits, **kwargs), now=NOW)
        assert parsed is not None
        return parsed

    def test_picks_the_most_headroom(self) -> None:
        statuses = (self.status('claude', 80), self.status('codex', 20))
        pick = suggest_agent(statuses)
        assert pick is not None
        self.assertEqual(pick.agent_id, 'codex')

    def test_tie_goes_to_fewer_tokens_today(self) -> None:
        statuses = (
            self.status('claude', 50, todayTotalTokens=9000),
            self.status('codex', 50, todayTotalTokens=100),
        )
        pick = suggest_agent(statuses)
        assert pick is not None
        self.assertEqual(pick.agent_id, 'codex')

    def test_ignores_stale_unready_unmeasured_and_sarina(self) -> None:
        old = (NOW - timedelta(hours=5)).isoformat()
        statuses = (
            self.status('stale', 0, updatedAt=old),
            self.status('unready', 0, ready=False),
            self.status('unknown', None),
            self.status('sarina', 0),
        )
        self.assertIsNone(suggest_agent(statuses))

    def test_no_agents_no_suggestion(self) -> None:
        self.assertIsNone(suggest_agent(()))


class RenderTests(unittest.TestCase):
    def test_empty(self) -> None:
        self.assertEqual(render_status(()), 'No agent usage records found.')

    def test_shows_limits_and_flags(self) -> None:
        old = (NOW - timedelta(hours=5)).isoformat()
        text = render_status((
            parse_record(record('claude'), now=NOW),
            parse_record(record('codex', updatedAt=old, limits=[]), now=NOW),
        ))
        self.assertIn('Claude (claude): 1.2k today', text)
        self.assertIn('Session (5-hour): 30% used', text)
        self.assertIn('[stale]', text)
        self.assertIn('no limit data', text)


if __name__ == '__main__':
    unittest.main()
