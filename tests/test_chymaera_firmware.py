import subprocess
import tempfile
import unittest
from pathlib import Path

from sarina.chymaera import check_device_info, merge_impact, parse_device_info
from sarina.chymaera.firmware import REBASE_POLICY_BUDGET, classify_path

HEALTHY_DEVICE_INFO = """
Chymaera Zero
firmware_origin_fork : Chymaera
firmware_origin_git  : https://github.com/Artifex-Technologies/Argus_Firmware.git
firmware_version     : 1.4.3-CHY-0.1.0
firmware_commit      : abc1234
hardware_model       : Flipper Zero
enclave_valid        : true
radio_alive          : true
"""


class ParseDeviceInfoTest(unittest.TestCase):
    def test_parses_key_value_lines(self):
        fields = parse_device_info(HEALTHY_DEVICE_INFO)
        self.assertEqual(fields['firmware_origin_fork'], 'Chymaera')
        self.assertEqual(fields['firmware_commit'], 'abc1234')
        self.assertEqual(fields['radio_alive'], 'true')

    def test_skips_banner_lines_without_separator(self):
        self.assertNotIn('Chymaera Zero', parse_device_info(HEALTHY_DEVICE_INFO))

    def test_keeps_colons_inside_values(self):
        fields = parse_device_info(HEALTHY_DEVICE_INFO)
        self.assertEqual(
            fields['firmware_origin_git'],
            'https://github.com/Artifex-Technologies/Argus_Firmware.git',
        )


class CheckDeviceInfoTest(unittest.TestCase):
    def test_healthy_device_has_no_warnings(self):
        report = check_device_info(parse_device_info(HEALTHY_DEVICE_INFO))
        self.assertTrue(report.ok, report.warnings)

    def test_stock_firmware_is_flagged(self):
        fields = parse_device_info(HEALTHY_DEVICE_INFO)
        fields['firmware_origin_fork'] = 'flipperdevices'
        report = check_device_info(fields)
        self.assertFalse(report.ok)
        self.assertTrue(any('firmware_origin_fork' in w for w in report.warnings))

    def test_broken_enclave_is_flagged(self):
        fields = parse_device_info(HEALTHY_DEVICE_INFO)
        fields['enclave_valid'] = 'false'
        self.assertTrue(any('enclave_valid' in w for w in check_device_info(fields).warnings))

    def test_unknown_version_is_a_warning(self):
        fields = parse_device_info(HEALTHY_DEVICE_INFO)
        fields['firmware_version'] = 'unknown'
        report = check_device_info(fields)
        self.assertFalse(report.ok)
        self.assertTrue(any('firmware_version' in w for w in report.warnings))

    def test_tagged_release_reports_tracked_upstream(self):
        fields = parse_device_info(HEALTHY_DEVICE_INFO)
        fields['firmware_version'] = '1.4.3-CHY-0.1.0'
        report = check_device_info(fields)
        self.assertTrue(report.ok, report.warnings)
        self.assertIn('tracks upstream release 1.4.3', report.notes)
        self.assertFalse(any('development build' in n for n in report.notes))

    def test_development_build_is_a_note_not_a_warning(self):
        # "+8" counts commits since the last chy-* tag. Normal during
        # development, so it must not fail the check.
        fields = parse_device_info(HEALTHY_DEVICE_INFO)
        fields['firmware_version'] = '1.4.3-CHY-0.1.0+8'
        report = check_device_info(fields)
        self.assertTrue(report.ok, report.warnings)
        self.assertTrue(any('8 commit(s) since chy-0.1.0' in n for n in report.notes))
        self.assertIn('## Notes', report.as_text())

    def test_untagged_fork_is_noted(self):
        fields = parse_device_info(HEALTHY_DEVICE_INFO)
        fields['firmware_version'] = '1.4.3-CHY-dev'
        report = check_device_info(fields)
        self.assertTrue(report.ok, report.warnings)
        self.assertTrue(any('no chy-* tag' in n for n in report.notes))

    def test_legacy_version_format_is_noted(self):
        fields = parse_device_info(HEALTHY_DEVICE_INFO)
        fields['firmware_version'] = 'chy-0.1.0-2-g540d03c7'
        report = check_device_info(fields)
        self.assertTrue(report.ok, report.warnings)
        self.assertTrue(any('pre-2026-09-12' in n for n in report.notes))

    def test_unrecognised_version_is_noted(self):
        fields = parse_device_info(HEALTHY_DEVICE_INFO)
        fields['firmware_version'] = 'something-else'
        report = check_device_info(fields)
        self.assertTrue(report.ok, report.warnings)
        self.assertTrue(any('not a recognised' in n for n in report.notes))

    def test_commit_mismatch_is_flagged(self):
        fields = parse_device_info(HEALTHY_DEVICE_INFO)
        report = check_device_info(fields, expected_commit='deadbee')
        self.assertTrue(any('firmware_commit' in w for w in report.warnings))

    def test_abbreviated_commit_matches_full_commit(self):
        fields = parse_device_info(HEALTHY_DEVICE_INFO)
        report = check_device_info(fields, expected_commit='abc1234567890')
        self.assertTrue(report.ok, report.warnings)

    def test_missing_field_is_flagged(self):
        report = check_device_info({})
        self.assertTrue(any('missing' in w for w in report.warnings))


