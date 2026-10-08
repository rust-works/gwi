#!/usr/bin/env python3
"""Documentation link check: relative links, anchors and ADR names (#52).

The Drive ADR links were dead from #19 until #38 and nothing noticed. Over the
files git tracks, this script fails on:

  * a relative link in markdown (`[t](path)`, `[ref]: path`, `<a href>`/`<img src>`)
    whose target is not a tracked file or directory;
  * a `#anchor` into a markdown file (or the same file) that names no heading
    and no `<a id>`/`<a name>`;
  * an `adr-NNNN*.md` name in a `.rs` file or a `.snap` snapshot that is not a
    file in `docs/adrs/`. Rustdoc links are relative to the rendered page, not
    the source file, so only the name is checked.

External links (`http://`, `https://`, `mailto:`, ...) are out of scope.
Known-dead links are listed in `scripts/doc-links-allowlist.txt`; an entry that
no longer matches a dead link is itself an error, so the list cannot rot.

    python3 scripts/check_doc_links.py

Standard library only. Unit tests: scripts/test_check_doc_links.py.
"""

import html
import posixpath
import re
import subprocess
import sys
import unicodedata
from pathlib import Path
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parent.parent
ALLOWLIST = ROOT / "scripts" / "doc-links-allowlist.txt"
ADR_DIR = "docs/adrs"

# A destination with a scheme (`https:`, `mailto:`) or a `//host` is external.
EXTERNAL_RE = re.compile(r"^(?:[A-Za-z][A-Za-z0-9+.-]*:|//)")
# `](dest)` closes any inline link or image, however its text is nested.
# One level of balanced parentheses is allowed in a bare destination: `a_(b).md`.
INLINE_LINK_RE = re.compile(r"\]\(\s*(<[^>]*>|(?:[^()\s]|\([^()\s]*\))*)")
REF_DEF_RE = re.compile(r"^ {0,3}\[[^\]]+\]:\s*(<[^>]*>|\S+)")
HTML_ATTR_RE = re.compile(
    r"""<(?:a|img)\b[^>]*?\s(?:href|src)\s*=\s*["']([^"']*)["']""", re.IGNORECASE
)
HTML_ANCHOR_RE = re.compile(r"""<a\s[^>]*?\b(?:id|name)\s*=\s*["']([^"']+)["']""")
# Any indent: a fence inside a list item is indented past the three spaces CommonMark allows
# a top-level one.
FENCE_RE = re.compile(r"^\s*(`{3,}|~{3,})")
ATX_RE = re.compile(r"^ {0,3}#{1,6}[ \t]+(.*?)(?:[ \t]+#+)?[ \t]*$")
SETEXT_RE = re.compile(r"^ {0,3}(?:=+|-+)[ \t]*$")
# A line that cannot be the text of a setext heading: a heading, quote, table row, list item or rule.
NOT_PARAGRAPH_RE = re.compile(r"^\s*(?:#|>|\||[-*+]\s|\d+[.)]\s|(?:=+|-+)\s*$)")
CODE_SPAN_RE = re.compile(r"(`+)(.+?)\1")
ADR_NAME_RE = re.compile(r"adr-\d{4}[A-Za-z0-9_-]*\.md")


def prose_lines(text):
    """Yield `(lineno, line)` for the lines outside fenced code blocks."""
    fence = None
    for lineno, line in enumerate(text.splitlines(), 1):
        m = FENCE_RE.match(line)
        if fence is None:
            if m:
                fence = m.group(1)
                continue
            yield lineno, line
        elif m and m.group(1)[0] == fence[0] and len(m.group(1)) >= len(fence):
            fence = None


def blank_code_spans(line):
    """Replace inline code spans with spaces: a link written in code is not a link."""
    return CODE_SPAN_RE.sub(lambda m: " " * len(m.group(0)), line)


def link_destinations(text):
    """Yield `(lineno, destination)` for every link in a markdown document."""
    for lineno, line in prose_lines(text):
        line = blank_code_spans(line)
        dests = [m.group(1) for m in INLINE_LINK_RE.finditer(line)]
        dests += HTML_ATTR_RE.findall(line)
        m = REF_DEF_RE.match(line)
        if m:
            dests.append(m.group(1))
        for dest in dests:
            dest = dest.strip()
            if dest.startswith("<") and dest.endswith(">"):
                dest = dest[1:-1]
            if dest:
                yield lineno, dest


def heading_slug(heading):
    """GitHub's anchor for a heading: its rendered text, lowercased, punctuation
    dropped, spaces turned into hyphens."""
    text = re.sub(r"!?\[([^\]]*)\](?:\([^)]*\)|\[[^\]]*\])", r"\1", heading)  # links, images
    text = html.unescape(text)
    text = re.sub(r"<[^>]*>", "", text)  # inline HTML
    text = re.sub(r"[`*]", "", text)  # code spans and emphasis keep their text
    kept = [
        c
        for c in text.lower()
        if c in "-_ " or c.isalnum() or unicodedata.category(c).startswith("M")
    ]
    return "".join(kept).replace(" ", "-")


def anchors(text):
    """Every anchor a markdown document defines. GitHub numbers repeated heading
    slugs (`faq`, `faq-1`, `faq-2`); explicit `<a id>`/`<a name>` are kept as written."""
    found = set()
    seen = {}
    previous = ""
    for _, line in prose_lines(text):
        found.update(a.lower() for a in HTML_ANCHOR_RE.findall(line))
        m = ATX_RE.match(line)
        heading = m.group(1) if m else None
        if (
            heading is None
            and SETEXT_RE.match(line)
            and previous.strip()
            and not NOT_PARAGRAPH_RE.match(previous)
        ):
            heading = previous.strip()
        previous = line
        if heading is None:
            continue
        slug = heading_slug(heading)
        n = seen.get(slug, 0)
        seen[slug] = n + 1
        found.add(slug if n == 0 else f"{slug}-{n}")
    return found


