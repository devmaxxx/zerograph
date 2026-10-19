import contextlib
import io
import tempfile
import unittest
from pathlib import Path

import judge

SUMMARY = """\
ask-fused-1  wall=0.61s user=0.40s sys=0.20s maxrss=1.55GB peak_cpu=120% peak_threads=8 samples=1
ask-fused-2  wall=0.75s user=0.41s sys=0.21s maxrss=1.56GB peak_cpu=118% peak_threads=8 samples=1
ask-fused-3  wall=0.60s user=0.40s sys=0.20s maxrss=1.36GB peak_cpu=121% peak_threads=8 samples=1
impact-1  wall=0.04s user=0.02s sys=0.01s maxrss=0.05GB peak_cpu=0% peak_threads=1 samples=0
impact-2  wall=0.05s user=0.02s sys=0.01s maxrss=0.05GB peak_cpu=0% peak_threads=1 samples=0
impact-3  wall=0.04s user=0.02s sys=0.01s maxrss=0.05GB peak_cpu=0% peak_threads=1 samples=0
"""

STAMPED = """\
100.00 start
100.40 dense: model open in 0.4s
100.90 dense: 128/33525 rows, 256.0 rows/s, ~2 min left
103.90 dense: 512/33525 rows, 170.7 rows/s, ~3 min left
109.90 dense: 1536/33525 rows, 170.7 rows/s, ~3 min left
118.20 dense: 33525/33525 rows, 180.0 rows/s, ~0 min left
118.30 dense: embedded 33525 rows in 18.3s
118.31        18.31 real        60.00 user         2.00 sys
118.31           1740000000  maximum resident set size
"""


