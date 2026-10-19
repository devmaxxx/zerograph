# The cost-family probe

What every reader and writer costs, measured the same way every time. This directory is the
2026-09-07 kit brought into the tree so the bars it sets can be reviewed in a diff; the pinned
fixture, the one writable worktree and the legacy graph stay under `~/bench/`.

Two directories, both locked: `~/bench/beauty-crm-502e8a6d`, the pinned fixture, read-only for ever
— `bench` and `dump` read its store as it stands, nothing walks it, nothing writes it; and
`~/bench/beauty-crm-test`, the one writable worktree, where every writer and every reader row runs.

- `quiet.sh` — the precondition every row is read under: ≥ 85% idle, 1-minute load < 3.0, AC power,
  no `cargo`/`rustc`/`zerograph`/`repograph` at all, and no other `node` at or above 5.0% CPU. The two
  rules differ because the two kinds of noise do. This project's own tools are bursty, so presence
  is the whole rule for them: a `cargo` at 0% is between two crates and a `zerograph` at 0% is
  about to embed. A `node` at 0% is a sibling harness or an MCP server asleep, and a process
  consuming no CPU moves no wall clock — counting those refused this machine on 66 of them, which
  measured the session and not the noise. The one exemption is this script's own ancestor chain
  (`$$` upwards through `PPID`), at any CPU: the harness that launched the probe is unavoidable and
  the `load1` clause refuses it on its own when it is busy. The printed line names what each rule
  counted — `busy=0 (cargo/rustc/zerograph: 0, node ≥5.0%: 0)`, a label that keeps the old name —
  and each refusal lists the `pid pcpu comm` rows behind its count. What says whether the machine
  spoiled a row is not this clause but G23's control, the same binary through the suite twice; the
  precondition is here to catch the obvious cases cheaply. Sourcing the script defines
  `ancestor_pids`, `busy_lines` and `busy_rows` and runs nothing, which is how `test_quiet.py`
  reads the walk and both rules on a machine that is not quiet. `readers.sh` and `embed.sh`, the two that time anything, run it first
  and refuse; `reset.sh` and `arms.sh` measure no wall clock and do not.
- `embedder.sh` — sourced, never run: the one place that names which weights every row here is read
  under, and the refusal when the environment does not say so. `reset.sh`, `readers.sh`, `embed.sh`
  and `arms.sh` each source it and refuse with exit 2 unless `ZEROGRAPH_EMBED_MODEL` is exported and
  names `intfloat/multilingual-e5-small`, so every invocation of the kit reads
  `ZEROGRAPH_EMBED_MODEL=intfloat/multilingual-e5-small bench/probe/<script> …` (or exports it once
  for the session). Declared in the environment rather than in a `zerograph.toml` beside the store
  for two reasons the file could not give: the variable outranks the model a store records, so a
  store restored under a changed built-in default is still read and embedded with the weights the
  reading names — the default was `e5-large` between `35357c1` and 2026-09-07 under ADR-002 — and it
  leaves the corpus worktree with no file of ours in it, where an untracked `zerograph.toml` is a
  changed path `changes` reads and every `changes` row would map two paths instead of the one it
  touched. Unset is refused rather than defaulted, so a row can never be read under whatever the
  built-in default happens to be that month, and a value naming another model is refused rather than
  replaced. The declaration is exported as `REPOGRAPH_EMBED_MODEL` too, the only name a pinned
  pre-rename binary reads, and one already set to other weights is refused. The declaration travels onto each row's transcript — a line in `summary.txt` beside the
  quiet line for `readers.sh` and `embed.sh`, the head of each arm's own output for `arms.sh` — and
  onto `readers.sh`'s `medians.txt`, which is the file copied out as the reference every later
  reading is judged against, so a reference cannot be compared against a suite read under other
  weights. `judge.py medians` and `read_medians` take only lines shaped like a run and a median, so
  the line travels through both without being read as a row.
- `reset.sh` — the writable worktree put back to the fixture's state: tree reverted and cleaned,
  store copied in, stamps settled by one `--no-dense update`. Refuses any directory that is not the
  locked worktree at `502e8a6d`. Before every arm, reader suites included.
  What it leaves has nothing untracked in it — `git status --porcelain` is empty — which is what the
  `changes` rows need, and it removes a `zerograph.toml` (or pre-rename `repograph.toml`) an earlier reset of this kit wrote.
