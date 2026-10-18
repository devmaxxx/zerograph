# Running the comparison again — the runbook

[`three-graphs.md`](three-graphs.md) is the protocol: what is measured and how an
answer is judged. This file is the operational half — what to do, in order, on the
day someone re-runs it, the rule a retrieval change is judged by, and the seven things
that went wrong on an earlier run so they do not have to be rediscovered.

The last run is [`2026-09-04-three-graphs-results.md`](2026-09-04-three-graphs-results.md).

## The pinned fixture — read this before rebuilding anything

Everything below describes a full three-tool run, which needs both foreign indexes built and
takes roughly half an hour. Most runs do not need that. There is a fixture that already has
all three:

```
~/bench/beauty-crm-502e8a6d      # detached worktree of the corpus at 502e8a6d
```

| what | where | built by | cost to rebuild |
|---|---|---|---|
| corpus at `502e8a6d` | the worktree itself | — | 856 MB |
| gitnexus index | `.gitnexus/` | gitnexus 1.6.9 | 218 s, 598 MB |
| graphify graph | `graphify-out/graph.json` | graphify 0.9.48 | 24 s, 123 MB |
| zerograph store | `.repograph/` | see below | 78 MB |

The zerograph store carries the graph, the dense vectors and the enrichment questions, so the
fixture answers in the enriched arm without anyone paying for `enrich` again. The vectors and
`questions.json` were copied from the working clone's store at the same corpus commit rather
than recomputed. That transfer is only valid because both stores were built from the same
`502e8a6d` graph, so it was checked rather than assumed: one binary, both repositories, both
arms, and all 82 cases came back identical case by case, not merely equal in total.

| arm | as measured 2026-09-04 | current | working clone |
|---|---|---|---|
| dense | keyword 40/40, paraphrase 15/30, code 12/12, p90 226 | keyword 40/40, paraphrase 15/30, code 12/12, p90 220 | identical |
| lexical | keyword 37/40, paraphrase 15/30, code 12/12, p90 220 | keyword 39/40, paraphrase 14/30, code 12/12, p90 215 | identical |

The 2026-09-04 column predates the questions-list gate; `current` is the same fixture read after
it (`bench/history/runs.jsonl`). What the transfer check established is that the two stores answer
case by case alike, which the gate does not touch.

**For a zerograph-only run, use `bench/history/run-zerograph.sh` and stop reading here.** It
builds the binary, runs both arms against the fixture, records them into the run history and
prints what improved, what regressed and what is chronically weak. Two minutes, no tokens.

Two things about the fixture that will otherwise cost an afternoon:

- **The fixture's `git status` is dirty by design.** `graphify-out/` is tracked in the corpus
  and graphify rewrites it in place. Four modified files and one new `.sig` are the deliverable,
  not debris. Judge cleanliness with `git status --porcelain -- ':(exclude)graphify-out'`.
- **gitnexus registers the fixture under the name `beauty-crm`, which the working clone already
  uses.** Plain-name resolution picks whichever was indexed last, silently, and a wrong name
  does not error — it answers about the other repository. `run.py` already defaults
  `--gitnexus-repo` to the corpus path, so the rule is simply never to override it with a name.

Rebuild the fixture only if the corpus commit under test has to change. That invalidates every
recorded run against it, which is the reason it is pinned.

## Before anything: the corpus is now hostile to two of the three tools

`beauty-crm` keeps `graphify-out/` (tracked) and `.gitnexus/` (git-ignored) on disk
**and runs neither tool**. That is a standing decision recorded in that repository's
`.claude/CLAUDE.md` and its `repo-query` skill, and re-running this comparison means
deliberately suspending it for the length of the run.

Do that in a scratch worktree, not in the working clone:

```bash
cd ~/Documents/projects/beauty-crm
git worktree add /tmp/bench-corpus <commit>
```

A worktree keeps the tools' writes — indexes, caches, skill files, rewritten
`CLAUDE.md` — off the clone someone is working in, and `git worktree remove` is the
whole of the cleanup. If the run happens in the clone instead, the post-run
checklist at the bottom is not optional.

## Pre-flight

- [ ] **The corpus is at a known commit with a clean tree.** Record the SHA; every
      number below is only comparable to another run through it.
      `git rev-parse --short HEAD && git status --porcelain`
- [ ] **`--strip-prefix` is needed if a graph was built in a worktree.** graphify
      writes absolute paths; the runner needs to know which directory to remove.