class Medians(unittest.TestCase):
    def test_runs_of_one_row_are_grouped_and_the_median_taken(self):
        m = judge.medians(SUMMARY.splitlines())
        self.assertEqual(m["ask-fused"], {"wall": 0.61, "maxrss": 1.55, "peak_cpu": 120.0,
                                          "avg_cpu": 0.98, "n": 3})
        self.assertEqual(m["impact"]["wall"], 0.04)

    def test_average_cpu_is_derived_from_the_time_report_and_not_from_the_sampler(self):
        # 0.40 user + 0.20 sys over 0.61 s of wall clock is 0.98 cores busy, on a row whose
        # sampled peak says 120% — the two are different readings and the derived one is the
        # column every reader row has.
        self.assertEqual(judge.avg_cpu(0.61, 0.40, 0.20), 0.98)
        # Three runs read 0.98, 0.83 and 1.0; the median is the middle one, taken like the rest.
        self.assertEqual(judge.medians(SUMMARY.splitlines())["ask-fused"]["avg_cpu"], 0.98)

    def test_a_row_whose_wall_clock_rounded_away_has_no_average_rather_than_a_zero(self):
        # Nothing was measured, so there is no number of cores to report; a row that ran and kept
        # no core busy is the case below, and the two must not arrive at a reader as one value.
        self.assertIsNone(judge.avg_cpu(0.0, 0.0, 0.0))
        line = "impact-1  wall=0.00s user=0.00s sys=0.00s maxrss=0.05GB peak_cpu=0% peak_threads=1 samples=0 rc=0"
        self.assertIsNone(judge.medians([line])["impact"]["avg_cpu"])

    def test_a_row_whose_cpu_fields_both_read_zero_measured_a_zero(self):
        # `/usr/bin/time` reports user and sys to 10 ms, so a 40 ms row that kept a core busy for
        # under one tick reports 0.00 of each. That is a reading of the column, not its absence.
        self.assertEqual(judge.avg_cpu(0.04, 0.0, 0.0), 0.0)
        line = "impact-1  wall=0.04s user=0.00s sys=0.00s maxrss=0.05GB peak_cpu=0% peak_threads=1 samples=0 rc=0"
        self.assertEqual(judge.medians([line])["impact"]["avg_cpu"], 0.0)

    def test_one_run_whose_wall_rounded_away_costs_the_column_that_run_and_not_the_row(self):
        # Four of five runs have a reading; the median is taken over those four and the row keeps
        # the column. Dropping it would report `n/a` for a row four readings of which exist.
        lines = ["impact-1  wall=0.00s user=0.00s sys=0.00s maxrss=0.05GB peak_cpu=0% peak_threads=1 samples=0 rc=0"]
        lines += [f"impact-{i}  wall=0.04s user=0.02s sys=0.01s maxrss=0.05GB peak_cpu=0% "
                  "peak_threads=1 samples=0 rc=0" for i in range(2, 6)]
        m = judge.medians(lines)
        self.assertEqual(m["impact"]["avg_cpu"], 0.75)
        # And `n` still counts every run, because wall and RSS were read on all five.
        self.assertEqual(m["impact"]["n"], 5)

    def test_a_row_no_run_of_which_has_a_reading_has_no_average(self):
        lines = [f"impact-{i}  wall=0.04s maxrss=0.05GB peak_cpu=0% peak_threads=1 samples=0 rc=0"
                 for i in range(1, 4)]
        self.assertIsNone(judge.medians(lines)["impact"]["avg_cpu"])

    def test_an_even_count_takes_the_upper_middle_like_bench_does(self):
        self.assertEqual(judge.median([1.0, 2.0, 3.0, 4.0]), 3.0)

    def test_a_medians_file_reads_back_as_written(self):
        import os
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "m.txt")
            with open(p, "w") as f:
                f.write("ask-fused wall=0.61 maxrss=1.55 peak_cpu=120.0% avg_cpu=98% n=5\n")
            self.assertEqual(judge.read_medians(p)["ask-fused"],
                             {"wall": 0.61, "maxrss": 1.55, "peak_cpu": 120.0, "avg_cpu": 0.98, "n": 5})

    def test_the_two_cpu_columns_are_printed_in_the_same_unit(self):
        # Cores on the row, percent of one core in the file: 0.98 cores is 98%, beside a sampled
        # peak already written as a percentage, and the line reads back as the cores it came from.
        out = io.StringIO()
        with tempfile.TemporaryDirectory() as d:
            summary = Path(d, "summary.txt")
            summary.write_text(SUMMARY)
            with contextlib.redirect_stdout(out):
                judge.main(["judge.py", "medians", str(summary)])
        line = [l for l in out.getvalue().splitlines() if l.startswith("ask-fused")][0]
        self.assertIn("peak_cpu=120.0% avg_cpu=98% ", line)
        with tempfile.TemporaryDirectory() as d:
            medians = Path(d, "medians.txt")
            medians.write_text(out.getvalue())
            self.assertEqual(judge.read_medians(str(medians))["ask-fused"]["avg_cpu"], 0.98)

    def test_the_average_is_read_in_the_one_unit_it_is_written_in(self):
        # Percent of one core is what `medians` prints and the only spelling that reads. A bare
        # number is not a second unit to be guessed at: read as percent it is a hundredfold out,
        # read as cores it makes the file's own column ambiguous — so it is a drifted row.
        with tempfile.TemporaryDirectory() as d:
            pct, bare = Path(d, "pct.txt"), Path(d, "bare.txt")
            pct.write_text("ask-fused wall=0.61 maxrss=1.55 peak_cpu=120.0% avg_cpu=98% n=5\n")
            bare.write_text("ask-fused wall=0.61 maxrss=1.55 peak_cpu=120.0% avg_cpu=98 n=5\n")
            self.assertEqual(judge.read_medians(str(pct))["ask-fused"]["avg_cpu"], 0.98)
            with self.assertRaises(SystemExit) as e:
                judge.read_medians(str(bare))
            self.assertIn("avg_cpu=98 n=5", str(e.exception))

    def test_a_row_whose_spelling_drifted_is_refused_and_not_dropped_from_the_file(self):
        # Skipping it read three rows as two and judged the shorter suite green — and both sides of
        # a control drift together, so nothing downstream notices the row is gone.
        with tempfile.TemporaryDirectory() as d:
            p = Path(d, "medians.txt")
            p.write_text("embedder: ZEROGRAPH_EMBED_MODEL=small\n"
                         "ask-fused wall=0.61 maxrss=1.55 peak_cpu=120.0% avg_cpu=98% n=5\n"
                         "impact wall=0.04 maxrss=0.05 peak_cpu=0.0% cpu=75% n=5\n"
                         "dump10 wall=0.70 maxrss=1.36 peak_cpu=100.0% avg_cpu=90% n=5\n")
            with self.assertRaises(SystemExit) as e:
                judge.read_medians(str(p))
            self.assertIn(str(p), str(e.exception))
            self.assertIn("impact wall=0.04", str(e.exception))

    def test_the_prose_a_medians_file_travels_with_is_not_read_as_a_drifted_row(self):
        # `readers.sh` heads the file with the embedder line and `medians` prints its notes under
        # the rows; none of them is a reading, and the file is the one copied out of every run.
        with tempfile.TemporaryDirectory() as d:
            p = Path(d, "medians.txt")
            p.write_text("embedder: ZEROGRAPH_EMBED_MODEL=BAAI/bge-m3\n"
                         "quiet: idle=96% load1=1.2 avail=12.0GB\n"
                         "trace wall=0.42 maxrss=0.31 peak_cpu=0.0% avg_cpu=83% n=5 verdict=1\n"
                         + judge.verdict_note(["trace"]) + "\n")
            self.assertEqual(judge.read_medians(str(p))["trace"]["n"], 5)

    def test_a_summary_row_that_lost_its_run_index_is_refused_and_not_shortened(self):
        # The drift a summary file has: `measure.sh` writes `NAME-i`, and a row that arrives
        # without its index is skipped by the run parser — the median is then taken over one run
        # fewer than the file holds, under an `n` that says otherwise.
        with tempfile.TemporaryDirectory() as d:
            p = Path(d, "summary.txt")
            p.write_text(SUMMARY + "impact  wall=0.05s user=0.02s sys=0.01s maxrss=0.05GB "
                         "peak_cpu=0% peak_threads=1 samples=0 rc=0\n")
            with self.assertRaises(SystemExit) as e:
                judge.read_medians(str(p))
            self.assertIn(str(p), str(e.exception))
            self.assertIn("impact  wall=0.05s", str(e.exception))

    def test_the_prose_a_summary_travels_with_is_not_read_as_a_drifted_row(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d, "summary.txt")
            p.write_text("quiet: idle=96% load1=1.2 avail=12.0GB\n" + SUMMARY)
            self.assertEqual(judge.read_medians(str(p))["impact"]["n"], 3)

    def test_a_summary_file_of_runs_reads_as_medians_too(self):
        import os
        import tempfile
        with tempfile.TemporaryDirectory() as d:
            p = os.path.join(d, "s.txt")
            with open(p, "w") as f:
                f.write(SUMMARY)
            self.assertEqual(judge.read_medians(p)["impact"]["n"], 3)

    def test_a_reference_written_before_the_average_existed_is_read_without_that_column(self):
        # The references on disk were taken by an instrument that wrote no `avg_cpu`, and a
        # comparison against one is the whole point of keeping them: the row reads, and the
        # column it cannot speak about prints `n/a` instead of failing the file.
        with tempfile.TemporaryDirectory() as d:
            p = Path(d, "medians.txt")
            p.write_text("ask-fused wall=0.61 maxrss=1.55 peak_cpu=120.0 n=5\n")
            ref = judge.read_medians(str(p))
            self.assertIsNone(ref["ask-fused"]["avg_cpu"])
            new = {"ask-fused": {"wall": 0.61, "maxrss": 1.55, "peak_cpu": 120.0, "avg_cpu": 0.98, "n": 5}}
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                ok = judge.print_table(judge.compare(ref, new), ("wall Δ", "RSS Δ", "peak CPU Δ", "avg CPU Δ"))
            self.assertTrue(ok)
            self.assertEqual(out.getvalue().splitlines()[2], "| ask-fused | +0.0% | +0.0% | +0.0% | n/a | ok |")

    def test_a_file_no_line_of_which_is_a_row_is_refused_by_name_and_by_shape(self):
        # Falling through to the run parser and returning nothing put an empty comparison in front
        # of `print_table`, which names no file; the refusal has to say which file and what a line
        # of it should look like.
        with tempfile.TemporaryDirectory() as d:
            p = Path(d, "medians.txt")
            p.write_text("quiet: idle=96%\nzerograph: embedder intfloat/multilingual-e5-small\n")
            with self.assertRaises(SystemExit) as e:
                judge.read_medians(str(p))
            self.assertIn(str(p), str(e.exception))
            self.assertIn("wall=", str(e.exception))


