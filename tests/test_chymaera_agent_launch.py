"""Tests for the proposal-first agent launcher.

The point of this module is what it refuses to do, so most tests are refusals:
no auto-approve flags, nothing runs unconfirmed, every attempt is audited.
"""

from __future__ import annotations

import json
import subprocess
import tempfile
import unittest
from pathlib import Path

from sarina.chymaera.agent_launch import (
    UNSUPPORTED,
    LaunchError,
    audit_log_path,
    launch,
    plan_launch,
    supported_agents,
)

# The spellings Omarchy uses to start each agent without permission prompts
# (bin/omarchy-agent at v4.0.4). None of these may ever appear in a proposal.
AUTO_APPROVE_FLAGS = (
    '--auto', '--yolo', '--allow-all', '--permission-mode', 'bypassPermissions',
    '--approve-for-me', '--auto-approve', '--approval-mode', '--trust', 'never',
)


def found(name: str) -> str:
    return f'/usr/bin/{name}'


class PlanTests(unittest.TestCase):
    def test_no_supported_agent_gets_an_auto_approve_flag(self) -> None:
        for agent in supported_agents():
            proposal = plan_launch(agent, 'fix the build', which=found)
            for flag in AUTO_APPROVE_FLAGS:
                self.assertNotIn(flag, proposal.argv, f'{agent} carries {flag}')

    def test_argument_forms(self) -> None:
        cases = {
            'claude': ('claude', '--', 'p'),
            'codex': ('codex', '--', 'p'),
            'opencode': ('opencode', '--prompt', 'p'),
            'gemini': ('gemini', '--prompt-interactive', 'p'),
            'copilot': ('copilot', '--interactive', 'p'),
            'cursor-agent': ('cursor-agent', 'agent', '--', 'p'),
            'hermes': ('hermes', 'chat', '--tui', '--query=p'),
            'pi': ('pi', 'p'),
        }
        for agent, argv in cases.items():
            self.assertEqual(plan_launch(agent, 'p', which=found).argv, argv, agent)

    def test_a_prompt_starting_with_a_dash_cannot_become_an_option(self) -> None:
        for agent in ('claude', 'codex', 'grok', 'muse', 'omp', 'cursor-agent'):
            argv = plan_launch(agent, '--dangerously-skip-permissions', which=found).argv
            self.assertEqual(argv[-2], '--', agent)
            self.assertEqual(argv[-1], '--dangerously-skip-permissions')

    def test_no_prompt_means_a_bare_interactive_launch(self) -> None:
        self.assertEqual(plan_launch('claude', which=found).argv, ('claude',))

    def test_unsupported_agents_are_refused_with_a_reason(self) -> None:
        for agent, reason in UNSUPPORTED.items():
            with self.assertRaises(LaunchError) as raised:
                plan_launch(agent, 'x')
            self.assertIn(reason, str(raised.exception))

    def test_unknown_agent_is_refused(self) -> None:
        with self.assertRaises(LaunchError):
            plan_launch('rm', 'x')

    def test_prompt_limits(self) -> None:
        with self.assertRaises(LaunchError):
            plan_launch('claude', 'x' * 9000)
        with self.assertRaises(LaunchError):
            plan_launch('claude', 'a\x00b')

    def test_shell_metacharacters_stay_a_single_argument(self) -> None:
        prompt = '$(touch /tmp/pwned); `id` | rm -rf ~ && echo "x"'
        argv = plan_launch('claude', prompt, which=found).argv
        self.assertEqual(argv, ('claude', '--', prompt))

    def test_pi_is_flagged_as_unattended(self) -> None:
        proposal = plan_launch('pi', 'x', which=found)
        self.assertTrue(any('no approval prompts' in w for w in proposal.warnings))
        self.assertNotIn('the agent will ask', proposal.as_text())

    def test_others_say_the_agent_will_ask(self) -> None:
        text = plan_launch('claude', 'x', which=found).as_text()
        self.assertIn('the agent will ask before acting', text)
        self.assertIn('claude -- <prompt>', text)

    def test_missing_binary_is_reported(self) -> None:
        proposal = plan_launch('claude', 'x', which=lambda name: None)
        self.assertIsNone(proposal.binary_path)
        self.assertIn('not found on PATH', proposal.as_text())

    def test_proposal_never_contains_the_prompt_text(self) -> None:
        text = plan_launch('claude', 'my secret token abc123', which=found).as_text()
        self.assertNotIn('abc123', text)


