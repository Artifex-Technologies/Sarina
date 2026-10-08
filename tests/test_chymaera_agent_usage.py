"""Tests for Sarina's record in Omarchy's agents panel.

The record shape is Omarchy's own (read from its ``omarchy-agent-usage-codex``
collector at tag v4.0.4), so the keys asserted here are the ones the panel reads.
"""

from __future__ import annotations

import json
import os
import tempfile
import unittest
from datetime import datetime
from pathlib import Path

from sarina.chymaera.agent_usage import (
    build_record,
    collect_record,
    default_session_directories,
    read_sessions,
    usage_directory,
    write_record,
)

NOW = datetime(2026, 9, 26, 15, 0, 0)


def write_session(directory: Path, session_id: str, *, day: datetime, model: str = 'qwen3-coder',
                  turns: int = 4, input_tokens: int = 1000, output_tokens: int = 500,
                  cache_read: int = 0, cache_write: int = 0) -> Path:
    path = directory / f'{session_id}.json'
    path.write_text(json.dumps({
        'session_id': session_id,
        'model_config': {'model': model},
        'turns': turns,
        'usage': {
            'input_tokens': input_tokens,
            'output_tokens': output_tokens,
            'cache_read_input_tokens': cache_read,
            'cache_creation_input_tokens': cache_write,
        },
    }), encoding='utf-8')
    stamp = day.timestamp()
    os.utime(path, (stamp, stamp))
    return path


class RecordTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.dir = Path(self._tmp.name)

    def test_empty_directory_gives_a_valid_zero_record(self) -> None:
        record = collect_record([self.dir], now=NOW)
        self.assertEqual(record['id'], 'sarina')
        self.assertEqual(record['name'], 'Sarina')
        self.assertEqual(record['schemaVersion'], 1)
        self.assertTrue(record['ready'])
        self.assertEqual(record['totalSessions'], 0)
        self.assertEqual(len(record['recentDays']), 7)
        self.assertEqual(record['recentDays'][-1]['date'], '2026-09-26')
        self.assertEqual(record['recentDays'][0]['date'], '2026-09-20')

    def test_record_carries_every_key_the_panel_reads(self) -> None:
        # Main.qml reads exactly these off a record.
        required = {
            'id', 'name', 'ready', 'hasLocalStats', 'todayPrompts', 'todaySessions',
            'todayTotalTokens', 'todayTokensByModel', 'recentDays', 'totalPrompts',
            'totalSessions', 'activeDays', 'activeDates', 'modelUsage',
        }
        record = collect_record([self.dir], now=NOW)
        self.assertLessEqual(required, set(record))

    def test_today_and_earlier_days_are_bucketed_separately(self) -> None:
        write_session(self.dir, 'a', day=datetime(2026, 9, 26, 9, 0), input_tokens=100, output_tokens=50, turns=3)
        write_session(self.dir, 'b', day=datetime(2026, 9, 24, 9, 0), input_tokens=200, output_tokens=100, turns=2)
        record = collect_record([self.dir], now=NOW)
        self.assertEqual(record['todayTotalTokens'], 150)
        self.assertEqual(record['todaySessions'], 1)
        self.assertEqual(record['todayPrompts'], 3)
        self.assertEqual(record['totalSessions'], 2)
        self.assertEqual(record['totalPrompts'], 5)
        by_day = {day['date']: day['messageCount'] for day in record['recentDays']}
        self.assertEqual(by_day['2026-09-26'], 150)
        self.assertEqual(by_day['2026-09-24'], 300)
        self.assertEqual(record['activeDays'], 2)
        self.assertEqual(record['activeDates'], ['2026-09-24', '2026-09-26'])

    def test_sessions_older_than_a_week_count_only_toward_totals(self) -> None:
        write_session(self.dir, 'old', day=datetime(2026, 8, 1, 9, 0), input_tokens=999, output_tokens=1)
        record = collect_record([self.dir], now=NOW)
        self.assertEqual(sum(day['messageCount'] for day in record['recentDays']), 0)
        self.assertEqual(record['totalSessions'], 1)
        self.assertEqual(record['modelUsage']['qwen3-coder']['inputTokens'], 999)

    def test_cache_tokens_are_summed_into_totals_and_kept_per_model(self) -> None:
        write_session(self.dir, 'c', day=NOW, input_tokens=10, output_tokens=20, cache_read=30, cache_write=40)
        record = collect_record([self.dir], now=NOW)
        self.assertEqual(record['todayTotalTokens'], 100)
        usage = record['modelUsage']['qwen3-coder']
        self.assertEqual(usage['cacheReadInputTokens'], 30)
        self.assertEqual(usage['cacheCreationInputTokens'], 40)

    def test_tokens_are_split_by_model(self) -> None:
        write_session(self.dir, 'a', day=NOW, model='m1', input_tokens=100, output_tokens=0)
        write_session(self.dir, 'b', day=NOW, model='m2', input_tokens=300, output_tokens=0)
        record = collect_record([self.dir], now=NOW)
        self.assertEqual(record['todayTokensByModel'], {'m1': 100, 'm2': 300})

    def test_broken_and_foreign_files_are_skipped(self) -> None:
        (self.dir / 'garbage.json').write_text('not json', encoding='utf-8')
        (self.dir / 'list.json').write_text('[1, 2]', encoding='utf-8')
        (self.dir / 'nousage.json').write_text(json.dumps({'session_id': 'x'}), encoding='utf-8')
        write_session(self.dir, 'good', day=NOW)
        self.assertEqual([s.session_id for s in read_sessions([self.dir])], ['good'])

    def test_hostile_token_values_become_zero(self) -> None:
        path = self.dir / 'evil.json'
        path.write_text(json.dumps({
            'session_id': 'evil',
            'turns': -5,
            'usage': {'input_tokens': -1, 'output_tokens': 'lots', 'cache_read_input_tokens': True,
                      'cache_creation_input_tokens': None},
        }), encoding='utf-8')
        (session,) = read_sessions([self.dir])
        self.assertEqual(session.total_tokens, 0)
        self.assertEqual(session.prompts, 0)
        self.assertEqual(session.model, 'unknown')

    def test_same_session_in_two_directories_is_counted_once_newest_wins(self) -> None:
        other = self.dir / 'other'
        other.mkdir()
        write_session(self.dir, 'dup', day=datetime(2026, 9, 25, 9, 0), input_tokens=1, output_tokens=0)
        write_session(other, 'dup', day=datetime(2026, 9, 26, 9, 0), input_tokens=500, output_tokens=0)
        record = collect_record([self.dir, other], now=NOW)
        self.assertEqual(record['totalSessions'], 1)
        self.assertEqual(record['todayTotalTokens'], 500)

    def test_missing_directory_is_not_an_error(self) -> None:
        self.assertEqual(read_sessions([self.dir / 'nope']), [])


