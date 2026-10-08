#!/usr/bin/env python3
"""Unit tests for scripts/check_doc_links.py (#52). They run on in-memory
documents and need no git or network. Run directly
(`python3 scripts/test_check_doc_links.py`); wired into CI's `Doc links` job.

Imported via `importlib` because the script's name is not on `sys.path`.
"""

import importlib.util
import pathlib
import unittest

_SCRIPT_PATH = pathlib.Path(__file__).parent / "check_doc_links.py"
_spec = importlib.util.spec_from_file_location("check_doc_links", _SCRIPT_PATH)
doc_links = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(doc_links)


def run(texts, extra_tracked=(), allowlist=()):
    """Check `texts`; every key is a tracked file, plus `extra_tracked` (empty files)."""
    tracked = set(texts) | set(extra_tracked)
    return doc_links.check(texts, tracked, allowlist)


def subjects(texts, **kwargs):
    problems, _ = run(texts, **kwargs)
    return [p[2] for p in problems]


class MarkdownLinks(unittest.TestCase):
    def test_live_relative_link_passes(self):
        self.assertEqual(subjects({"docs/a.md": "[b](b.md)", "docs/b.md": ""}), [])

    def test_dead_relative_link_fails(self):
        self.assertEqual(subjects({"docs/a.md": "[b](gone.md)"}), ["gone.md"])

    def test_parent_directory_and_root_relative(self):
        texts = {
            "docs/a.md": "[r](../README.md) [s](/README.md) [x](../nope.md)",
            "README.md": "",
        }
        self.assertEqual(subjects(texts), ["../nope.md"])

    def test_link_out_of_the_repository_fails(self):
        problems, _ = run({"a.md": "[x](../../etc/passwd)"})
        self.assertEqual(problems[0][3], "points outside the repository")

    def test_directory_target_passes(self):
        texts = {"a.md": "[d](docs/adrs/) [e](docs/nowhere/)"}
        self.assertEqual(subjects(texts, extra_tracked=["docs/adrs/adr-0000.md"]), ["docs/nowhere/"])

    def test_non_markdown_target_is_checked_too(self):
        texts = {"a.md": "[ok](src/lib.rs#L3) [dead](src/gone.rs)"}
        self.assertEqual(subjects(texts, extra_tracked=["src/lib.rs"]), ["src/gone.rs"])

    def test_untracked_file_does_not_count(self):
        # Only tracked files resolve, so a local, ignored file cannot mask a dead link.
        self.assertEqual(subjects({"a.md": "[x](ignored.md)"}), ["ignored.md"])

    def test_external_links_are_ignored(self):
        text = "[a](https://x.invalid/gone.md) [b](mailto:a@b.c) [c](//x.invalid/y.md)"
        self.assertEqual(subjects({"a.md": text}), [])

    def test_reference_definition_and_html_attribute(self):
        text = "[ref]: gone.md\n<a href=\"also-gone.md\">x</a> <img src='img.png'>\n"
        self.assertEqual(subjects({"a.md": text}), ["gone.md", "also-gone.md", "img.png"])

    def test_title_angle_brackets_and_encoding(self):
        text = '[a](b.md "title") [c](<my file.md>) [d](my%20file.md)'
        texts = {"a.md": text, "b.md": "", "my file.md": ""}
        self.assertEqual(subjects(texts), [])

    def test_links_in_code_are_not_links(self):
        text = "`[x](gone.md)` and\n```\n[y](gone.md)\n```\n~~~\n[z](gone.md)\n~~~\n"
        self.assertEqual(subjects({"a.md": text}), [])

    def test_code_in_link_text_still_checks_the_link(self):
        self.assertEqual(subjects({"a.md": "[`x`](gone.md)"}), ["gone.md"])

    def test_image_inside_link(self):
        self.assertEqual(subjects({"a.md": "[![alt](gone.png)](gone.md)"}), ["gone.png", "gone.md"])

    def test_repository_root_and_parenthesised_names(self):
        texts = {"docs/a.md": "[a](../) [b](/) [c](.) [d](f_(1).md) [e](<g (2).md>)", "docs/f_(1).md": "", "docs/g (2).md": ""}
        self.assertEqual(subjects(texts), [])

    def test_fence_inside_a_list_item(self):
        text = "1. Run:\n\n    ```bash\n    [x](gone.md)\n    # not-a-heading\n    ```\n\n[y](#not-a-heading)"
        self.assertEqual(subjects({"a.md": text}), ["#not-a-heading"])

    def test_href_only_inside_a_and_img_tags(self):
        text = '<div data-src="x.png"></div> set src="y.png"\n<img alt=\'q\' src="gone.png">'
        self.assertEqual(subjects({"a.md": text}), ["gone.png"])

    def test_line_number_is_reported(self):
        problems, _ = run({"a.md": "one\n\n[x](gone.md)\n"})
        self.assertEqual((problems[0][0], problems[0][1]), ("a.md", 3))

    def test_changelog_fragment_links_are_root_relative(self):
        texts = {"changelog.d/9.added.md": "- [d](docs/d.md) [bad](docs/x.md)", "docs/d.md": ""}
        self.assertEqual(subjects(texts), ["docs/x.md"])

    def test_changelog_readme_links_are_its_own(self):
        texts = {"changelog.d/README.md": "[c](../CHANGELOG.md)", "CHANGELOG.md": ""}
        self.assertEqual(subjects(texts), [])


