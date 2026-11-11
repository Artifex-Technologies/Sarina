from __future__ import annotations

import json
import subprocess
import unittest
from unittest.mock import patch

from blackarch_compat.categories import list_categories, parse_category_list
from blackarch_compat.docker_runner import (
    _build_test_script,
    _extract_conflicts,
    _extract_package_count,
    _normalize_group_name,
    run_category_test,
)
from blackarch_compat.report import CategoryTestResult, CompatibilityReport


def _completed(returncode=0, stdout='', stderr=''):
    return subprocess.CompletedProcess(args=[], returncode=returncode, stdout=stdout, stderr=stderr)


class NormalizeGroupNameTests(unittest.TestCase):
    def test_adds_prefix_when_missing(self) -> None:
        self.assertEqual(_normalize_group_name('webapp'), 'blackarch-webapp')

    def test_leaves_prefix_when_present(self) -> None:
        self.assertEqual(_normalize_group_name('blackarch-webapp'), 'blackarch-webapp')


class BuildTestScriptTests(unittest.TestCase):
    def test_includes_strap_and_install_and_verify_steps(self) -> None:
        script = _build_test_script(('webapp', 'scanner'))
        self.assertIn('strap.sh', script)
        self.assertIn('pacman -S --noconfirm blackarch-webapp blackarch-scanner', script)
        self.assertIn('pacman -Qk', script)
        self.assertIn('___PACKAGE_COUNT___', script)


class ExtractHelpersTests(unittest.TestCase):
    def test_extract_package_count_found(self) -> None:
        stdout = 'some output\n___PACKAGE_COUNT___842\nmore output\n'
        self.assertEqual(_extract_package_count(stdout), 842)

    def test_extract_package_count_missing(self) -> None:
        self.assertEqual(_extract_package_count('no marker here'), 0)

    def test_extract_package_count_malformed(self) -> None:
        self.assertEqual(_extract_package_count('___PACKAGE_COUNT___not-a-number'), 0)

    def test_extract_conflicts_detects_known_markers(self) -> None:
        output = 'installing foo\nerror: failed to commit transaction (conflicting dependencies)\ndone'
        conflicts = _extract_conflicts(output)
        self.assertEqual(len(conflicts), 1)
        self.assertIn('conflicting dependencies', conflicts[0])

    def test_extract_conflicts_empty_when_clean(self) -> None:
        self.assertEqual(_extract_conflicts('everything installed fine'), ())


class RunCategoryTestTests(unittest.TestCase):
    def test_no_categories_fails_without_calling_subprocess(self) -> None:
        with patch('blackarch_compat.docker_runner.subprocess.run') as run_mock:
            result = run_category_test(())
        run_mock.assert_not_called()
        self.assertFalse(result.ok)
        self.assertIn('no categories', result.error)

    def test_docker_unavailable_fails_without_calling_subprocess(self) -> None:
        with patch('blackarch_compat.docker_runner.docker_available', return_value=False), patch(
            'blackarch_compat.docker_runner.subprocess.run'
        ) as run_mock:
            result = run_category_test(('webapp',))
        run_mock.assert_not_called()
        self.assertFalse(result.ok)
        self.assertIn('not found', result.error)

    def test_successful_install_command_shape(self) -> None:
        stdout = '___PACKAGE_COUNT___10\n'
        with patch('blackarch_compat.docker_runner.docker_available', return_value=True), patch(
            'blackarch_compat.docker_runner.subprocess.run',
            return_value=_completed(returncode=0, stdout=stdout),
        ) as run_mock:
            result = run_category_test(('webapp',), image='archlinux:latest')

        command = run_mock.call_args.args[0]
        self.assertEqual(command[0], 'docker')
        self.assertIn('run', command)
        self.assertIn('--rm', command)
        self.assertIn('archlinux:latest', command)
        self.assertTrue(result.ok)
        self.assertEqual(result.package_count, 10)
        self.assertEqual(result.exit_code, 0)

    def test_nonzero_exit_marks_failure(self) -> None:
        with patch('blackarch_compat.docker_runner.docker_available', return_value=True), patch(
            'blackarch_compat.docker_runner.subprocess.run',
            return_value=_completed(returncode=1, stderr='boom'),
        ):
            result = run_category_test(('webapp',))
        self.assertFalse(result.ok)
        self.assertEqual(result.exit_code, 1)
        self.assertIn('boom', result.stderr_tail)

    def test_conflict_output_marks_failure_even_with_zero_exit(self) -> None:
        stdout = 'error: unable to satisfy dependency foo\n'
        with patch('blackarch_compat.docker_runner.docker_available', return_value=True), patch(
            'blackarch_compat.docker_runner.subprocess.run',
            return_value=_completed(returncode=0, stdout=stdout),
        ):
            result = run_category_test(('webapp',))
        self.assertFalse(result.ok)
        self.assertEqual(len(result.conflicts), 1)

    def test_timeout_does_not_raise(self) -> None:
        with patch('blackarch_compat.docker_runner.docker_available', return_value=True), patch(
            'blackarch_compat.docker_runner.subprocess.run',
            side_effect=subprocess.TimeoutExpired(cmd='docker', timeout=5),
        ):
            result = run_category_test(('webapp',), timeout_seconds=5)
        self.assertFalse(result.ok)
        self.assertIsNotNone(result.error)

    def test_os_error_does_not_raise(self) -> None:
        with patch('blackarch_compat.docker_runner.docker_available', return_value=True), patch(
            'blackarch_compat.docker_runner.subprocess.run',
            side_effect=OSError('no docker'),
        ):
            result = run_category_test(('webapp',))
        self.assertFalse(result.ok)
        self.assertIsNotNone(result.error)


