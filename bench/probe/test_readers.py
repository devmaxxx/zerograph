import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

import judge

HERE = Path(__file__).resolve().parent
READERS = HERE / "readers.sh"
# The real one, not a stub: which weights a row answered under is the suite's own declaration, and
# a stub here would only pin this file's idea of it.
EMBEDDER = HERE / "embedder.sh"
SMALL = "intfloat/multilingual-e5-small"
CN = "packages/ui/src/lib/cn.ts"
ORIGINAL = "export const cn = (...a) => a.join(' ')\n"
TOUCHED = "// touched for the changes measurement"

QUIET = """#!/bin/bash
echo "quiet: idle=99.0% load1=0.10 ac=1 busy=0 (cargo/rustc/zerograph: 0, node ≥5.0%: 0)"
"""

# The binary and the clock stand still: what this suite reads is readers.sh's own plumbing — what
# it resolves before it changes directory, what it puts back, and what status it hands its caller.
MEASURE = """#!/bin/bash
set -u
NAME=$1; LOG=$2
echo "$NAME  wall=0.10s user=0.05s sys=0.01s maxrss=0.10GB peak_cpu=0% peak_threads=1 samples=0 rc=0" >> "$LOG/summary.txt"
if [ -n "${INTERRUPT_ON_CHANGES:-}" ]; then
  case "$NAME" in changes-*) kill -TERM "$PPID" ;; esac
fi
"""

JUDGE_OK = """import sys
print("ask-fused wall=0.1 maxrss=0.1 peak_cpu=0.0% avg_cpu=60% n=1")
"""

JUDGE_REFUSES = """import sys
sys.stderr.write("ask-fused: a run exited 2 — that row measured a failure, not a reader\\n")
sys.exit(1)
"""


@unittest.skipUnless(os.name == "posix", "bash and a POSIX signal: the macOS kit's own platform")
class Suite(unittest.TestCase):
    """readers.sh over stubs. A real row needs a store, a binary and a quiet machine; none of the
    three is what these two cases are about."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp())
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)
        probe = self.tmp / "probe"
        probe.mkdir()
        shutil.copy(READERS, probe / "readers.sh")
        shutil.copy(EMBEDDER, probe / "embedder.sh")
        for name, text in (("quiet.sh", QUIET), ("measure.sh", MEASURE)):
            (probe / name).write_text(text)
            (probe / name).chmod(0o755)
        self.judge = probe / "judge.py"
        self.judge.write_text(JUDGE_OK)
        self.cn = self.tmp / "wt" / CN
        self.cn.parent.mkdir(parents=True)
        self.cn.write_text(ORIGINAL)
        (self.tmp / "log").mkdir()

    def run_suite(self, worktree="wt", **env):
        return subprocess.run(["bash", "probe/readers.sh", "./bin/zerograph", worktree, "log", "1"],
                              cwd=self.tmp, capture_output=True, text=True,
                              env={**{k: v for k, v in os.environ.items() if k != "REPOGRAPH_EMBED_MODEL"},
                                   "ZEROGRAPH_EMBED_MODEL": SMALL, **env})

    def test_a_medians_step_that_refused_is_not_reported_as_a_suite_that_passed(self):
        self.judge.write_text(JUDGE_REFUSES)
        r = self.run_suite()
        self.assertNotEqual(r.returncode, 0, r.stdout)
        left = (self.tmp / "log" / "medians.txt").read_text() + r.stderr
        self.assertIn("measured a failure", left)

    def test_a_suite_whose_medians_read_hands_back_its_medians_and_a_zero_status(self):
        r = self.run_suite()
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn("ask-fused wall=0.1", (self.tmp / "log" / "medians.txt").read_text())

    # The interruption is a real SIGTERM in the one window that matters — the stub sends it from
    # the `changes` row, between the touch and the restore — and not a sleep racing a real suite.
    def test_a_relative_worktree_still_gets_the_touched_file_put_back(self):
        r = self.run_suite(INTERRUPT_ON_CHANGES="1")
        self.assertNotEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.cn.read_text(), ORIGINAL)

    def test_an_absolute_worktree_gets_it_back_too(self):
        r = self.run_suite(worktree=str(self.tmp / "wt"), INTERRUPT_ON_CHANGES="1")
        self.assertNotEqual(r.returncode, 0, r.stdout)
        self.assertEqual(self.cn.read_text(), ORIGINAL)

    def test_the_touch_the_changes_row_reads_is_gone_by_the_end_of_a_clean_run(self):
        self.run_suite()
        self.assertNotIn(TOUCHED, self.cn.read_text())

    def test_the_file_the_medians_are_taken_from_names_the_embedder(self):
        self.run_suite()
        self.assertIn(f"embedder: ZEROGRAPH_EMBED_MODEL={SMALL}", (self.tmp / "log" / "summary.txt").read_text())

    def test_the_medians_that_become_the_reference_name_it_too_and_still_read_back(self):
        # `medians.txt` is what is copied out as the reference, so it is the file that has to say
        # which weights answered — and it is read back by `judge.py control` and `compare`, which
        # would refuse a suite outright if the line entered one of their tables as a row.
        self.run_suite()
        medians = self.tmp / "log" / "medians.txt"
        self.assertIn(f"embedder: ZEROGRAPH_EMBED_MODEL={SMALL}", medians.read_text())
        self.assertEqual(list(judge.read_medians(medians)), ["ask-fused"])


if __name__ == "__main__":
    unittest.main()
