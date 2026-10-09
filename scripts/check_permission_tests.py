#!/usr/bin/env python3
"""Require root skips around literal permission-denial modes in Rust sources.

This is a textual check for rustfmt-shaped functions, not a Rust parser.
Computed modes and control-flow correctness still need review.
"""

import argparse
import pathlib
import re

# Mask comments and literals, preserving offsets and newlines for diagnostics.
# Raw strings must precede ordinary strings; chars exclude Rust lifetimes.
_NON_CODE = re.compile(
    r'//[^\n]*|/\*|(?:br|r)(?P<hashes>\#*)".*?"(?P=hashes)'
    r'|b?"(?:\\.|[^"\\])*"|b?\'(?:\\.|[^\'\\])\'',
    re.DOTALL,
)
_MODE = re.compile(r'\bfrom_mode\s*\(\s*0o([0-7](?:_?[0-7]){2})\s*\)')
_FUNCTION = re.compile(
    r'^(?P<indent>[ \t]*)(?:pub(?:\([^\n)]*\))?\s+)?'
    r'(?:async\s+)?fn\s+(?P<name>\w+)\b', re.MULTILINE
)
_SKIP = re.compile(r'\bskip_as_root\s*!\s*\(\s*\)')


def mask_non_code(text):
    """Mask literals and comments, including nested Rust block comments."""
    parts = []
    position = 0
    while match := _NON_CODE.search(text, position):
        end = match.end()
        if match.group() == '/*':
            depth = 1
            end = len(text)
            for delimiter in re.finditer(r'/\*|\*/', text[match.end():]):
                depth += 1 if delimiter.group() == '/*' else -1
                if depth == 0:
                    end = match.end() + delimiter.end()
                    break
        parts.append(text[position:match.start()])
        parts.append(re.sub(r'[^\n]', ' ', text[match.start():end]))
        position = end
    parts.append(text[position:])
    return ''.join(parts)


def check(text):
    """Return (line, function) for each restrictive call without a root skip."""
    code = mask_non_code(text)
    modes = [mode for mode in _MODE.finditer(code)
             if not int(mode[1].replace('_', ''), 8) & 0o200]
    if not modes:
        return []
    functions = []
    for match in _FUNCTION.finditer(code):
        # Rustfmt puts the function's final brace at its declaration's indent.
        end = re.search(r'^' + match['indent'] + r'\}', code[match.end():], re.MULTILINE)
        if end:
            functions.append((match.start(), match.end() + end.end(), match['name']))
    problems = []
    for mode in modes:
        enclosing = [fn for fn in functions if fn[0] <= mode.start() < fn[1]]
        function = max(enclosing, default=None, key=lambda fn: fn[0])
        if function and _SKIP.search(code[function[0]:function[1]]):
            continue
        problems.append((code.count('\n', 0, mode.start()) + 1,
                         function[2] if function else '<outside function>'))
    return problems


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--root', type=pathlib.Path,
                        default=pathlib.Path(__file__).resolve().parent.parent)
    root = parser.parse_args().root
    failures = 0
    for directory in ('src', 'tests'):
        for path in sorted((root / directory).rglob('*.rs')):
            for line, function in check(path.read_text(encoding='utf-8')):
                print(f'{path.relative_to(root)}:{line}: {function}: '
                      'permission-denial mode needs skip_as_root!() in this function; '
                      'construct helper permissions in the guarded test')
                failures += 1
    if not failures:
        print('Permission-denial root-skip check passed.')
    return bool(failures)


if __name__ == '__main__':
    raise SystemExit(main())