class Anchors(unittest.TestCase):
    def test_heading_anchor_resolves(self):
        texts = {"a.md": "[x](b.md#some-heading)", "b.md": "# Some Heading\n"}
        self.assertEqual(subjects(texts), [])

    def test_dead_anchor_fails(self):
        texts = {"a.md": "[x](b.md#nope)", "b.md": "# Some Heading\n"}
        self.assertEqual(subjects(texts), ["b.md#nope"])

    def test_same_file_anchor(self):
        texts = {"a.md": "# Top\n[ok](#top) [bad](#bottom)\n"}
        self.assertEqual(subjects(texts), ["#bottom"])

    def test_anchor_case_is_ignored_as_on_github(self):
        self.assertEqual(subjects({"a.md": "# Top\n[x](#TOP)"}), [])

    def test_slug_rules(self):
        slug = doc_links.heading_slug
        self.assertEqual(slug("`invalid_grant`"), "invalid_grant")
        self.assertEqual(slug("Coming from omni-dev"), "coming-from-omni-dev")
        self.assertEqual(slug("Q&A: what's new?"), "qa-whats-new")
        self.assertEqual(slug("A [link](x.md) and *emphasis*"), "a-link-and-emphasis")
        self.assertEqual(slug("`commit-guidelines.md`"), "commit-guidelinesmd")
        self.assertEqual(slug("Émigré 2"), "émigré-2")
        self.assertEqual(slug("Two  spaces"), "two--spaces")

    def test_rule_after_a_heading_or_list_is_not_a_setext_heading(self):
        text = "# Foo\n---\n\n- item\n---\n\n| a |\n---\n[x](#-foo) [y](#item) [z](#a)"
        self.assertEqual(subjects({"a.md": text}), ["#-foo", "#item", "#a"])

    def test_slug_of_reference_links_and_entities(self):
        self.assertEqual(doc_links.heading_slug("See [the guide][g]"), "see-the-guide")
        self.assertEqual(doc_links.heading_slug("A &amp; B"), "a--b")

    def test_repeated_headings_are_numbered(self):
        texts = {"a.md": "# FAQ\n# FAQ\n# FAQ\n[a](#faq) [b](#faq-1) [c](#faq-2) [d](#faq-3)"}
        self.assertEqual(subjects(texts), ["#faq-3"])

    def test_closing_hashes_and_setext_headings(self):
        texts = {"a.md": "## Atx ##\n\nSetext\n======\n\nSub\n---\n[a](#atx) [b](#setext) [c](#sub)"}
        self.assertEqual(subjects(texts), [])

    def test_headings_in_code_fences_are_not_anchors(self):
        texts = {"a.md": "```\n# Fake\n```\n[x](#fake)"}
        self.assertEqual(subjects(texts), ["#fake"])

    def test_explicit_html_anchor(self):
        texts = {"a.md": '<a id="custom"></a>\n<a name="Other"></a>\n[a](#custom) [b](#other)'}
        self.assertEqual(subjects(texts), [])

    def test_anchor_into_a_non_markdown_file_is_not_checked(self):
        texts = {"a.md": "[x](src/lib.rs#L10)"}
        self.assertEqual(subjects(texts, extra_tracked=["src/lib.rs"]), [])

    def test_anchor_in_a_dead_file_reports_the_file(self):
        problems, _ = run({"a.md": "[x](gone.md#h)"})
        self.assertEqual(problems[0][3], "no such file")


