from __future__ import annotations

import json
import tempfile
import unittest
from dataclasses import dataclass
from pathlib import Path

from sarina.memory_review import (
    MemoryReviewQueue,
    _extract_json_array,
    propose_memory_entries,
)
from sarina.memory_runtime import MemoryRuntime, MemoryRuntimeError


@dataclass
class _FakeTurn:
    content: str


class _FakeClient:
    def __init__(self, content: str) -> None:
        self._content = content
        self.last_messages = None

    def complete(self, messages, tools):  # noqa: ANN001
        self.last_messages = messages
        return _FakeTurn(content=self._content)


class ExtractJsonArrayTests(unittest.TestCase):
    def test_plain_json_array(self) -> None:
        self.assertEqual(_extract_json_array('[{"a": 1}]'), [{'a': 1}])

    def test_fenced_json_array(self) -> None:
        text = 'Sure, here you go:\n```json\n[{"a": 1}]\n```\nHope that helps.'
        self.assertEqual(_extract_json_array(text), [{'a': 1}])

    def test_embedded_array_without_fence(self) -> None:
        text = 'Result: [{"a": 1}] -- done'
        self.assertEqual(_extract_json_array(text), [{'a': 1}])

    def test_no_array_returns_none(self) -> None:
        self.assertIsNone(_extract_json_array('no json here'))


class ProposeMemoryEntriesTests(unittest.TestCase):
    def test_proposes_add_and_flags_blocked_content(self) -> None:
        payload = [
            {
                'scope': 'agent',
                'action': 'add',
                'old_text': None,
                'content': 'This repo pins numpy to 1.26 for onnxruntime compatibility.',
                'rationale': 'Discovered during dependency debugging.',
            },
            {
                'scope': 'user',
                'action': 'add',
                'old_text': None,
                'content': 'password: hunter2',
                'rationale': 'Should never be proposed for real, used to test the scanner gate.',
            },
        ]
        client = _FakeClient(json.dumps(payload))
        proposals = propose_memory_entries(
            'session-1',
            [
                {'role': 'user', 'content': 'Why does the ml extra fail to import numpy?'},
                {'role': 'assistant', 'content': 'Pinned numpy to 1.26 and it works now.'},
            ],
            client,
        )

        self.assertEqual(len(proposals), 2)
        statuses = {proposal.content: proposal.status for proposal in proposals}
        self.assertEqual(statuses['This repo pins numpy to 1.26 for onnxruntime compatibility.'], 'pending')
        self.assertEqual(statuses['password: hunter2'], 'blocked')

    def test_empty_transcript_short_circuits(self) -> None:
        client = _FakeClient('[]')
        proposals = propose_memory_entries('session-1', [], client)
        self.assertEqual(proposals, [])
        self.assertIsNone(client.last_messages)

    def test_malformed_response_yields_no_proposals(self) -> None:
        client = _FakeClient('I do not have anything worth remembering.')
        proposals = propose_memory_entries(
            'session-1',
            [{'role': 'user', 'content': 'just chatting'}],
            client,
        )
        self.assertEqual(proposals, [])

    def test_caps_proposals_at_five(self) -> None:
        payload = [
            {'scope': 'agent', 'action': 'add', 'content': f'fact {i}', 'rationale': 'r'}
            for i in range(10)
        ]
        client = _FakeClient(json.dumps(payload))
        proposals = propose_memory_entries(
            'session-1',
            [{'role': 'user', 'content': 'lots of facts'}],
            client,
        )
        self.assertEqual(len(proposals), 5)


class MemoryReviewQueueTests(unittest.TestCase):
    def test_approve_applies_mutation_to_memory_runtime(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            base = Path(tmp_dir)
            queue = MemoryReviewQueue(storage_path=base / 'pending.json')
            memory_runtime = MemoryRuntime(agent_path=base / 'agent.md', user_path=base / 'user.md')

            payload = [{'scope': 'agent', 'action': 'add', 'content': 'Reviewed fact worth keeping.', 'rationale': 'r'}]
            from sarina.memory_review import propose_memory_entries

            proposals = propose_memory_entries(
                'session-1',
                [{'role': 'user', 'content': 'discussing something durable'}],
                _FakeClient(json.dumps(payload)),
            )
            queue.add(proposals)
            self.assertEqual(len(queue.list_pending()), 1)

            entry_id = queue.list_pending()[0].entry_id
            mutation = queue.approve(entry_id, memory_runtime)
            self.assertEqual(mutation.action, 'add')
            self.assertIn('Reviewed fact worth keeping.', memory_runtime.entries('agent'))
            self.assertEqual(queue.list_pending(), [])

    def test_reject_marks_entry_resolved(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            base = Path(tmp_dir)
            queue = MemoryReviewQueue(storage_path=base / 'pending.json')
            payload = [{'scope': 'agent', 'action': 'add', 'content': 'Maybe not worth keeping.', 'rationale': 'r'}]
            from sarina.memory_review import propose_memory_entries

            proposals = propose_memory_entries('session-1', [{'role': 'user', 'content': 'x'}], _FakeClient(json.dumps(payload)))
            queue.add(proposals)
            entry_id = queue.list_pending()[0].entry_id
            queue.reject(entry_id)
            self.assertEqual(queue.list_pending(), [])

    def test_approve_unknown_entry_raises(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            base = Path(tmp_dir)
            queue = MemoryReviewQueue(storage_path=base / 'pending.json')
            memory_runtime = MemoryRuntime(agent_path=base / 'agent.md', user_path=base / 'user.md')
            with self.assertRaises(MemoryRuntimeError):
                queue.approve('does-not-exist', memory_runtime)

    def test_blocked_entry_cannot_be_approved(self) -> None:
        with tempfile.TemporaryDirectory() as tmp_dir:
            base = Path(tmp_dir)
            queue = MemoryReviewQueue(storage_path=base / 'pending.json')
            memory_runtime = MemoryRuntime(agent_path=base / 'agent.md', user_path=base / 'user.md')
            payload = [{'scope': 'agent', 'action': 'add', 'content': 'password: hunter2', 'rationale': 'r'}]
            from sarina.memory_review import propose_memory_entries

            proposals = propose_memory_entries('session-1', [{'role': 'user', 'content': 'x'}], _FakeClient(json.dumps(payload)))
            queue.add(proposals)
            blocked_id = queue.load()[0].entry_id
            with self.assertRaises(MemoryRuntimeError):
                queue.approve(blocked_id, memory_runtime)


if __name__ == '__main__':
    unittest.main()