class LocationTests(unittest.TestCase):
    def test_usage_directory_honours_xdg_state_home(self) -> None:
        self.assertEqual(
            usage_directory({'XDG_STATE_HOME': '/s'}),
            Path('/s') / 'omarchy' / 'agents' / 'usage',
        )

    def test_usage_directory_defaults_under_home(self) -> None:
        path = usage_directory({})
        self.assertEqual(path.parts[-5:], ('.local', 'state', 'omarchy', 'agents', 'usage'))

    def test_session_dirs_env_replaces_defaults(self) -> None:
        dirs = default_session_directories({'SARINA_SESSION_DIRS': os.pathsep.join(['/a', '/b'])})
        self.assertEqual(dirs, (Path('/a'), Path('/b')))

    def test_default_session_dirs_include_cwd_home_and_service(self) -> None:
        dirs = default_session_directories({}, cwd=Path('/work'), home=Path('/home/u'))
        self.assertEqual(dirs[0], Path('/work') / '.port_sessions' / 'agent')
        self.assertEqual(dirs[1], Path('/home/u') / '.port_sessions' / 'agent')
        self.assertEqual(dirs[2], Path('/var/lib/sarina/sessions'))


class WriteTests(unittest.TestCase):
    def test_write_is_atomic_and_readable_back(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp) / 'nested' / 'usage'
            record = build_record([], now=NOW)
            path = write_record(record, target)
            self.assertEqual(path, target / 'sarina.json')
            self.assertEqual(json.loads(path.read_text(encoding='utf-8'))['id'], 'sarina')
            # No temp files left behind for the panel to trip over.
            self.assertEqual([p.name for p in target.iterdir()], ['sarina.json'])

    def test_second_write_replaces_the_first(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            target = Path(tmp)
            write_record({'id': 'sarina', 'n': 1}, target)
            write_record({'id': 'sarina', 'n': 2}, target)
            self.assertEqual(json.loads((target / 'sarina.json').read_text(encoding='utf-8'))['n'], 2)


if __name__ == '__main__':
    unittest.main()