class RefusedRuns(unittest.TestCase):
    """What `medians` must not average. Both shapes come off `measure.sh` looking like readings."""

    def test_a_row_whose_command_exited_non_zero_is_refused_not_averaged(self):
        line = "ask-fused-1  wall=0.02s user=0.01s sys=0.00s maxrss=0.01GB peak_cpu=0% peak_threads=1 samples=0 rc=2"
        with self.assertRaises(SystemExit) as e:
            judge.medians([line])
        self.assertIn("exited 2", str(e.exception))

    def test_a_row_that_predates_the_cpu_fields_reads_with_no_average_rather_than_as_a_failure(self):
        # `user` and `sys` are what the average is derived from and nothing else reads them, so a
        # row without them is a row missing a column — not a run that did not complete.
        line = "control-1  wall=1930.05s user=5600.11s maxrss=2.15GB peak_cpu=293% samples=6 rc=0"
        m = judge.medians([line])
        self.assertEqual(m["control"]["wall"], 1930.05)
        self.assertIsNone(m["control"]["avg_cpu"])

    def test_a_row_with_no_wall_at_all_names_the_field(self):
        line = "ask-fused-1  wall=s user=s sys=s maxrss=GB peak_cpu=0% peak_threads=1 samples=0 rc=0"
        with self.assertRaises(SystemExit) as e:
            judge.medians([line])
        self.assertIn("wall", str(e.exception))
        self.assertIn("maxrss", str(e.exception))

    def test_a_clean_row_still_reads_with_rc_on_the_line(self):
        line = "impact-1  wall=0.04s user=0.02s sys=0.01s maxrss=0.05GB peak_cpu=0% peak_threads=1 samples=0 rc=0"
        self.assertEqual(judge.medians([line])["impact"]["n"], 1)

    def test_a_table_of_no_rows_is_refused_because_all_of_nothing_is_true(self):
        with self.assertRaises(SystemExit):
            judge.print_table([], ("wall", "RSS"))

    def test_a_path_that_cannot_be_read_is_refused_by_name(self):
        for argv in (["judge.py", "medians", "/nope/summary.txt"], ["judge.py", "cadence", "/nope/main.err"]):
            with self.assertRaises(SystemExit) as e:
                judge.main(argv)
            self.assertIn("/nope/", str(e.exception))


FLOOR_LINE = ("bench-dense-1  wall=6.10s user=5.00s sys=0.40s maxrss=1.62GB peak_cpu=340% "
              "peak_threads=9 samples=6 rc=3 floors_missed=1")
# A `trace` that found no path within the depth: the same verdict status, on a row with no floors.
TRACE_LINE = ("trace-1  wall=0.42s user=0.30s sys=0.05s maxrss=0.31GB peak_cpu=0% "
              "peak_threads=1 samples=0 rc=3")
BROKEN_LINE = ("bench-dense-1  wall=0.02s user=0.01s sys=0.00s maxrss=0.01GB peak_cpu=0% "
               "peak_threads=1 samples=0 rc=1")

# What a row's `.time` file holds: the measured command's stderr, then `/usr/bin/time -l`'s report,
# which is written last and is therefore the tail of the file.
TIME_FILE = """\
Error: graph is empty at /Users/max/bench/beauty-crm-test — run build first
        0.02 real         0.01 user         0.00 sys
             1212416  maximum resident set size
                   0  average shared memory size
"""


