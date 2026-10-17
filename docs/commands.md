# Commands

## Status

0.6.4 is the version `main` carries. Its default globs read TypeScript and JavaScript, Kotlin, Java,
C#, Rust, Python, Dart, Swift, GraphQL, SQL, Bicep, HCL, Shell, Vue, Lua, Go, C, C++, PHP, Ruby,
Scala, Elixir and OCaml; every command below is
implemented rather than planned:
`build` (a full re-read that replaces the stored graph only when it saves, so one interrupted
leaves the previous store answering) and `update` (incremental; a no-op `update` is a fixed point), `families`, `ask`, `explain`,
`verify`, `impact`, `trace`, `changes`, `embed`, `watch`, `serve` (and `stop`), `mcp`, `prime`, `install-agent`,
`import-legacy`, `dump` and `bench`. Two rerank the fused pool and both are opt-in: `ask --rerank`,
which spends model tokens, and `ask --rerank-local` (zero tokens, a local cross-encoder, measured and
rejected as a floor candidate). `bench` fails the process when a floor in [Bench](benchmarks.md#bench) is missed;
floors are keyed by the store's embedder, and a store under any other model is measured and not
graded. `enrich` was removed in 0.6.0: an older store's `questions.json` is no longer read.

`--no-dense` skips the embedding stage everywhere it could apply — `build`, `update`,
`embed`, `watch`, `serve`, `ask`, `bench`, `dump`. Without it, those commands use local embeddings
once the model is cached (see [Embeddings](embeddings.md)).

## Use

Build the graph once, then keep it fresh incrementally:

```bash
zerograph --repo /path/to/project build     # full build
zerograph --repo /path/to/project update    # re-extract only changed files
```

`--repo` defaults to the current directory. State lives in `<repo>/.zerograph/`; add it to
`.gitignore`.

After a first build, read which id families the documents defined and what was left as text:

```bash
zerograph families                          # every family, its nodes, and the line that defines it
zerograph families --json                   # the same numbers for a script
```

Ask it something:

```bash
zerograph ask cancellation policy
zerograph ask FR-PAY-22                     # an exact id short-circuits straight to the node
zerograph ask asGrosze                      # so does an exact symbol name (a plain lowercase word does so only alone)
zerograph ask "FR-PAY-22 refund"            # quoted or not, the same words find the same ids and names
zerograph ask --bodies отмена записи        # print full requirement bodies, not just headlines
zerograph ask --json отмена записи          # machine-readable
zerograph ask --seeds 8 отмена записи       # widen the search beyond the default of 5; costs more tokens
zerograph ask --no-dense отмена записи      # lexical only, no embedding query
```

Ask it what depends on a symbol, what a symbol reaches, and how one reaches another:

```bash
zerograph impact StaffService               # callers by depth, importing files, a risk line
zerograph impact --down StaffController     # what it calls, through injected services and barrels
zerograph impact --json --depth 1 asGrosze  # machine-readable; depth 1 is the "will break" list alone
zerograph trace StaffController StaffService  # shortest chain of calls between two symbols
zerograph changes                           # what the uncommitted diff touches, and who reaches it
zerograph changes --base main --depth 1     # the whole branch; depth 1 is the direct callers alone
```

`impact` and `trace` resolve a name the way `explain` does, except that when a document node and
code share it they take the code, production code before a test (a `test/` or `e2e/` path, a
`*.test.*`, `*.spec.*` or `*.stories.*` file), and name on stderr three of the candidates they
passed over and how many more there are.

Answers are lines of the form:

```
path:line  [ID]  headline
  └ path:line  [ID]  headline
```

Top-level lines are the seeds the query matched, location first so a terminal can open it. A
document id (`BE-M10/T05`) is printed; a code, file or entity id is not, because the location and
the name already say it. A `└` line is a neighbour reached by one hop over the id graph, printed
under the seed it came from. A real run, against `beauty-crm`:

```
$ zerograph --repo beauty-crm ask cancellation
docs/prd-2026-08-16/plans/milestones/backend/BE-M05-slot-engine-scheduling-api-appointments-conflicts-requests-t.md:270  BE-M05/T15  `BookingRules` + `CancellationPolicy` CRUD, policy **snapshot** attached to the …
packages/db/src/schema/salon/scheduling.ts:110  CANCELLATION_CONSEQUENCES
docs/prd-2026-08-16/plans/milestones/backend/BE-M10-payments-provider-stripe-connect-implementation-deposits-car.md:80  BE-M10/T05  Cancellation policy engine (`packages/domain/policy.ts`): inputs policy + appoin…
packages/db/migrations/0008_first_slice.sql:808  salon_ctx/cancellation_policy  cancellation_policy
  └ packages/db/sql/0002_rls.sql:135  salon_ctx/cancellation_policy  cancellation_policy
```

### Answering a program instead of a person

Every reader takes `--json`, and every one of them answers with an object rather than a bare array,
so a field can be added without breaking a parser written against the version before it:

| Command | Top-level keys |
| --- | --- |
| `ask --json` | `seeds`, `expanded` |
| `impact --json` | `root`, `at`, `direction`, `risk`, `direct`, `total`, `files`, `importers`, `layers` |
| `changes --json` | `risk`, `touched`, `affected`, `files` |
| `families --json` | `families`, `milestones`, `mention_only` |
| `explain --json` | `id`, `kind`, `label`, `file`, `line`, `community`, `edges` |
| `verify --json` | `nodes`, `edges`, `nodes_by_kind`, `edges_by_kind`, `dangling`, `undeclared`, `gaps`, `cite_only`, `held_aside`, `held_aside_prefixes` |
| `trace --json` | `from`, `to`, `depth`, `path` |
| `prime --json` | `nodes`, `edges`, `families`, `model` |
| `model --json` | `store`, `configured`, `configured_from`, `this_run`, `agrees`, `recommended` |

`explain --json` resolves each edge's direction for you — `dir` is `in` or `out` and `other` is the
node at the far end — so a caller never works out which end of an edge it was standing on. `file`
and `line` say where the edge is written, as the text form's trailing `path:line` does, so two
edges from one id that two documents declare read as two: the line is that of the declaration the
edge sits in, and `null` when no node declared in that file is an end of it. Each
row of `impact` and `changes`, and each step of a `trace` path, carries `passes`: `true` when the
edge is an identifier handed to a call rather than called, which the text forms print as `Passes`.
`kind` stays the graph's own edge kind, `Calls`, so a reader filtering on it keeps those rows. One
difference from the text forms is deliberate: a `trace` that finds no path within the depth is an
answer to the question that was asked, so the JSON form prints `"path": null` and exits 0 where the
text form exits **3**. The two are the same answer in two shapes: a caller parsing an object should
not have to read an exit code to learn what the object already says, and a shell script reading the
text form should not have to tell that answer from the `1` an unknown symbol exits with. A command
given `--json` that fails once it is running prints `{"error": "…", "code": 1}` on stdout instead of text on stderr,
with the same exit status, so a caller parsing stdout learns why it got no answer. A usage error is still clap's text on
stderr with exit 2: it fails before any command runs.

Inspect one node and everything attached to it:

```bash
zerograph explain <id|symbol|label>         # tries an exact id, then a symbol name, then a label
zerograph explain FR-PAY-22
zerograph explain asGrosze
```

`explain` resolves its argument as an id — as typed, then without the punctuation around it, then
ignoring case, as `ask` reads an id in a question — then as a symbol name, then as a
case-insensitive label match. Like every other reader it brings the store in line with the tree first; `--stale`
answers from the store as it stands.

Check the graph's health — counts by kind, dangling edges, how many citations are held aside because
no line defines their prefix, and ids that are referenced but never declared, split into gaps inside
a declared family (worth chasing) and shapes the dialect does not read as ids at all:

```bash
zerograph verify
```

Fold in a graphify graph's model-only edges as a frozen legacy layer:

```bash
zerograph import-legacy path/to/graphify/graph.json
```

This prints how many of the source graph's edges resolved to real nodes on both ends, on one end, or
needed a new `LegacyConcept`. It is opt-in and lossy by construction — see its coverage note under
[Bench](benchmarks.md#bench) before running it on a store you plan to benchmark.

Run the recorded benchmark:

```bash
zerograph bench                      # bench/cases.jsonl: 40 keyword + 30 paraphrase + 12 code
zerograph bench --cases other.jsonl  # any shape: the 40/30/12 shape is graded, any other is measured ungated
zerograph dump --queries qs.jsonl --out lists.json   # every retriever's ranked list per question, 300 deep
```
