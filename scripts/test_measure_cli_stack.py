#!/usr/bin/env python3
"""Regression tests for MSVC frame accounting used in CLI stack reports."""

import unittest

from measure_cli_stack import builder_paths, windows_frames


class FrameTests(unittest.TestCase):
    def test_allocations_saved_registers_and_direct_builder_calls(self):
        frames, calls = windows_frames('''
.seh_proc root_augment_args
 pushq %rbp
 .seh_pushreg %rbp
 subq $4096, %rsp
 .seh_stackalloc 4096
 callq child_augment_subcommands
 callq dependency
 .seh_endproc
.seh_proc child_augment_subcommands
 .seh_stackalloc 512
 callq root_augment_args
 .seh_endproc
.seh_proc dependency
 .seh_stackalloc 9999
 .seh_endproc
''')
        self.assertEqual(frames, {
            "root_augment_args": 4104, "child_augment_subcommands": 512,
            "dependency": 9999,
        })
        self.assertEqual(calls["root_augment_args"], {
            "child_augment_subcommands", "dependency",
        })
        paths = builder_paths(frames, calls)
        self.assertEqual(paths[0]["bytes"], 4616)
        self.assertEqual(len(paths[0]["frames"]), 2)

    def test_no_windows_unwind_frames(self):
        self.assertEqual(windows_frames("sub sp, sp, #4096"), ({}, {}))


if __name__ == "__main__":
    unittest.main()