- [ ] **Each tool's version is recorded**, not assumed: `zerograph --version`,
      `gitnexus --version`, graphify's own. The result header carries them and two
      result files are comparable only when those agree or the difference is the
      point of the run.
- [ ] **Disk.** gitnexus is ~600 MB per corpus, graphify ~160 MB. Both stay after
      the run unless removed.

## Build each index

How each index got fresh is part of the result, so write it into the `notes` block
of the result file exactly as it was run.

```bash
# zerograph — ~1 s lexical, ~100 s for the vectors on first build
zerograph --repo . build

# graphify — AST layer off the semantic cache, 0 tokens
graphify update <path> --force

# gitnexus — 236 s, ~598 MB. --skip-agents-md is NOT optional, see traps below
gitnexus analyze -f --embeddings --skip-agents-md .
```

`zerograph enrich` (≈$2.50 on haiku, once) and `ask --rerank` (≈19k input tokens per
question on sonnet) are opt-in and change the numbers. If either is on, say so in
the notes — the 2026-09-03 run read paraphrase 16/30 with an opus `enrich` where the
shipped floor for an enriched store is 14/30, and a reader who does not know that
will read a regression into the next run. The build above runs neither, so its store
is graded on the raw floors instead — 9/30 with embeddings, 7/30 with `--no-dense` —
and `bench`'s summary line prints which of the two it used.

Two more switches change the numbers and belong in the notes the same way: `enrich --code`
adds questions about code (290 batches more on this corpus; the summary line then carries
`code_questions=`), and `ZEROGRAPH_EMBED_MODEL=<hub id>` with `zerograph embed` measures a
store under another embedder — on a copy of the store, never on the fixture.

## Run both suites

```bash
cd ~/Documents/projects/zerograph/bench/compare
python3 run.py \
  --repo /tmp/bench-corpus \
  --zerograph ~/Documents/projects/zerograph/target/release/zerograph \
  --graphify-graph /tmp/bench-corpus/graphify-out/graph.json \
  --truth /tmp/truth.json \
  --out ../results/$(date +%F)-beauty-crm.json
```

Useful flags: `--tools zerograph` for one tool, `--suites blast` to skip the slow
retrieval pass, `--strip-prefix <dir>` for a worktree-built graphify graph,
`--truth <path>` to reuse a truth file rather than rebuild it.

**Rebuild the truth file when the corpus commit moves.** `truth.py` reads the
expectations out of the repository with ripgrep, so a reused truth file from an older
commit silently scores the new corpus against the old one. Reuse it only inside one
run, across tools.

## The seven traps, in the order they bit

1. **`gitnexus analyze` writes six skill directories into `.claude/skills/gitnexus/`
   without being asked for `--skills`**, whose own help says that flag generates them.
   `beauty-crm`'s `.gitignore` un-ignores `.claude/skills/`, so they arrive as
   untracked files a commit would take, and each one instructs an agent to query a
   graph the repository does not read. `rm -rf .claude/skills/gitnexus` after every
   rebuild.
2. **`--skip-agents-md` or it rewrites `CLAUDE.md` and `AGENTS.md`** unasked. Check
   `git status --porcelain` immediately after the analyze; a clean tree is the proof
   the flag worked.
3. **`~/.gitnexus/registry.json` drops a repository whose storage path has vanished.**
   Deleting `.gitnexus/` removes the entry; `registry.json.bak` is where the previous
   entry survives, and a re-analyze writes a fresh one. Compare the `stats` block
   across runs — nodes and edges should move with the corpus, not with the weather.
4. **graphify's `affected` refuses an ambiguous name** — `No unique node match` for
   `DatabaseService`, `TenantContextInterceptor`, `ActorResolver`, `OutboxPublisher`.
   That is scored as a miss, not skipped: a blast radius nobody can obtain is a blast
   radius of nothing. Do not "fix" it by disambiguating the case, or the suite stops
   measuring the tool as an agent would meet it.
5. **gitnexus's `detect-changes` CLI prints fifteen symbols then `... and N more`.**
   The MCP payload carries more. The table reports the CLI number; if the difference
   matters to a conclusion, read the payload by hand and quote it in the notes, as
   the 2026-09-03 run did for base `cbc931ba~1`.
6. **Latency is whole-process wall clock.** gitnexus has an `eval-server` that skips
   start-up; it was not used, because the other two have no equivalent. Turning it on
   for one tool makes the latency column meaningless.
