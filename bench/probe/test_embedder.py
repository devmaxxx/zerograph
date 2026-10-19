import os
import shutil
import subprocess
import tempfile
import unittest
from pathlib import Path

import judge

HERE = Path(__file__).resolve().parent
EMBEDDER = HERE / "embedder.sh"
SMALL = "intfloat/multilingual-e5-small"
OTHER = "BAAI/bge-m3"

# The four scripts that run a writer or a reader against the writable worktree, each with arguments
# that name nothing real — the arguments are bound under `set -u` before the source in three of the
# four, so a call with none of them dies over `$1` rather than over the declaration. The declaration
# is a precondition of the whole kit, so every one of them has to refuse over it before it resolves
# a directory or opens a store — which is also what makes these cases cheap enough to be unit
# tests: none of the four gets far enough to need one.
SCRIPTS = {
    "reset.sh": [],
    "readers.sh": ["bin/zerograph", "wt", "log", "1"],
    "embed.sh": ["control-1", "bin/zerograph", "wt", "log"],
    "arms.sh": ["bin/zerograph", "wt", "log", "tag"],
}


def sourced(snippet, model=None, **env):
    """embedder.sh's calls without a script around them: sourcing it defines them and runs nothing."""
    return run(["bash", "-c", f"source '{EMBEDDER}'; {snippet}"], None, model, **env)


def run(argv, cwd, model, **env):
    """One environment for both callers here. `model` is the whole of the declaration — `None` means
    it is absent, and this machine's own value never leaks in behind it — and whatever else a script
    reads off the environment rides on `env`. The pre-rename variable is absent the same way, unless
    a case passes it."""
    e = {k: v for k, v in os.environ.items() if k not in ("ZEROGRAPH_EMBED_MODEL", "REPOGRAPH_EMBED_MODEL")}
    if model is not None:
        e["ZEROGRAPH_EMBED_MODEL"] = model
    e.update(env)
    return subprocess.run(argv, cwd=cwd, capture_output=True, text=True, env=e)


@unittest.skipUnless(os.name == "posix", "bash: the macOS kit's own platform")
class Declaration(unittest.TestCase):
    """Which weights a row was read under, declared in the environment and never guessed."""

    def setUp(self):
        self.tmp = Path(tempfile.mkdtemp()).resolve()
        self.addCleanup(shutil.rmtree, self.tmp, ignore_errors=True)

    def script(self, name, model, **env):
        # `reset.sh`'s three directories are named away from this machine's real ones on purpose: a
        # check that ran after them would reset the corpus worktree instead of refusing, and this
        # case would notice by the wording rather than by the damage.
        return run(["bash", str(HERE / name), *SCRIPTS[name]], self.tmp, model,
                   FIX=str(self.tmp / "fix"), WT=str(self.tmp / "wt"), BIN=str(self.tmp / "bin"), **env)

    def test_the_declared_model_reaches_the_binary_a_script_runs(self):
        # Declared as a plain shell variable and not in the environment, which is what makes the
        # assertion bite: what a row is read under is what the binary's own process sees, and a
        # declaration that stopped at the script would leave the binary on the built-in default.
        r = sourced(f"ZEROGRAPH_EMBED_MODEL={SMALL}; embedder_declared && "
                    "bash -c 'echo child sees $ZEROGRAPH_EMBED_MODEL and $REPOGRAPH_EMBED_MODEL'", None)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn(f"child sees {SMALL} and {SMALL}", r.stdout, "a pinned pre-rename binary reads the old name")

    def test_an_absent_declaration_refuses_and_says_what_to_export(self):
        r = sourced("embedder_declared", None)
        self.assertEqual(r.returncode, 2, r.stdout)
        self.assertIn("ZEROGRAPH_EMBED_MODEL", r.stderr)
        self.assertIn(SMALL, r.stderr)

    def test_another_model_refuses_and_names_both(self):
        r = sourced("embedder_declared", OTHER)
        self.assertEqual(r.returncode, 2, r.stdout)
        self.assertIn(OTHER, r.stderr)
        self.assertIn(SMALL, r.stderr)

    def test_every_script_that_runs_a_reader_or_a_writer_refuses_without_it(self):
        for name in SCRIPTS:
            with self.subTest(script=name):
                r = self.script(name, None)
                self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
                self.assertIn("ZEROGRAPH_EMBED_MODEL", r.stderr)

    def test_every_script_refuses_a_declaration_naming_another_model(self):
        for name in SCRIPTS:
            with self.subTest(script=name):
                r = self.script(name, OTHER)
                self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
                self.assertIn(OTHER, r.stderr)

    def test_a_declaration_set_to_nothing_is_refused_as_absent(self):
        r = sourced("embedder_declared", "")
        self.assertEqual(r.returncode, 2, r.stdout)
        self.assertIn("ZEROGRAPH_EMBED_MODEL", r.stderr)

    def test_the_pre_rename_variable_naming_another_model_is_refused(self):
        r = sourced("embedder_declared", SMALL, REPOGRAPH_EMBED_MODEL=OTHER)
        self.assertEqual(r.returncode, 2, r.stdout)
        self.assertIn("REPOGRAPH_EMBED_MODEL", r.stderr)
        self.assertIn(OTHER, r.stderr)

    def test_the_pre_rename_variable_agreeing_with_the_declaration_passes(self):
        r = sourced("embedder_declared", SMALL, REPOGRAPH_EMBED_MODEL=SMALL)
        self.assertEqual(r.returncode, 0, r.stderr)

    def test_every_script_refuses_when_the_pre_rename_variable_disagrees(self):
        for name in SCRIPTS:
            with self.subTest(script=name):
                r = self.script(name, SMALL, REPOGRAPH_EMBED_MODEL=OTHER)
                self.assertEqual(r.returncode, 2, r.stdout + r.stderr)
                self.assertIn("REPOGRAPH_EMBED_MODEL", r.stderr)

    def test_the_line_a_transcript_carries_is_not_read_back_as_a_row(self):
        # It sits in `summary.txt` beside the rows `judge.py medians` takes its medians over, the
        # way the quiet line does, so a reading says which embedder it was taken under. A line that
        # parsed as a row would enter one of those medians as a run with no numbers on it.
        r = sourced("embedder_line", SMALL)
        self.assertEqual(r.returncode, 0, r.stderr)
        self.assertIn(SMALL, r.stdout)
        self.assertEqual(judge.medians([r.stdout]), {})


if __name__ == "__main__":
    unittest.main()