- `measure.sh NAME LOG -- cmd…` — one command: wall, user, sys, max RSS, sampled peak CPU and threads.
- `readers.sh BIN WORKTREE LOG [N]` — the ten reader rows, N runs each, every row `--stale`, medians
  in `LOG/medians.txt`. In the writable worktree straight after `reset.sh`; the suite writes nothing,
  and the store's checksums before and after are how that is shown.
- `embed.sh NAME BIN WORKTREE LOG` — a whole-store embed with stamped stderr, for the cadence. It
  removes the vectors first, so it refuses the pinned fixture by path.
- `arms.sh BIN REPO LOG TAG` — the four bench arms, recorded into `bench/history/runs.jsonl`; `REPO`
  is the fixture for a baseline read.
- `judge.py` — medians, the twice-through-one-binary control, a candidate against a reference, the
  cadence reading. `graphdiff.py` — two `graph.json` files, same visible graph or not.

The bars: a reader row is judged as a median of n ≥ 5, within 10% wall and 5% max RSS of its
reference, and only once `judge.py control` has shown the same binary twice inside those bars on
this machine (`docs/bench/2026-09-10-next-version-levers-results.md` §1 is the first such control).
That `n` is read and not just printed: `control` and `compare` refuse a row built from fewer runs
than the floor, which is five unless a last argument (`judge.py compare REF NEW 3`) says otherwise.
A whole-store embed is judged as a median of three against a control of three, and §9 reads its
three clauses off the summary lines by hand. A control's spread is the gap between its two runs
over the **mean** of the two, on every column it prints: the two sides are the same binary and are
interchangeable, and a spread taken against whichever run was named first put an 11% pair on both
sides of the 10% bar depending on the order of the arguments. A candidate's delta is a different
reading — there the reference is not interchangeable with the candidate, and the move is stated as
a fraction of it, under one rule on every column: a side that has no reading at all — a peak
`top` never sampled, an average with no run to read it off — prints `n/a` and is judged on
nothing, while a 0.00 wall clock, max RSS or average is a reading, so a candidate that reads zero
too has not moved and a candidate that moved off zero moved `+inf%` and is outside any bar set on
that column. Peak CPU's 0 is never a reading, on either side, because it is the sampler saying it
never caught the row. The derived average CPU is reported on every row and
judged on none: it is printed by `compare`, its spread is printed by `control`, and no bar is set
on it until a control has said what that spread is.

**A missed floor is a reading; a broken store is not.** `judge.py medians` refuses a whole suite
when any row exited non-zero, because a command that failed still gets a full `time` report and its
row would enter the median as an honestly-measured fast run. One non-zero exit is not that: a
`bench` row that answered every case and then missed a floor spent its wall clock reading, and its
wall, max RSS and both CPU columns are readings of that reader — while a `bench` row that found no store
never did the work at all. **As of 0.5.0 the status says which:** `zerograph` exits 3 for a
question it answered — a suite that ran every case and missed a floor — and 1 where there was
nothing to measure. 3 and not 2, because 2 is written above the command: `clap` answers a mistyped
flag with it and the npm launcher answers a missing platform binary with it, so a 2 never reached
a reader and is refused on every row.

**The status admits the row; the field is narrower.** `trace` answers "no path within the depth"
with the same 3 after spending its wall clock traversing, so `judge.py` reads a verdict row of any
command as a reading. Only a `bench` row has a floor to miss, so `measure.sh` writes
`floors_missed=1` on those alone — `bench`, `bench-dense`, `bench-nodense`, matched by the same
expression `judge.py` matches — and a verdict on any other row is recorded with the status and no
field, under a `verdict=1` of its own: the row is kept and named, because a bar set on it is set on
an answer and not on the traversal the row was written to time. `judge.py` requires both halves to
agree: a `bench` row that exited 3 without the field is refused, so is any other row that carries
it, and so is a row that claims it beside an exit 0 — the field and the status contradict each
other there. The status is the interface; the wording is a message and no script reads it.
**The kit therefore needs a binary that
exits 3 for a verdict:** an older one exits 1 with the old words, and its missed floors are refused
as the failures they are then indistinguishable from. Every other non-zero exit is still refused, and the
refusal quotes the tail of what the row said, so a reader is told what failed and not only that
something did. The field rides through `medians.txt` on the rows that carry it, and `control` and
`compare` print it in the verdict cell and say it in a line under the table. `arms.sh` has ruled
the same way on the same event since `deceb5c` — an arm that fails a floor is still a recorded arm
— and the two now agree.