class AdrNames(unittest.TestCase):
    ADRS = ["docs/adrs/adr-0066.md", "docs/adrs/adr-0077-sheets-deletion.md"]

    def test_live_names_pass(self):
        texts = {
            "src/a.rs": "/// [ADR-0066](../../docs/adrs/adr-0066.md) and adr-0077-sheets-deletion.md\n",
            "tests/snapshots/x.snap": "see docs/adrs/adr-0066.md\n",
        }
        self.assertEqual(subjects(texts, extra_tracked=self.ADRS), [])

    def test_dead_name_in_rust_and_snapshot_fails(self):
        texts = {
            "src/a.rs": "//! [ADR-0069](../docs/adrs/adr-0069.md)\n",
            "tests/snapshots/x.snap": "docs/adrs/adr-0099-gone.md\n",
        }
        self.assertEqual(
            subjects(texts, extra_tracked=self.ADRS), ["adr-0069.md", "adr-0099-gone.md"]
        )

    def test_only_the_name_is_checked_not_the_path(self):
        # Rustdoc links resolve against the rendered page, so the `../` depth is not ours to check.
        texts = {"src/a.rs": "//! [ADR-0066](../../../../../../docs/adrs/adr-0066.md)\n"}
        self.assertEqual(subjects(texts, extra_tracked=self.ADRS), [])

    def test_url_to_another_repository_is_skipped(self):
        texts = {"src/a.rs": "//! <https://github.com/rust-works/omni-dev/blob/main/docs/adrs/adr-0089.md>\n"}
        self.assertEqual(subjects(texts, extra_tracked=self.ADRS), [])

    def test_line_number_is_reported(self):
        problems, _ = run({"src/a.rs": "//\n// adr-0001.md\n"}, extra_tracked=self.ADRS)
        self.assertEqual((problems[0][0], problems[0][1]), ("src/a.rs", 2))

    def test_names_in_other_files_are_not_scanned(self):
        self.assertEqual(subjects({"Cargo.toml": "adr-0001.md"}), [])


class Allowlist(unittest.TestCase):
    def test_allowed_dead_link_is_tolerated(self):
        problems, stale = run({"a.md": "[x](gone.md)"}, allowlist=[("a.md", "gone.md")])
        self.assertEqual((problems, stale), ([], []))

    def test_entry_is_per_file_and_link(self):
        texts = {"a.md": "[x](gone.md)", "b.md": "[x](gone.md)"}
        problems, _ = run(texts, allowlist=[("a.md", "gone.md")])
        self.assertEqual([p[0] for p in problems], ["b.md"])

    def test_entry_for_a_fixed_link_is_stale(self):
        problems, stale = run({"a.md": "[x](b.md)", "b.md": ""}, allowlist=[("a.md", "gone.md")])
        self.assertEqual((problems, stale), ([], [("a.md", "gone.md")]))

    def test_read_allowlist(self):
        import tempfile

        with tempfile.TemporaryDirectory() as d:
            path = pathlib.Path(d) / "allow.txt"
            path.write_text("# comment\n\na.md: ../x y.md#h\n", encoding="utf-8")
            self.assertEqual(doc_links.read_allowlist(path), [("a.md", "../x y.md#h")])
            path.write_text("no separator\n", encoding="utf-8")
            with self.assertRaises(SystemExit):
                doc_links.read_allowlist(path)


if __name__ == "__main__":
    unittest.main()