def link_base(source):
    """The file a link in `source` is relative to. A changelog fragment is pasted
    into CHANGELOG.md at release, so its links are written relative to that."""
    if source.startswith("changelog.d/") and not source.endswith("README.md"):
        return "CHANGELOG.md"
    return source


def resolve(source, path_part):
    """The repo-relative path a link in `source` points at, or None when it leaves
    the repository. A leading `/` is the repository root, as on GitHub."""
    if path_part.startswith("/"):
        target = posixpath.normpath(path_part.lstrip("/"))
    else:
        target = posixpath.normpath(posixpath.join(posixpath.dirname(source), path_part))
    if target == ".." or target.startswith("../"):
        return None
    return "" if target == "." else target


def exists(target, tracked, directories):
    return target in tracked or target in directories


def check_link(source, dest, tracked, directories, anchor_cache, texts):
    """The problem with one link as a message, or None when it is fine."""
    if EXTERNAL_RE.match(dest):
        return None
    path_part, _, fragment = dest.partition("#")
    path_part = unquote(path_part.partition("?")[0])
    if not path_part and link_base(source) != source:
        return None  # `#anchor` in a fragment names a heading of the release section
    if path_part:
        target = resolve(link_base(source), path_part)
        if target is None:
            return "points outside the repository"
        if not exists(target, tracked, directories):
            return "no such file"
    else:
        target = source
    if not fragment or not target.endswith(".md") or target not in tracked:
        return None
    if target not in anchor_cache:
        anchor_cache[target] = anchors(texts[target])
    if unquote(fragment).lower() not in anchor_cache[target]:
        return f"no heading or anchor `#{fragment}` in {target}"
    return None


def adr_name_problems(text, adr_files):
    """Yield `(lineno, name)` for each `adr-NNNN*.md` name in `text` that is not in
    docs/adrs/. A name that is the tail of a URL names an ADR elsewhere; skip it."""
    for lineno, line in enumerate(text.splitlines(), 1):
        for m in ADR_NAME_RE.finditer(line):
            token = re.split(r"[\s(<\[]", line[: m.start()])[-1]
            if "://" in token:
                continue
            if m.group(0) not in adr_files:
                yield lineno, m.group(0)


def check(texts, tracked, allowlist=()):
    """Check the documentation links in `texts` (path -> content).

    Returns `(problems, stale)`: the `(path, line, subject, message)` of every
    dead link not on the allowlist, and the allowlist entries that matched nothing.
    """
    directories = {""}  # the repository root
    for path in tracked:
        parent = posixpath.dirname(path)
        while parent and parent not in directories:
            directories.add(parent)
            parent = posixpath.dirname(parent)
    adr_files = {
        posixpath.basename(p) for p in tracked if posixpath.dirname(p) == ADR_DIR
    }
    anchor_cache = {}
    found = []
    for path in sorted(texts):
        text = texts[path]
        if path.endswith(".md"):
            for lineno, dest in link_destinations(text):
                message = check_link(path, dest, tracked, directories, anchor_cache, texts)
                if message:
                    found.append((path, lineno, dest, message))
        elif path.endswith((".rs", ".snap")):
            for lineno, name in adr_name_problems(text, adr_files):
                found.append((path, lineno, name, f"no such ADR in {ADR_DIR}/"))
    allowed = set(allowlist)
    problems = [p for p in found if (p[0], p[2]) not in allowed]
    matched = {(p[0], p[2]) for p in found}
    stale = sorted(allowed - matched)
    return problems, stale


def read_allowlist(path):
    """Entries are `<file>: <link as written>`; `#` starts a comment line."""
    entries = []
    for raw in path.read_text(encoding="utf-8").splitlines():
        line = raw.strip()
        if not line or line.startswith("#"):
            continue
        source, sep, dest = line.partition(": ")
        if not sep:
            sys.exit(f"{path}: malformed entry (want `<file>: <link>`): {raw}")
        entries.append((source.strip(), dest.strip()))
    return entries


def tracked_files(root):
    out = subprocess.run(
        ["git", "-C", str(root), "ls-files", "-z"],
        check=True,
        capture_output=True,
    ).stdout.decode("utf-8")
    return [p for p in out.split("\0") if p]


def main():
    # Tracked and present: a file deleted in the working tree is a dead link target, and a
    # submodule is listed by git but is not a file.
    tracked = [p for p in tracked_files(ROOT) if (ROOT / p).is_file()]
    texts = {
        path: (ROOT / path).read_text(encoding="utf-8")
        for path in tracked
        if path.endswith((".md", ".rs", ".snap"))
    }
    allowlist = read_allowlist(ALLOWLIST) if ALLOWLIST.exists() else []
    problems, stale = check(texts, set(tracked), allowlist)
    for path, lineno, subject, message in problems:
        print(f"{path}:{lineno}: {subject}: {message}")
    for source, dest in stale:
        print(
            f"{ALLOWLIST.relative_to(ROOT)}: stale entry `{source}: {dest}` "
            "(that link is no longer dead; remove the entry)"
        )
    if problems or stale:
        print(f"\n{len(problems)} dead link(s), {len(stale)} stale allowlist entr(ies)")
        return 1
    print(f"checked {len(texts)} files: no dead documentation links")
    return 0


if __name__ == "__main__":
    sys.exit(main())