**What each CPU column is worth.** The two are different readings of the same run and neither
replaces the other. `avg_cpu` is `(user + sys) / wall` off the `time` report `measure.sh` already
writes, so it exists for a row that lasted 40 ms; `peak_cpu` is what `top` caught at its loudest
and exists only for a row that lasted long enough to be sampled. A summary too old to carry `user`
and `sys` reads without the average rather than being refused, and so does a `medians.txt` written
before the column existed — the references on disk are exactly those files.

**The peak CPU column is judged wherever it was sampled; the average is reported everywhere.**
`judge.py compare` prints four columns and judges three: wall against 10%, max RSS against 5%,
peak CPU against 10%, and the average beside them with no bar on it. Peak CPU is judged because
§9's gate asks for the run's peak CPU unmoved and a clause no code reads is green whatever the run
did; it is only as good as the sampler under it, so a command that finishes inside about two
seconds is never sampled and reports `peak_cpu = 0`. That is every reader row, and it is not the
whole-store embed of §9 (≈ 1,930 s, 293% recorded), which is the row the column was added for. A
row whose peak read 0 on either side prints `n/a` and is left unjudged on that column rather than
passing on a zero that means "never sampled", or reading an unsampled candidate against a sampled
reference as a fall to nothing; an average is printed as `n/a` only where one of the two
sides has no reading at all — a file written before the column existed, or a row every run of which
rounded its wall clock to zero and left nothing to divide. One such run among five costs the column
that run and not the row: the median is taken over the runs that read, beside the `n` that counts
them all. A row that ran and kept a core busy for under one tick of `/usr/bin/time`'s 10 ms reads
`0.00` and is a reading like any other: two such rows spread by nothing, a measured `0.00`
reference against a candidate that did keep a core busy reads `+inf%`, and `n/a` in either place
would say the column was never read. In a `medians.txt` the average is written as `NN%` and in no
other unit — a bare number is a row whose spelling drifted and is refused by name, because a field readable as
either cores or percent is one a reader has to guess at and the guess is a hundredfold wide. Both
files are judged whole on one check: a line carrying a `wall=` that reads as neither a median nor a
run is refused naming the file and the line — a `medians.txt` row reworded, a `summary.txt` row
that lost its `-N` index — rather than skipped into a median taken over fewer runs than the file
holds. Prose carries no `wall=` and passes through both.
The average carries no bar because `/usr/bin/time` reports user and sys to 10 ms: on a 0.04 s row
one tick of either moves the column by a fifth to a third, which a 10% bar would fail on a machine
that did nothing wrong. `judge.py control` prints the average's own spread beside the wall and RSS
spreads it judges — that is the reading a bar would have to be set from, and nobody has taken it
yet. The one row the peak column judges never carries a zero into the table at all: `embed.sh`
counts the samples its peak was taken over, puts the count on the row as `samples=` beside
`measure.sh`'s, and refuses to print a row where the sampler read nothing or read a peak of zero —
refusing when no PID was ever found guards one door of two, and the other is every `top` failing or the process ending between two checks. The
verdicts keep the columns their own committed clauses name: `judge.py control` judges wall and max
RSS only, which is §1's Gate as written, and a reader row read under §1's bars is judged by those
two columns — both CPU columns are printed beside them and neither is part of that clause.
macOS only:
`/usr/bin/time -l`, `top -l`, `pmset` and `caffeinate` are Darwin's, and CI runs none of the shell
here — `python3 -m unittest discover -s bench/probe` is what a change to this directory is gated on,
and its `reset.sh` and `quiet.sh` tests skip themselves off POSIX, the latter's one whole-script
case that needs a real harness also where there is no `node` to be one.