class ClassifyPathTest(unittest.TestCase):
    def test_protobuf_is_wire_format(self):
        self.assertEqual(classify_path('assets/protobuf/application.proto')[0], 'wire-format')

    def test_external_app_is_low_risk(self):
        self.assertEqual(classify_path('applications_user/chymaera/main.c')[0], 'external-app')

    def test_core_paths(self):
        for path in ('furi/core/thread.c', 'targets/f7/api_symbols.csv', 'lib/nfc/nfc.c'):
            self.assertEqual(classify_path(path)[0], 'core', path)

    def test_applications_user_wins_over_applications(self):
        self.assertEqual(classify_path('applications_user/x.c')[0], 'external-app')
        self.assertEqual(classify_path('applications/main/gui.c')[0], 'core')

    def test_markdown_outside_known_prefixes_is_docs(self):
        self.assertEqual(classify_path('ReadMe.md')[0], 'docs')

    def test_unknown_path_is_other(self):
        self.assertEqual(classify_path('.editorconfig')[0], 'other')


def _run(repo, *args):
    subprocess.run(['git', *args], cwd=repo, check=True, capture_output=True)


def _write(repo, relative, text):
    target = Path(repo) / relative
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_text(text, encoding='utf-8')


class MergeImpactTest(unittest.TestCase):
    """Builds a real two-branch git repo so the git plumbing is exercised."""

    def setUp(self):
        self._tmp = tempfile.TemporaryDirectory()
        self.repo = Path(self._tmp.name)
        _run(self.repo, 'init', '-b', 'main')
        _run(self.repo, 'config', 'user.email', 'test@example.com')
        _run(self.repo, 'config', 'user.name', 'Test')

        _write(self.repo, 'furi/core.c', 'base\n')
        _write(self.repo, 'applications_user/app.c', 'base\n')
        _write(self.repo, 'assets/protobuf/application.proto', 'base\n')
        _run(self.repo, 'add', '-A')
        _run(self.repo, 'commit', '-m', 'base')

        # Upstream branch changes core + protobuf.
        _run(self.repo, 'checkout', '-b', 'upstream-dev')
        _write(self.repo, 'furi/core.c', 'upstream\n')
        _write(self.repo, 'assets/protobuf/application.proto', 'upstream\n')
        _run(self.repo, 'add', '-A')
        _run(self.repo, 'commit', '-m', 'upstream work')

        # Our fork: edits an upstream file both sides touch (high), edits an
        # upstream file only we touch (medium), and adds a brand-new file
        # inside an upstream directory (low - this is the menu_wii.c case).
        _run(self.repo, 'checkout', 'main')
        _write(self.repo, 'furi/core.c', 'chymaera\n')
        _write(self.repo, 'applications_user/app.c', 'chymaera\n')
        _write(self.repo, 'applications/services/gui/modules/menu_wii.c', 'new\n')
        _run(self.repo, 'add', '-A')
        _run(self.repo, 'commit', '-m', 'fork work')

    def tearDown(self):
        self._tmp.cleanup()

    def tiers(self, upstream_ref='upstream-dev'):
        report = merge_impact(self.repo, upstream_ref=upstream_ref)
        return {risk.path: risk.tier for risk in report.risks}

    def test_file_changed_on_both_sides_is_high(self):
        self.assertEqual(self.tiers()['furi/core.c'], 'high')

    def test_our_edit_to_an_upstream_file_is_medium(self):
        # Exists upstream, but upstream has not moved it - conflicts only later.
        self.assertEqual(self.tiers()['applications_user/app.c'], 'medium')

    def test_new_file_in_an_upstream_directory_is_low(self):
        # The menu_wii.c case: a path-prefix rule would call this core and
        # high-risk. It cannot conflict, because it does not exist upstream.
        self.assertEqual(self.tiers()['applications/services/gui/modules/menu_wii.c'], 'low')

    def test_area_label_is_still_reported_for_context(self):
        report = merge_impact(self.repo, upstream_ref='upstream-dev')
        areas = {risk.path: risk.area for risk in report.risks}
        self.assertEqual(areas['furi/core.c'], 'core')
        self.assertEqual(areas['applications_user/app.c'], 'external-app')

    def test_upstream_only_change_is_not_reported(self):
        # We never touched the proto; it merges cleanly and is not our problem.
        self.assertNotIn('assets/protobuf/application.proto', self.tiers())

    def test_conflicting_lists_only_high(self):
        report = merge_impact(self.repo, upstream_ref='upstream-dev')
        self.assertEqual([r.path for r in report.conflicting], ['furi/core.c'])

    def test_non_additive_excludes_new_files(self):
        report = merge_impact(self.repo, upstream_ref='upstream-dev')
        paths = [r.path for r in report.non_additive]
        self.assertIn('furi/core.c', paths)
        self.assertIn('applications_user/app.c', paths)
        self.assertNotIn('applications/services/gui/modules/menu_wii.c', paths)

    def test_counts_both_sides(self):
        report = merge_impact(self.repo, upstream_ref='upstream-dev')
        self.assertEqual(len(report.upstream_changed), 2)
        self.assertEqual(len(report.local_changed), 3)

    def test_text_report_surfaces_the_conflict(self):
        text = merge_impact(self.repo, upstream_ref='upstream-dev').as_text()
        self.assertIn('furi/core.c', text)
        self.assertIn('high (1)', text)
        self.assertIn('low (1)', text)

    def test_small_fork_does_not_trip_the_policy_alarm(self):
        report = merge_impact(self.repo, upstream_ref='upstream-dev')
        self.assertFalse(report.policy_breached)
        self.assertNotIn('POLICY ALARM', report.as_text())

    def test_policy_alarm_fires_when_too_many_upstream_files_are_edited(self):
        _run(self.repo, 'checkout', 'main')
        for index in range(REBASE_POLICY_BUDGET + 1):
            _write(self.repo, f'lib/vendor{index}.c', 'base\n')
        _run(self.repo, 'add', '-A')
        _run(self.repo, 'commit', '-m', 'add vendored files')
        _run(self.repo, 'checkout', '-b', 'up2', 'main')
        _run(self.repo, 'checkout', 'main')
        for index in range(REBASE_POLICY_BUDGET + 1):
            _write(self.repo, f'lib/vendor{index}.c', 'ours\n')
        _run(self.repo, 'add', '-A')
        _run(self.repo, 'commit', '-m', 'edit them all')
        report = merge_impact(self.repo, upstream_ref='up2')
        self.assertTrue(report.policy_breached)
        self.assertIn('POLICY ALARM', report.as_text())

    def test_clean_merge_is_reported_as_clean(self):
        _run(self.repo, 'checkout', '-b', 'clean-branch', 'main')
        _write(self.repo, 'documentation/notes.md', 'docs only\n')
        _run(self.repo, 'add', '-A')
        _run(self.repo, 'commit', '-m', 'docs')
        _run(self.repo, 'checkout', 'main')
        report = merge_impact(self.repo, upstream_ref='clean-branch')
        self.assertEqual(report.conflicting, ())
        self.assertIn('should apply cleanly', report.as_text())

    def test_unknown_ref_raises_runtime_error(self):
        with self.assertRaises(RuntimeError):
            merge_impact(self.repo, upstream_ref='does/not/exist')


if __name__ == '__main__':
    unittest.main()