class FloorVerdicts(unittest.TestCase):
    """An answered question is a reading; a store that was never there is not. `zerograph` exits 3
    for the first and 1 for the second, so a verdict status is what admits the row — for any
    command, because `trace` answers "no path" the same way and spends the same wall clock doing
    it. What the `floors_missed` field additionally takes is a row that runs `bench`: no other
    command has a floor to miss, and 2 is not a verdict at all (`clap` writes it for a usage error
    and the npm launcher for a missing binary)."""

    def test_a_trace_row_that_answered_with_a_verdict_is_a_reading(self):
        m = judge.medians([TRACE_LINE])
        self.assertEqual(m["trace"]["wall"], 0.42)
        self.assertNotIn("floors_missed", str(m["trace"]))

    def test_a_bench_row_that_verdicted_without_the_field_is_refused(self):
        # `bench`'s verdict is a missed floor and the instrument writes the fact beside it. A
        # status with no field is an instrument that did not write it — a name is a weaker claim
        # than a status, and here neither half stands on its own.
        with self.assertRaises(SystemExit) as e:
            judge.medians([FLOOR_LINE.replace(" rc=3 floors_missed=1", " rc=3")])
        self.assertIn("bench-dense", str(e.exception))

    def test_a_usage_error_is_refused_on_every_row(self):
        # 2 is `clap`'s: the command never ran, so the row measured argv parsing.
        for line in (FLOOR_LINE.replace(" rc=3 ", " rc=2 "), TRACE_LINE.replace(" rc=3", " rc=2")):
            with self.assertRaises(SystemExit) as e:
                judge.medians([line])
            self.assertIn("exited 2", str(e.exception))

    def test_a_bench_row_that_missed_a_floor_is_a_reading_and_carries_the_field(self):
        m = judge.medians([FLOOR_LINE])
        self.assertEqual(m["bench-dense"]["n"], 1)
        self.assertEqual(m["bench-dense"]["wall"], 6.10)
        self.assertEqual(m["bench-dense"]["floors_missed"], 1)

    def test_a_bench_row_whose_stderr_said_nothing_of_floors_is_refused(self):
        with self.assertRaises(SystemExit) as e:
            judge.medians([BROKEN_LINE])
        self.assertIn("bench-dense", str(e.exception))
        self.assertIn("exited 1", str(e.exception))

    def test_a_row_that_is_not_a_bench_row_is_refused_whatever_its_stderr_said(self):
        with self.assertRaises(SystemExit) as e:
            judge.medians([FLOOR_LINE.replace("bench-dense-1", "ask-fused-1")])
        self.assertIn("ask-fused", str(e.exception))

    def test_a_bench_row_that_claims_the_field_on_the_old_status_is_refused(self):
        # A binary that exited 1 said nothing this reads, whatever `measure.sh` was told to write.
        with self.assertRaises(SystemExit) as e:
            judge.medians([FLOOR_LINE.replace(" rc=3 ", " rc=1 ")])
        self.assertIn("exited 1", str(e.exception))

    def test_a_row_that_succeeded_and_claims_a_missed_floor_is_refused_for_saying_both(self):
        # The two halves contradict each other, so neither can be believed: `bench` that exits 0
        # met its floors. Every row name, because the disagreement is the fact, not the command.
        for name in ("bench-dense-1", "ask-fused-1"):
            line = FLOOR_LINE.replace("bench-dense-1", name).replace(" rc=3 ", " rc=0 ")
            with self.assertRaises(SystemExit) as e:
                judge.medians([line])
            self.assertIn("floors_missed", str(e.exception))
            self.assertIn("rc=0", str(e.exception))

    def test_the_contradiction_names_the_run_and_quotes_what_it_said(self):
        # The sibling refusal a line above sends its reader to the row's own transcript; this one
        # left them with a row name and five runs to guess between.
        with tempfile.TemporaryDirectory() as d:
            line = FLOOR_LINE.replace("bench-dense-1", "bench-dense-4").replace(" rc=3 ", " rc=0 ")
            Path(d, "summary.txt").write_text(line + "\n")
            Path(d, "bench-dense-4.time").write_text(TIME_FILE)
            with self.assertRaises(SystemExit) as e:
                judge.read_medians(str(Path(d, "summary.txt")))
            msg = str(e.exception)
            self.assertIn("bench-dense-4", msg)
            self.assertIn("graph is empty at /Users/max/bench/beauty-crm-test", msg)
            self.assertNotIn("maximum resident set size", msg)

    def test_a_verdict_on_a_row_with_no_floors_is_kept_and_named(self):
        # The row is a reading — it traversed for its wall clock and then said no — but it is a
        # reading of an answer, and a bar set on it is set on the answer. So it is kept with a
        # flag of its own and the table says which rows carry it.
        m = judge.medians([TRACE_LINE])
        self.assertEqual(m["trace"]["verdict"], 1)
        self.assertNotIn("floors_missed", str(m["trace"]))

    def test_the_verdict_flag_rides_through_medians_and_the_table_says_it(self):
        with tempfile.TemporaryDirectory() as d:
            summary = Path(d, "summary.txt")
            summary.write_text(TRACE_LINE + "\n" + SUMMARY)
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                judge.main(["judge.py", "medians", str(summary)])
            printed = out.getvalue()
            self.assertIn("trace: answered with a verdict", printed)
            medians = Path(d, "medians.txt")
            medians.write_text(printed)
            back = judge.read_medians(str(medians))
            self.assertEqual(back["trace"]["verdict"], 1)
            self.assertNotIn("verdict", str(back["ask-fused"]))

    def test_both_tables_name_the_rows_that_answered_with_a_verdict(self):
        rows = {"trace": {"wall": 0.42, "maxrss": 0.31, "peak_cpu": 0.0, "avg_cpu": 0.83, "n": 5, "verdict": 1}}
        clean = {"trace": {"wall": 0.42, "maxrss": 0.31, "peak_cpu": 0.0, "avg_cpu": 0.83, "n": 5}}
        self.assertEqual(judge.verdict_rows(clean, rows), {"trace"})
        for judged, head in ((judge.compare(rows, clean), ("wall Δ", "RSS Δ", "peak CPU Δ", "avg CPU Δ")),
                             (judge.control(rows, clean), ("wall spread", "RSS spread", "avg CPU spread"))):
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                judge.print_table(judged, head, verdicts=judge.verdict_rows(rows, clean))
            self.assertIn("trace: answered with a verdict", out.getvalue())
        # And a table no row of which answered says nothing about verdicts.
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            judge.print_table(judge.compare(clean, clean), ("wall Δ", "RSS Δ", "peak CPU Δ", "avg CPU Δ"),
                              verdicts=judge.verdict_rows(clean, clean))
        self.assertNotIn("verdict", out.getvalue())

    def test_a_clean_bench_row_keeps_the_shape_every_other_row_has(self):
        line = FLOOR_LINE.replace(" rc=3 floors_missed=1", " rc=0")
        self.assertNotIn("floors_missed", str(judge.medians([line])["bench-dense"]))

    def test_the_refusal_quotes_the_tail_of_what_that_row_said(self):
        with tempfile.TemporaryDirectory() as d:
            Path(d, "summary.txt").write_text(BROKEN_LINE + "\n")
            Path(d, "bench-dense-1.time").write_text(TIME_FILE)
            with self.assertRaises(SystemExit) as e:
                judge.read_medians(str(Path(d, "summary.txt")))
            msg = str(e.exception)
            self.assertIn("graph is empty at /Users/max/bench/beauty-crm-test", msg)
            # `time`'s report is the tail of the file and never what the command said.
            self.assertNotIn("maximum resident set size", msg)

    def test_a_refusal_with_no_transcript_beside_it_still_names_the_row(self):
        with tempfile.TemporaryDirectory() as d:
            Path(d, "summary.txt").write_text(BROKEN_LINE + "\n")
            with self.assertRaises(SystemExit) as e:
                judge.read_medians(str(Path(d, "summary.txt")))
            self.assertIn("bench-dense", str(e.exception))

    def test_the_field_survives_the_round_trip_through_medians(self):
        with tempfile.TemporaryDirectory() as d:
            summary = Path(d, "summary.txt")
            summary.write_text(FLOOR_LINE + "\n" + SUMMARY)
            out = io.StringIO()
            with contextlib.redirect_stdout(out):
                judge.main(["judge.py", "medians", str(summary)])
            printed = out.getvalue()
            self.assertIn("floors_missed: bench-dense", printed)
            medians = Path(d, "medians.txt")
            medians.write_text(printed)
            back = judge.read_medians(str(medians))
            self.assertEqual(back["bench-dense"]["floors_missed"], 1)
            self.assertEqual(back["bench-dense"]["n"], 1)
            # A row nothing happened to reads exactly as it always has, note line and all.
            self.assertNotIn("floors_missed", [l for l in printed.splitlines() if l.startswith("ask-fused")][0])
            self.assertNotIn("floors_missed", str(back["ask-fused"]))

    def test_the_table_marks_the_row_and_says_it_under_the_verdict(self):
        ref = {"bench-dense": {"wall": 6.0, "maxrss": 1.60, "peak_cpu": 340.0, "avg_cpu": 3.4, "n": 5, "floors_missed": 1}}
        new = {"bench-dense": {"wall": 6.1, "maxrss": 1.60, "peak_cpu": 340.0, "avg_cpu": 3.4, "n": 5}}
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            ok = judge.print_table(judge.compare(ref, new), ("wall Δ", "RSS Δ", "peak CPU Δ", "avg CPU Δ"),
                                   judge.floors_rows(ref, new))
        lines = out.getvalue().splitlines()
        self.assertTrue(ok)
        self.assertEqual(lines[2], "| bench-dense | +1.7% | +0.0% | +0.0% | +0.0% | ok, floors_missed |")
        self.assertTrue(lines[-1].startswith("floors_missed: bench-dense"))

    def test_a_table_no_row_of_which_missed_a_floor_says_nothing_about_them(self):
        rows = {"ask-fused": {"wall": 0.6, "maxrss": 1.5, "peak_cpu": 120.0, "avg_cpu": 0.98, "n": 5}}
        self.assertEqual(judge.floors_rows(rows, rows), set())
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            judge.print_table(judge.control(rows, rows), ("wall spread", "RSS spread", "avg CPU spread"),
                              judge.floors_rows(rows, rows))
        self.assertNotIn("floors_missed", out.getvalue())