class ParseCategoryListTests(unittest.TestCase):
    def test_filters_dedupes_and_sorts(self) -> None:
        output = 'blackarch-webapp\nblackarch-scanner\nblackarch-webapp\nblackarch\nunrelated-group\n'
        self.assertEqual(
            parse_category_list(output),
            ['blackarch-scanner', 'blackarch-webapp'],
        )

    def test_empty_input(self) -> None:
        self.assertEqual(parse_category_list(''), [])


class ListCategoriesTests(unittest.TestCase):
    def test_docker_unavailable_returns_empty(self) -> None:
        with patch('blackarch_compat.categories.docker_available', return_value=False), patch(
            'blackarch_compat.categories.subprocess.run'
        ) as run_mock:
            result = list_categories()
        run_mock.assert_not_called()
        self.assertEqual(result, [])

    def test_successful_discovery(self) -> None:
        stdout = 'blackarch-webapp\nblackarch-scanner\n'
        with patch('blackarch_compat.categories.docker_available', return_value=True), patch(
            'blackarch_compat.categories.subprocess.run',
            return_value=_completed(returncode=0, stdout=stdout),
        ):
            result = list_categories()
        self.assertEqual(result, ['blackarch-scanner', 'blackarch-webapp'])

    def test_nonzero_exit_returns_empty(self) -> None:
        with patch('blackarch_compat.categories.docker_available', return_value=True), patch(
            'blackarch_compat.categories.subprocess.run',
            return_value=_completed(returncode=1),
        ):
            self.assertEqual(list_categories(), [])

    def test_timeout_returns_empty(self) -> None:
        with patch('blackarch_compat.categories.docker_available', return_value=True), patch(
            'blackarch_compat.categories.subprocess.run',
            side_effect=subprocess.TimeoutExpired(cmd='docker', timeout=5),
        ):
            self.assertEqual(list_categories(), [])


class ReportTests(unittest.TestCase):
    def test_to_json_summarizes_pass_fail(self) -> None:
        report = CompatibilityReport(
            image='archlinux:latest',
            results=[
                CategoryTestResult(categories=('webapp',), ok=True, package_count=5),
                CategoryTestResult(categories=('scanner',), ok=False, error='boom'),
            ],
        )
        payload = json.loads(report.to_json())
        self.assertEqual(payload['total'], 2)
        self.assertEqual(payload['passed'], 1)
        self.assertEqual(payload['failed'], 1)
        self.assertEqual(payload['results'][0]['categories'], ['webapp'])

    def test_to_markdown_includes_failures_section_only_when_failing(self) -> None:
        passing_only = CompatibilityReport(
            image='archlinux:latest',
            results=[CategoryTestResult(categories=('webapp',), ok=True)],
        )
        self.assertNotIn('## Failures', passing_only.to_markdown())

        with_failure = CompatibilityReport(
            image='archlinux:latest',
            results=[
                CategoryTestResult(categories=('webapp',), ok=True),
                CategoryTestResult(categories=('scanner',), ok=False, error='boom', exit_code=1),
            ],
        )
        markdown = with_failure.to_markdown()
        self.assertIn('## Failures', markdown)
        self.assertIn('boom', markdown)
        self.assertIn('PASS', markdown)
        self.assertIn('FAIL', markdown)


if __name__ == '__main__':
    unittest.main()
