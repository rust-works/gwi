#!/usr/bin/env python3
"""Regression tests for the permission-denial source guard (#113)."""

import pathlib
import subprocess
import sys
import tempfile
import unittest

import check_permission_tests as guard


def function(body, name='denial', declaration='fn', attribute='#[test]'):
    return f'{attribute}\n{declaration} {name}() {{\n{body}\n}}\n'


class PermissionTests(unittest.TestCase):
    def test_denial_modes_require_guard(self):
        for mode in ('000', '100', '400', '444', '500', '555', '0_0_0'):
            with self.subTest(mode=mode):
                self.assertEqual(guard.check(function(f'    from_mode(0o{mode});')),
                                 [(3, 'denial')])

    def test_writable_modes_pass(self):
        for mode in ('200', '300', '600', '644', '700', '755'):
            self.assertEqual(guard.check(function(f'    from_mode(0o{mode});')), [])

    def test_guarded_sync_and_async_tests_pass(self):
        for declaration, attribute in (('fn', '#[test]'), ('async fn', '#[tokio::test]')):
            for prefix in ('', 'crate::test_support::'):
                self.assertEqual(guard.check(function(
                    f'    {prefix}skip_as_root!();\n    from_mode(0o500);',
                    declaration=declaration, attribute=attribute)), [])

    def test_multiline_call_and_nested_blocks(self):
        body = '    if true {\n        Permissions::from_mode(\n            0o500\n        );\n    }'
        self.assertEqual(guard.check(function(body)), [(4, 'denial')])

    def test_sibling_guard_does_not_cover_unguarded_test(self):
        text = function('    skip_as_root!();', name='guarded')
        text += function('    from_mode(0o500);', name='unguarded')
        self.assertEqual(guard.check(text), [(7, 'unguarded')])

    def test_comments_and_strings_do_not_supply_a_guard(self):
        for fake in ('// skip_as_root!();', '/* skip_as_root!(); */',
                     'let s = "skip_as_root!(); }";',
                     'let s = r##"skip_as_root!(); }"##;'):
            self.assertEqual(len(guard.check(function(
                f'    {fake}\n    from_mode(0o500);'))), 1)

    def test_comments_and_literals_do_not_supply_a_mode(self):
        for fake in ('// from_mode(0o500)', '/* from_mode(0o500) */',
                     'let s = "from_mode(0o500)";',
                     'let s = r#"from_mode(0o500)"#;', "let c = '}';"):
            self.assertEqual(guard.check(function(f'    {fake}')), [])

    def test_helper_cannot_borrow_its_callers_guard(self):
        text = function('    from_mode(0o500);', name='helper', attribute='')
        text += function('    skip_as_root!();\n    helper();')
        self.assertEqual(guard.check(text), [(3, 'helper')])

    def test_indented_method_with_lifetime_argument(self):
        text = "impl Fixture {\n    fn mode(&self, name: &'a str) {\n        from_mode(0o500);\n    }\n}\n"
        self.assertEqual(guard.check(text), [(3, 'mode')])

    def test_cli_scans_both_directories_and_exits_nonzero(self):
        script = pathlib.Path(guard.__file__).resolve()
        with tempfile.TemporaryDirectory() as root:
            for directory in ('src', 'tests'):
                path = pathlib.Path(root) / directory / 'nested' / 'denial.rs'
                path.parent.mkdir(parents=True)
                path.write_text(function('    from_mode(0o500);'))
            command = [sys.executable, str(script), '--root', root]
            result = subprocess.run(command, capture_output=True, text=True, check=False)
            self.assertEqual(result.returncode, 1, result.stderr)
            self.assertIn('src/nested/denial.rs:3: denial:', result.stdout)
            self.assertIn('tests/nested/denial.rs:3: denial:', result.stdout)
            for path in pathlib.Path(root).rglob('*.rs'):
                path.write_text(function('    skip_as_root!();\n    from_mode(0o500);'))
            result = subprocess.run(command, capture_output=True, text=True, check=False)
            self.assertEqual(result.returncode, 0, result.stderr)


if __name__ == '__main__':
    unittest.main()