class Control(unittest.TestCase):
    def test_the_same_binary_twice_inside_the_bars_passes(self):
        a = {"ask-fused": {"wall": 0.60, "maxrss": 1.50, "peak_cpu": 120.0, "avg_cpu": 0.98, "n": 5}}
        b = {"ask-fused": {"wall": 0.63, "maxrss": 1.55, "peak_cpu": 121.0, "avg_cpu": 0.99, "n": 5}}
        (r,) = judge.control(a, b)
        # Each spread is the gap over the mean of the two readings: 0.03 s over 0.615 s, 0.05 GB
        # over 1.525 GB, 0.01 cores over 0.985.
        self.assertEqual((r.row, r.wall, r.rss, r.avg), ("ask-fused", 0.0488, 0.0328, 0.0102))
        self.assertTrue(r.ok)

    def test_the_average_cpu_spread_is_reported_and_does_not_decide_the_control(self):
        # What a control is for: the same binary twice says how repeatable each column is. The
        # average moves with `/usr/bin/time`'s 10 ms resolution on a row that lasts 40 ms, so the
        # spread is printed for a reader and no verdict is taken on it until one has been read.
        a = {"impact": {"wall": 0.04, "maxrss": 0.05, "peak_cpu": 0.0, "avg_cpu": 0.75, "n": 5}}
        b = {"impact": {"wall": 0.04, "maxrss": 0.05, "peak_cpu": 0.0, "avg_cpu": 1.00, "n": 5}}
        (r,) = judge.control(a, b)
        self.assertEqual((r.row, r.wall, r.rss), ("impact", 0.0, 0.0))
        self.assertAlmostEqual(r.avg, 0.2857, places=4)
        self.assertTrue(r.ok)

    def test_a_control_one_side_of_which_predates_the_column_reports_no_spread(self):
        a = {"impact": {"wall": 0.04, "maxrss": 0.05, "peak_cpu": 0.0, "avg_cpu": None, "n": 5}}
        b = {"impact": {"wall": 0.04, "maxrss": 0.05, "peak_cpu": 0.0, "avg_cpu": 0.75, "n": 5}}
        self.assertIsNone(judge.control(a, b)[0].avg)
        self.assertIsNone(judge.control(b, a)[0].avg)

    def test_a_control_reads_the_same_whichever_run_is_given_first(self):
        # The two sides of a control are the same binary and are interchangeable, so the whole row
        # — both judged spreads, the reported one and the verdict — must not depend on which run
        # was named first. Taken against the first side, an 11% pair read 11% one way and 9.9% the
        # other, across the 10% bar.
        a = {"impact": {"wall": 1.00, "maxrss": 0.05, "peak_cpu": 0.0, "avg_cpu": 0.75, "n": 5}}
        b = {"impact": {"wall": 1.11, "maxrss": 0.05, "peak_cpu": 0.0, "avg_cpu": 1.00, "n": 5}}
        self.assertEqual(judge.control(a, b), judge.control(b, a))

    def test_a_wall_pair_across_the_bar_is_outside_from_either_side(self):
        # 0.11 s over a mean of 1.055 s is 10.4%, over the 10% bar, and says so both ways round.
        a = {"impact": {"wall": 1.00, "maxrss": 0.05, "peak_cpu": 0.0, "avg_cpu": 0.75, "n": 5}}
        b = {"impact": {"wall": 1.11, "maxrss": 0.05, "peak_cpu": 0.0, "avg_cpu": 0.75, "n": 5}}
        for first, second in ((a, b), (b, a)):
            (r,) = judge.control(first, second)
            self.assertEqual(r.wall, 0.1043)
            self.assertFalse(r.ok)

    def test_a_side_that_measured_no_cpu_at_all_is_a_reading_and_not_an_absent_column(self):
        # Both runs kept a core busy for under one tick of `/usr/bin/time`'s 10 ms, on a row that
        # ran and was measured. Two readings of zero are a spread of zero; `n/a` there would say
        # the column was never read.
        lines = [f"impact-{i}  wall=0.04s user=0.00s sys=0.00s maxrss=0.05GB peak_cpu=0% "
                 "peak_threads=1 samples=0 rc=0" for i in range(1, 6)]
        rows = judge.medians(lines)
        (r,) = judge.control(rows, judge.medians(lines))
        self.assertEqual(r.avg, 0.0)

    def test_a_row_outside_either_bar_fails_and_names_which(self):
        a = {"dump10": {"wall": 0.70, "maxrss": 1.36, "peak_cpu": 100.0, "avg_cpu": 0.9, "n": 5}}
        b = {"dump10": {"wall": 0.90, "maxrss": 1.36, "peak_cpu": 100.0, "avg_cpu": 0.9, "n": 5}}
        (r,) = judge.control(a, b)
        self.assertFalse(r.ok)
        self.assertAlmostEqual(r.wall, 0.25, places=4)
        self.assertEqual(r.rss, 0.0)

    def test_a_row_missing_from_one_side_is_reported_not_skipped(self):
        with self.assertRaises(SystemExit):
            judge.control({"a": {"wall": 1, "maxrss": 1, "peak_cpu": 1, "avg_cpu": 1, "n": 5}}, {})

    def test_a_row_only_the_second_run_has_is_reported_too(self):
        with self.assertRaises(SystemExit):
            judge.control({}, {"a": {"wall": 1, "maxrss": 1, "peak_cpu": 1, "avg_cpu": 1, "n": 5}})

    def test_a_candidate_row_the_reference_never_had_is_reported(self):
        with self.assertRaises(SystemExit):
            judge.compare({}, {"a": {"wall": 1, "maxrss": 1, "peak_cpu": 1, "avg_cpu": 1, "n": 5}})