7. **An `ask` against a store copy that has no source tree beside it empties the graph.**
   Every command that brings the store in line with the tree first — `ask`, `update`, `watch`,
   `impact`, `trace`, `changes`, all of which take `--stale` — refreshes against what it finds on
   disk, finds nothing, and removes every node. On 2026-09-05 one latency `ask` emptied a store
   copy's graph, restored afterwards from the fixture's `graph.bin` and `graph.json`. Measure on a
   copy that has the corpus beside it, or pass `--stale`; `bench` and `dump` read the store as it
   stands and cannot cause this, which is why the damage surfaces one command later as
   `graph is empty at <path> — run build first`. A store-only copy stays safe only while its
   `manifest.json` is empty: a manifest naming files that are not there is what triggers the
   refresh, so copying the fixture's manifest into such a copy re-arms the trap.

## Judging a retrieval change: the three-way rule

A lever is written down before it is measured, and read against all three of these — the four
hundred held-out questions first, the 82 recorded cases second, because reading the 82 first is how
a change gets fitted to the smoke test without anyone intending it:

1. **Every recorded floor holds in all four arms**, p90 ≤ 250: `bench` and `bench --no-dense`, on
   the enriched store and on a raw one.
2. **Held-out recall@5 is not significantly worse in either arm.** `bench/heldout.py`'s header is
   the order of operations — build the set, dump it on the old binary and on the new one, in both
   arms, then `compare`. The set the 2026-09-05 levers were read against is `heldout-400-syn.jsonl`:
   400 questions of kind `synthetic`, which is the kind `dump` applies leave-one-out to. A set
   written under any other kind holds nothing out and reads about 0.95 whatever the change was —
   that happened once, and G7 records it. The test is a paired exact McNemar, so what counts is the
   questions that changed answer and in which direction, not the two totals.
3. **The developer suite is not down in either arm**: `bench --cases bench/dev-cases.jsonl`, both
   arms. It is ungated by design, so this one is read rather than enforced by an exit code. A lever
   aimed at one kind of question says before the run what that kind has to reach — the 2026-09-05
   code levers had to put `where` above 0/9 as well as leave the rest standing.

The rule is not re-read once the numbers are in. Six levers were measured against it on 2026-09-05
and one passed; [the results](2026-09-05-dev-cases-results.md) say which, and what the other five
cost.

## Timing an ask

**A bar its own control cannot pass is measuring the machine.** Before a wall or a footprint is
used to judge a change, run the same command through the same binary twice and see whether the two
readings sit inside the bar you were about to apply. They often do not: the reader suite's own
control read `ask-fused` at 0.61–0.75 s on one binary and 0.60–0.65 s on the other against a 10%
bar, and max RSS bounced between 1.36 and 1.56 GB on **both** against a 5% one. Three cheap habits
follow, and every timing row below assumes them:

- **n runs and a median**, never one reading. `bench --repeat 3` does this for the suites and
  prints the median beneath the runs; for a single command, run it three times and take the middle.
- **A pinned index.** Reader rows are taken against a store no writer in the session has touched —
  the fixture's index grew from 33,526 to 33,554 rows *during* one session's own `watch` and
  `update` measurements, which is enough to move a row and nothing in the numbers says so.
- **A stated machine.** Write down the load the reading was taken under, the way every probe row
  states the idle baseline it is read against. A laptop that sleeps mid-run is the extreme case and
  it happens: one whole-store embed read 6,332 s of wall against 184.7 s of its own sync time, and
  only the process's own progress lines could tell the difference.

**A performance change is judged on the dumps first.** It ships only if `dump` of both suites in
both arms is byte-identical to the baseline and `bench` prints the same lines; a change that moves
an answer is not a performance change, whatever the clock says. Take the baseline before the first
edit, on the same fixture, and keep the four files:

```bash
S=~/bench/<scratch>                       # transcripts live outside the repo
for arm in dense lexical; do
  nd=""; [ $arm = lexical ] && nd="--no-dense"
  zerograph --repo ~/bench/beauty-crm-502e8a6d $nd dump --queries bench/cases.jsonl     --out $S/base-rec-$arm.json
  zerograph --repo ~/bench/beauty-crm-502e8a6d $nd dump --queries bench/dev-cases.jsonl --out $S/base-dev-$arm.json
done
shasum $S/base-*.json > $S/base-shasums.txt
```

Afterwards, the same four with a different tag, then `cmp` each pair and `bench` in all four arms.
`dump` and `bench` read the store as it stands, so both are safe against the pinned fixture —
unlike `serve`, which refreshes and writes and is measured on a copy.