class LaunchTests(unittest.TestCase):
    def setUp(self) -> None:
        self._tmp = tempfile.TemporaryDirectory()
        self.addCleanup(self._tmp.cleanup)
        self.log = Path(self._tmp.name) / 'log' / 'agent-launch.jsonl'
        self.calls: list[list[str]] = []

    def fake_run(self, argv, **kwargs):
        self.calls.append(list(argv))
        return subprocess.CompletedProcess(argv, 7)

    def entries(self) -> list[dict]:
        return [json.loads(line) for line in self.log.read_text(encoding='utf-8').splitlines()]

    def test_unconfirmed_launch_runs_nothing_and_is_audited(self) -> None:
        proposal = plan_launch('claude', 'go', which=found)
        with self.assertRaises(LaunchError):
            launch(proposal, 'go', confirmed=False, audit_path=self.log, run=self.fake_run)
        self.assertEqual(self.calls, [])
        self.assertEqual(self.entries()[0]['outcome'], 'refused')

    def test_confirmed_launch_runs_the_proposed_argv_and_returns_its_status(self) -> None:
        proposal = plan_launch('claude', 'go', which=found)
        status = launch(proposal, 'go', confirmed=True, audit_path=self.log, run=self.fake_run)
        self.assertEqual(status, 7)
        self.assertEqual(self.calls, [['claude', '--', 'go']])
        self.assertEqual(self.entries()[0]['outcome'], 'launched')

    def test_prompt_swapped_after_the_proposal_is_refused(self) -> None:
        proposal = plan_launch('claude', 'harmless', which=found)
        with self.assertRaises(LaunchError):
            launch(proposal, 'something else', confirmed=True, audit_path=self.log, run=self.fake_run)
        self.assertEqual(self.calls, [])
        self.assertIn('differs', self.entries()[0]['detail'])

    def test_missing_binary_is_refused_even_when_confirmed(self) -> None:
        proposal = plan_launch('claude', 'go', which=lambda name: None)
        with self.assertRaises(LaunchError):
            launch(proposal, 'go', confirmed=True, audit_path=self.log, run=self.fake_run)
        self.assertEqual(self.calls, [])

    def test_audit_stores_a_hash_and_a_short_preview_not_the_prompt(self) -> None:
        prompt = 'line one\n' + 'x' * 500 + 'SECRET-TAIL'
        proposal = plan_launch('claude', prompt, which=found)
        launch(proposal, prompt, confirmed=True, audit_path=self.log, run=self.fake_run)
        entry = self.entries()[0]
        self.assertEqual(entry['promptLength'], len(prompt))
        self.assertEqual(len(entry['promptSha256']), 64)
        self.assertLessEqual(len(entry['promptPreview']), 60)
        self.assertNotIn('\n', entry['promptPreview'])
        self.assertNotIn('SECRET-TAIL', self.log.read_text(encoding='utf-8'))

    def test_audit_log_appends(self) -> None:
        proposal = plan_launch('claude', 'go', which=found)
        for _ in range(3):
            launch(proposal, 'go', confirmed=True, audit_path=self.log, run=self.fake_run)
        self.assertEqual(len(self.entries()), 3)

    def test_audit_path_honours_xdg_state_home(self) -> None:
        self.assertEqual(
            audit_log_path({'XDG_STATE_HOME': '/s'}),
            Path('/s') / 'sarina' / 'agent-launch.jsonl',
        )


if __name__ == '__main__':
    unittest.main()