class CompareCpuColumns(unittest.TestCase):
    """The two CPU columns and what each of them is worth. The sampled peak is judged: G19's
    control triple — 1,930 s, 293%, 2.15 GB — is the shape it exists for, and the reader rows are
    the shape it cannot speak about. The derived average is printed beside it and judged by
    nobody: `/usr/bin/time` reports to 10 ms, so one tick moves a 0.04 s row by a fifth, and no
    control has yet read what that column's own spread is."""

    def row(self, wall, rss, cpu, avg=1.0):
        return {"wall": wall, "maxrss": rss, "peak_cpu": cpu, "avg_cpu": avg, "n": 5}

    def test_a_candidate_inside_the_cpu_bar_passes(self):
        ref = {"embed": self.row(1930.0, 2.15, 293.0)}
        new = {"embed": self.row(1930.0, 2.15, 315.0)}
        (r,) = judge.compare(ref, new)
        self.assertAlmostEqual(r.peak, 0.0751, places=4)
        self.assertTrue(r.ok)

    def test_a_candidate_outside_the_cpu_bar_fails_on_that_column_alone(self):
        ref = {"embed": self.row(1930.0, 2.15, 293.0)}
        new = {"embed": self.row(2026.5, 2.15, 360.0)}
        (r,) = judge.compare(ref, new)
        self.assertEqual((r.wall, r.rss), (0.05, 0.0))
        self.assertAlmostEqual(r.peak, 0.2287, places=4)
        self.assertFalse(r.ok)

    def test_a_row_the_sampler_never_caught_is_unjudged_rather_than_green(self):
        ref = {"impact": self.row(0.04, 0.05, 0.0)}
        (r,) = judge.compare(ref, {"impact": self.row(0.04, 0.05, 0.0)})
        self.assertIsNone(r.peak)
        # And the derived column still reads where the sampled one cannot; it is reported, not judged.
        self.assertEqual(r.avg, 0.0)
        self.assertTrue(r.ok)
        # A reference of 0 is no reading, not a reading of zero, so a candidate that did get
        # sampled has nothing to be within 10% of either — the wall column is what moved.
        (r,) = judge.compare(ref, {"impact": self.row(0.04, 0.05, 130.0)})
        self.assertIsNone(r.peak)
        self.assertTrue(r.ok)

    def test_a_candidate_the_sampler_never_caught_is_unjudged_rather_than_a_move_to_zero(self):
        # The sampler is the same sampler on both sides: a candidate reading 0 is `top` saying
        # nothing about that run, so there is nothing to place against the reference's 293%.
        # Reading it as -100% would report the loudest possible improvement for a run nobody
        # measured, and the row is still judged on the wall clock and max RSS it does have.
        ref = {"embed": self.row(1930.0, 2.15, 293.0)}
        (r,) = judge.compare(ref, {"embed": self.row(1930.0, 2.15, 0.0)})
        self.assertIsNone(r.peak)
        self.assertEqual((r.wall, r.rss), (0.0, 0.0))
        self.assertTrue(r.ok)

    def test_the_table_prints_four_delta_columns_and_names_the_unjudged_one(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            judge.print_table(judge.compare({"impact": self.row(0.04, 0.05, 0.0)},
                                            {"impact": self.row(0.04, 0.05, 0.0)}),
                              ("wall Δ", "RSS Δ", "peak CPU Δ", "avg CPU Δ"))
        lines = out.getvalue().splitlines()
        self.assertEqual(lines[0], "| row | wall Δ | RSS Δ | peak CPU Δ | avg CPU Δ | |")
        self.assertEqual(lines[1], "|---|---|---|---|---|---|")
        self.assertEqual(lines[2], "| impact | +0.0% | +0.0% | n/a | +0.0% | ok |")

    def test_an_average_far_outside_the_others_is_reported_and_the_row_still_passes(self):
        # 43% on the derived average with wall, RSS and peak all inside: no bar has been set on
        # this column, so the number is printed and the verdict is taken on the other three.
        ref = {"ask-fused": self.row(0.61, 1.55, 0.0, avg=0.98)}
        new = {"ask-fused": self.row(0.61, 1.55, 0.0, avg=1.40)}
        (r,) = judge.compare(ref, new)
        self.assertEqual((r.wall, r.rss), (0.0, 0.0))
        self.assertIsNone(r.peak)
        self.assertAlmostEqual(r.avg, 0.4286, places=4)
        self.assertTrue(r.ok)

    def test_a_move_off_a_measured_zero_average_reads_as_an_infinite_one(self):
        # A reference of 0.00 cores is a reading, not an absent column: the candidate keeping a
        # core and a quarter busy is an unbounded move off it, and `n/a` would say the column was
        # never read. No bar is set on this column, so the row still passes.
        ref = {"impact": self.row(0.04, 0.05, 0.0, avg=0.0)}
        (r,) = judge.compare(ref, {"impact": self.row(0.04, 0.05, 0.0, avg=12.5)})
        self.assertEqual(r.avg, float("inf"))
        self.assertTrue(r.ok)

    def test_a_wall_clock_that_rounded_away_puts_the_row_outside_rather_than_green(self):
        # One rule for every column: a reference of 0.00 s is a reading, and a candidate that
        # spent a second and a half moved off it by an unbounded amount. Reading that as "unchanged"
        # would clear the largest regression the column can show.
        ref = {"impact": self.row(0.00, 0.05, 0.0)}
        (r,) = judge.compare(ref, {"impact": self.row(1.50, 0.05, 0.0)})
        self.assertEqual(r.wall, float("inf"))
        self.assertFalse(r.ok)

    def test_two_wall_clocks_that_both_rounded_away_have_not_moved(self):
        ref = {"impact": self.row(0.00, 0.05, 0.0)}
        (r,) = judge.compare(ref, {"impact": self.row(0.00, 0.05, 0.0)})
        self.assertEqual(r.wall, 0.0)
        self.assertTrue(r.ok)

    def test_two_measured_zero_averages_have_not_moved(self):
        ref = {"impact": self.row(0.04, 0.05, 0.0, avg=0.0)}
        (r,) = judge.compare(ref, {"impact": self.row(0.04, 0.05, 0.0, avg=0.0)})
        self.assertEqual(r.avg, 0.0)
        self.assertTrue(r.ok)

    def test_an_average_a_side_never_read_stays_unjudged_and_prints_as_absent(self):
        ref = {"impact": self.row(0.04, 0.05, 0.0, avg=None)}
        (r,) = judge.compare(ref, {"impact": self.row(0.04, 0.05, 0.0, avg=1.2)})
        self.assertIsNone(r.avg)

    def test_the_table_prints_an_infinite_average_move_instead_of_raising(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            judge.print_table(judge.compare({"impact": self.row(0.04, 0.05, 0.0, avg=0.0)},
                                            {"impact": self.row(0.04, 0.05, 0.0, avg=12.5)}),
                              ("wall \u0394", "RSS \u0394", "peak CPU \u0394", "avg CPU \u0394"))
        self.assertEqual(out.getvalue().splitlines()[2],
                         "| impact | +0.0% | +0.0% | n/a | +inf% | ok |")

    def test_the_control_table_reports_the_average_beside_the_two_its_clause_judges(self):
        out = io.StringIO()
        with contextlib.redirect_stdout(out):
            judge.print_table(judge.control({"impact": self.row(0.04, 0.05, 0.0)},
                                            {"impact": self.row(0.04, 0.05, 0.0)}),
                              ("wall spread", "RSS spread", "avg CPU spread"))
        self.assertEqual(out.getvalue().splitlines()[0],
                         "| row | wall spread | RSS spread | avg CPU spread | |")


class RunCount(unittest.TestCase):
    """The `n` every row has always carried, now read. §1 judges its control at five runs a row;
    §9's embed is three, and a suite with its own shape says the number instead of inheriting it."""

    def rows(self, n):
        return {"ask-fused": {"wall": 0.60, "maxrss": 1.50, "peak_cpu": 120.0, "avg_cpu": 0.98, "n": n}}

    def test_a_control_over_one_run_a_row_is_refused_not_cleared(self):
        with self.assertRaises(SystemExit) as e:
            judge.control(self.rows(1), self.rows(1))
        self.assertIn("n=1", str(e.exception))

    def test_a_candidate_under_the_floor_is_refused_too(self):
        with self.assertRaises(SystemExit):
            judge.compare(self.rows(5), self.rows(3))

    def test_a_suite_of_three_runs_is_judged_when_the_floor_is_said_out_loud(self):
        (r,) = judge.compare(self.rows(3), self.rows(3), min_n=3)
        self.assertEqual((r.wall, r.ok), (0.0, True))

    def test_the_two_file_subcommands_print_usage_instead_of_an_index_error(self):
        for argv in (["judge.py", "control", "a.txt"], ["judge.py", "compare", "a.txt"]):
            with self.assertRaises(SystemExit) as e:
                judge.main(argv)
            self.assertIn("usage", str(e.exception))

    def test_a_floor_that_is_not_a_number_prints_usage_instead_of_a_traceback(self):
        # `²` is a digit `int` refuses, so the guard has to ask what `int` asks and not what
        # `isdigit` answers, or the traceback arrives through the clause that replaced it.
        for floor in ("three", "3.5", "²"):
            with self.assertRaises(SystemExit) as e:
                judge.main(["judge.py", "compare", "a.txt", "b.txt", floor])
            self.assertIn("usage", str(e.exception))

    def test_a_floor_of_zero_is_refused_because_it_admits_the_reading_the_floor_exists_to_stop(self):
        for floor in ("0", "-3"):
            with self.assertRaises(SystemExit) as e:
                judge.main(["judge.py", "control", "a.txt", "b.txt", floor])
            self.assertIn("usage", str(e.exception))


class Cadence(unittest.TestCase):
    def test_intervals_are_read_from_the_stamps_not_from_rows_per_second(self):
        c = judge.cadence(STAMPED.splitlines())
        self.assertEqual(c["first"], 0.9)
        self.assertEqual(c["intervals"], [0.9, 3.0, 6.0, 8.3])
        self.assertEqual(c["max"], 8.3)
        self.assertEqual(c["median"], 6.0)
        self.assertAlmostEqual(c["ratio"], 8.3 / 6.0, places=3)

    def test_the_first_interval_counts_from_process_start_not_from_model_open(self):
        lines = ["50.00 start", "80.00 dense: model open in 30.0s", "85.00 dense: 128/9 rows, 1.0 rows/s, ~1 min left"]
        self.assertEqual(judge.cadence(lines)["first"], 35.0)

    def test_a_transcript_with_no_progress_line_is_refused(self):
        with self.assertRaises(SystemExit):
            judge.cadence(["1.00 start", "2.00 dense: embedded 0 rows in 1.0s"])


class Verdict(unittest.TestCase):
    def test_cadence_verdict_needs_every_interval_under_the_bar_and_a_flat_tail(self):
        ok, why = judge.cadence_ok({"first": 0.9, "intervals": [0.9, 3.0, 6.0, 8.3], "max": 8.3, "median": 6.0, "ratio": 8.3 / 6.0}, bar=60.0, tail=1.3)
        self.assertFalse(ok)
        self.assertIn("1.383", why)
        ok, _ = judge.cadence_ok({"first": 0.9, "intervals": [5.0, 6.0], "max": 6.0, "median": 6.0, "ratio": 1.0}, bar=60.0, tail=1.3)
        self.assertTrue(ok)


if __name__ == "__main__":
    unittest.main()