Note what those dumps cannot see: they build their own dense closure and call `query::ask`
directly, so **the `ask` command's own arm is never executed by Rule 1**. A change to that arm is
compared by running the ask shapes themselves against a binary built before the change, stdout and
stderr separately — see [the perf results](2026-09-06-perf-results.md).

Only then, the clock. `perf-time.sh` — five `ZEROGRAPH_TIMING=1` runs per arm, medians per stage:

```bash
#!/bin/bash
# usage: perf-time.sh <binary> <repo> <label>   — five ZEROGRAPH_TIMING runs per arm, medians printed
B=$1; R=$2; L=$3; S=~/bench/<scratch>
for arm in dense lexical; do
  nd=""; [ $arm = lexical ] && nd="--no-dense"
  : > $S/$L-$arm.txt
  for i in 1 2 3 4 5; do
    (cd $R && ZEROGRAPH_TIMING=1 $B ask --stale $nd 'штраф за отмену записи' 2>&1 >/dev/null | grep '^timing' >> $S/$L-$arm.txt)
  done
  echo "== $arm =="
  python3 -P - "$S/$L-$arm.txt" <<'PY'
import re, sys, statistics
rows = {}
for line in open(sys.argv[1]):
    m = re.match(r'timing:\s+([\d.]+) ms\s+\(\+\s*([\d.]+) ms\)\s+(.*)', line)
    if m: rows.setdefault(m.group(3), []).append((float(m.group(1)), float(m.group(2))))
for stage, v in rows.items():
    print(f"{stage:32} total median {statistics.median(x for x,_ in v):8.1f} ms   step median {statistics.median(y for _,y in v):7.1f} ms   n={len(v)}")
PY
done
```

Three things the 2026-09-06 run learned the hard way:

- **Between-sitting drift on this machine exceeds the effects being measured**, so a before/after
  pair has to be **interleaved in one sitting** — A, B, A, B, … — never five of one and then five
  of the other, and never a fresh set against a median stored earlier in the day. The same binary
  read 369.8 ms in one sitting and 346.1 ms an hour later; two five-run sets of identical code sat
  44.9 ms apart, wider than the lever they were being used to judge. Build every arm's binary
  first, then measure them round-robin, and write the machine's load into the transcript.
- **`git archive` hands cargo a stale mtime, and cargo hands back the wrong binary.** Building each
  arm from `git archive <commit>` into a shared `--target-dir` stamps the *commit's* mtimes, which
  are older than the artifacts the previous arm left in that directory. Cargo calls the build fresh
  and copies the earlier commit's binary out under the later commit's name. The first round of the
  2026-09-06 builds produced four binaries with **the same sha** — one binary, which would have
  been measured four times and published as a before/after table with nothing in it. `touch` the
  extracted tree before building (`find $SRC -type f -exec touch {} +`), then `shasum` every binary
  and check the four differ: that comparison is the only thing that catches it.
- **`pkill -f 'zerograph serve'` matches nothing**, because the command line reads
  `zerograph --repo <path> serve`. Match on the repo instead (`pkill -f '<repo-dir> serve'`). A
  whole timing pair was lost to orphan servers that a `pkill` was believed to have killed: every
  later `serve` refused to bind with `another serve answers`, and the "before" client was measured
  against an "after" server. Before measuring a socket, start the server yourself, wait for its own
  bind line in its own log, and refuse to send a request without it.

## What to write down

The result file's header carries the corpus path, commit, timestamp and case counts
by itself. The `notes` block does not write itself, and it is what makes the file
readable in six months. The 2026-09-03 file's notes are the model: one line per tool
saying how its index was built and how big it came out, plus a line for every
judgement call the run made — the gitnexus CLI cap, graphify's refusals, and exactly
what the truth file counts.

Then write the prose beside it, as
[`2026-09-03-three-graphs-results.md`](2026-09-03-three-graphs-results.md) does: the
tables, what each shape means, what was decided, and the caveats. A result file
nobody read is a run nobody made.

## Measuring paraphrase: use the widened arm, not the recorded one

The suite in `bench/cases.jsonl` carries 30 paraphrase cases, and the best stack this
project has already reads 29 of them. One hit is the whole remaining headroom, so that
arm can no longer tell an improvement from a run-to-run wobble, and the four-way tie
between embedders it reported in
[the embedder readings](2026-09-22-embedders-results.md) was its ceiling rather than
theirs.

A widened arm of 60 lives in the repository: `bench/cases-wide-112.jsonl` is the 82
recorded cases plus 30 paraphrase ones, and `bench/paraphrase-gate.py` is what admitted
them. The 2026-09-22 kit's texts were lost; these were written again on 2026-10-18 for
the same target ids ([readings](2026-10-18-widened-arm-q4.md)). Run it with `--cases`,
against the writable store and never the pinned fixture:

```bash
zerograph --repo ~/bench/beauty-crm-test bench --cases bench/cases-wide-112.jsonl
```

A new case passes `python3 bench/paraphrase-gate.py <graph.json> <cases.jsonl>`, which
needs `snowballstemmer`, before it joins the file.

Two things to know before quoting a number off it. `RECORDED_SHAPE` in `src/bench.rs`
names 40/30/12, so a 112-case file is measured and **not graded** — the summary line
says `gated=false` and no floor is applied. And a new case is admitted by the gate, not
by judgement: at most 0.34 of the question's content stems may also occur in the target
node's label and body, the anchor must resolve, and no second node may answer it. A
question that reuses the document's vocabulary is a keyword case in a longer sentence,
and an arm of those reads high for a retriever that does nothing but match tokens.

Write the cells into `bench/history/runs.jsonl` like any other run, with the arm named
so the suite is obvious (`paraphrase60b:<model>[+rerank]` for the current texts;
`paraphrase60` rows read the lost ones and do not compare).

## Cutting a release

Eight steps, in order. Each one exists because skipping it once cost a release run.

1. **Prove the binaries first.**
   ```bash
   gh workflow run release.yml --ref <release branch>
   ```
   A dispatch from a branch builds, smokes and packs all three targets and publishes nothing, so
   the three `build` jobs are green before the PR merges, not after the tag.

2. **Bump in one PR.** Change `Cargo.toml`'s `version`, let `cargo build` update `Cargo.lock`, and
   replace every old version string under `npm/` and in the README status line:
   ```bash
   grep -rn '"<old>"' npm/
   ```
   One PR keeps the crate, the lockfile and the npm launcher naming the same version at every commit.

3. **Squash with a written message.**
   ```bash
   gh pr merge <n> --squash --subject "chore(release): <version>" --body "<one paragraph>"
   ```
   The default body concatenates every branch commit's message, and a docs commit's `[skip ci]` in it
   silences the tag's run.

4. **Check the commit the tag goes on.**
   ```bash
   git fetch origin main && SHA=$(git rev-parse origin/main) && git log -1 --format=%B $SHA | grep -iE '\[(skip ci|ci skip|no ci|skip actions|actions skip)\]'
   ```
   A match means the tag's push will start no run, and step 7 has to dispatch one.

5. **Once: the first npm version by hand.** npm's trusted publishing, which the release job uses,
   can only be set up on a package that exists. Before the first tag, `npm login` as the owner and
   publish an empty `zerograph@0.0.0` from a temporary directory:
   ```bash
   scripts/npm-placeholder.sh <code>   # the authenticator's current code; omit it in a terminal and npm asks
   ```
   Never `npm publish` inside `npm/zerograph/`: until the release job fills `bin/`, that publishes
   the real version number with no binary in it, and a version is never given back.
   Then on npmjs.com, the package's Settings → Trusted publishing: GitHub Actions, repository
   `devmaxxx/zerograph`, workflow `release.yml`. Once set up, this step is skipped.

6. **Tag and push.**
   ```bash
   git tag v<version> $SHA && gh auth switch --hostname github.com --user devmaxxx && git push origin v<version>
   ```
   The tag goes on the commit step 4 read, and it is pushed from the account that owns the repository.

7. **Exactly one publishing run.** If step 4 matched, dispatch at once:
   ```bash
   gh workflow run release.yml --ref v<version>
   ```
   If it did not, `gh run list --workflow release --branch v<version>` shows a run within two
   minutes, or dispatch then. A dispatch from the tag publishes because the publish steps test
   `startsWith(github.ref, 'refs/tags/')`. Two runs would race to publish the same assets.

8. **Verify.**
   ```bash
   gh api repos/devmaxxx/zerograph/releases/latest --jq .tag_name
   npm view zerograph@<version> version
   ```
   The first prints the tag, the release carries three assets, and the second prints the version: a
   release is done when someone else can install it, not when the tag exists.

## Post-run cleanup, if the run was not in a worktree

- [ ] `rm -rf .claude/skills/gitnexus`
- [ ] `git status --porcelain` clean — nothing tracked was rewritten
- [ ] the ignore entries still cover both indexes: `pnpm lint` and
      `pnpm format:check` green with ~750 MB of machine-written JSON on disk
- [ ] `beauty-crm`'s standing decision restated where it was suspended: neither tool
      is run against that tree outside a measurement
