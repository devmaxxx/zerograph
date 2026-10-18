# Configuration

`zerograph.toml` at the repository root — this file also doubles as the worked example, set to its
own defaults. Every key is optional; a repository with no `zerograph.toml` gets `Config::default()`
in full, not an empty config:

| Key                  | Default                                                                                     |
| -------------------- | ------------------------------------------------------------------------------------------- |
| `doc_globs`          | `["**/*.md"]`                                                                               |
| `code_globs`         | `["**/*.ts", "**/*.tsx", "**/*.js", "**/*.jsx", "**/*.mjs", "**/*.cjs", "**/*.kt", "**/*.java", "**/*.cs", "**/*.rs", "**/*.py", "**/*.dart", "**/*.swift", "**/*.gql", "**/*.graphql", "**/*.sql", "**/*.bicep", "**/*.tf", "**/*.hcl", "**/*.sh", "**/*.bash", "**/*.vue", "**/*.lua", "**/*.go", "**/*.c", "**/*.h", "**/*.cc", "**/*.cpp", "**/*.cxx", "**/*.c++", "**/*.ino", "**/*.hh", "**/*.hpp", "**/*.hxx", "**/*.h++", "**/*.php", "**/*.rb", "**/*.rbw", "**/*.gemspec", "**/*.rake", "**/*.ex", "**/*.exs", "**/*.scala", "**/*.sc", "**/*.ml", "**/*.mli"]` — every language in [Languages](languages.md) |
| `text_globs`         | `[]` — files no doc or code glob claims, each read as one text node when its content is text  |
| `skip`               | `["**/node_modules/**", "**/dist/**", "**/*.min.js", "**/.yarn/**", "**/.pnp.*", "**/target/**", "**/.venv/**", "**/venv/**", "**/__pycache__/**", "**/.terraform/**", "**/.dart_tool/**", "**/Pods/**", "**/vendor/**", "**/_build/**", "**/TRACKER.md", "graphify-out/**", ".zerograph/**", ".repograph/**"]` |
| `include`            | `[]` — the whole repository; set, only these directories (`src`, `docs/specs`) or globs are read |
| `registries`         | `["docs/constitution.yaml"]`                                                                |
| `rerank_command`     | **machine file only** — headless `claude -p --model {model}` with thinking off, see [Spending tokens on purpose](rerank.md) |
| `rerank_model`       | `haiku` — whatever goes in `rerank_command`'s `{model}`                                     |
| `reranker_dir`       | directory of the exported cross-encoder for `--rerank-local`; empty = `~/.cache/zerograph/reranker`. Machine file only: a repository's is ignored, like the commands |
| `embed_model`        | `onnx-community/embeddinggemma-300m-ONNX:q4`; the model the vectors are written with — nine were measured and `zerograph model` switches it, see [Embeddings](embeddings.md) |
| `embed_skip`         | `[]` — globs whose files get no vectors but keep their nodes, edges and lexical rows, see [Files without vectors](configuration.md#files-without-vectors) |
| `mcp_reranks`        | `25` — rerank commands one MCP server process starts (one that fails or times out counts, an exact id that never asks the model does not) before it answers `rerank: true` from the fused order with a line saying so; `0` turns rerank over MCP off. Machine file only: the agent decides each call and the tokens are the reader's, see [MCP](mcp.md) |
| `vector_cache_max`   | `"4G"` — bytes the machine's vector cache may hold before the least recently used rows go; a whole number of bytes or `K`/`M`/`G`/`T`; `0` turns the cache off. Machine file only: the cache is every checkout's, see [The vector cache](keeping-fresh.md#the-vector-cache). `ZEROGRAPH_VECTOR_CACHE` moves the cache to another absolute directory |
| `reader_budget`      | `10` — seconds a reader may spend refreshing before it answers from the store as it stands and leaves the rest to a detached `update`; `0` never refreshes inline. The machine file may set it; `ZEROGRAPH_READER_BUDGET` overrides, see [Keeping it fresh](keeping-fresh.md#keeping-it-fresh) |
| `resources`          | `"balanced"` = 40% of the logical cores; `"low"` a sixth, `"full"` a half — how much of the machine a run may take, see [Resources](configuration.md#resources) |

`ZEROGRAPH_CODE_GLOBS`, whitespace-separated, replaces `code_globs` for one run. It is a measurement's
switch, as `ZEROGRAPH_EMBED_MODEL` is, so a reading can name other globs without writing a
`zerograph.toml` into the tree it measures.

`include` narrows the walk to what a project lets a reader see: `include = ["src", "docs"]` reads
nothing outside those two directories, whatever the other globs claim, and the next `update` drops
what an earlier store held outside them. Credential files are never read under any setting: `.env`
and `.env.*` (the templates `.env.example`, `.sample`, `.template` and `.dist` excepted), private
keys and keystores, `.npmrc`, `.pypirc`, `.netrc`, `.git-credentials`, cloud credentials,
`*.tfvars`, `*.tfstate` and `secrets.{yaml,json,toml,…}` — their content would otherwise reach the store, a
`--rerank` prompt sent to a model, and the answers.

The store writes its own `.gitignore` holding `*`, so `.zerograph/` stays out of a commit in a
repository that never listed it.

A directory below the root that holds a `.git` of its own — a worktree such as
`.claude/worktrees/*`, a clone, or a git submodule — is another checkout and is never walked,
whether or not an ignore file names it. Submodules therefore leave the index too: build one with
`--repo` pointed at it if its files should be answered from.

`text_globs` (empty by default) takes in every file no doc or code glob claims whose content is
text — configuration, YAML, JSON, data — as one node each, searchable by `ask` and named by
`changes`, with nothing parsed below it. A file is binary when its first 8,000 bytes hold a NUL.
Lockfiles, `*.min.*`, `*.map`, `vendor/` and text over 1 MiB are left out; the build says how
many were over the size. `ZEROGRAPH_TEXT_GLOBS`, whitespace-separated, replaces the list for one run,
and `ZEROGRAPH_TEXT_GLOBS=-` turns it off for one (`-` does the same for `ZEROGRAPH_CODE_GLOBS`).
`ask` points a text file's line at the one holding most of the question's words, and `--bodies`
prints the twelve lines around it rather than the whole file. A store holding text nodes is read by
0.6.0 and later only: 0.5.x stops on `graph.json: unknown variant Text`, and `build` with that
binary rebuilds a store it can read.
Text nodes ride a list of their own in the fusion, seated only when a configuration file covers the
question's words as completely as a requirement does — the admission `enrich`'s generated
questions were seated under until 0.6.0. It stays empty by default because text still costs a little `ask` recall: on the bench
fixture under the default embedder, `text_globs = ["**/*"]` takes paraphrase questions from 20 to 19
of 30 and the dev suite from 36 to 33 of 60 (one shared list cost 18 and 30), while twelve
config-file questions read 12/12. Turn it on where finding a configuration file matters more than
finding the requirement; the readings are in [the text-list rule](bench/2026-10-08-text-list-rule.md).

### Files without vectors

`embed_skip` (empty by default) names files the dense index leaves out: generated code, SQL
migrations, mocks — what is read and changed but never asked about in prose.

```toml
embed_skip = ["**/generated/**", "**/*.gen.ts", "**/migrations/**", "**/*.mock.ts"]
```

A matching file keeps its nodes, its edges and its rows in the lexical index, so `impact`, `trace`,
`changes`, `explain` and an `ask` by name or exact words still find it; a paraphrase does not,
because nothing of it is embedded. `skip` would take it out of all of those. The globs match the
path a node is declared in, the way `skip` matches a file, and a requirement a skipped file only
mentions keeps the rows its own document gives it. Its rows are not counted as owed, so the MCP
`status` tool does not report them, and the next writer — `update`, `embed`, `build`, or a
reader catching up a moved graph — drops any it already held. Changing the key moves only those
files' rows: every other row is matched by hash and kept, so adding a glob embeds nothing and
removing one embeds that file's rows alone. A glob that does not parse is an error naming the file,
and the machine file refuses the key, since which files get vectors is one repository's choice.

On beauty-crm's 55.8k embedded rows, generated files were 10.5% and SQL migrations 8.2%.

### Choosing a model, and where the choice lives

`rerank_command` is the transport — any program that reads a prompt on stdin — and which model
that program runs is a separate key, substituted into the command's `{model}`.
Changing model is a word rather than a rewritten command line; a command naming no `{model}` runs
exactly as written. Nothing here assumes a vendor. It runs under `sh -c`; on Windows that is Git
for Windows' `sh`, found on `PATH`, beside `git`, at `CLAUDE_CODE_GIT_BASH_PATH` or under Program
Files, with Git's `usr\bin` put on the command's own `PATH`. Without Git for Windows,
`ask --rerank` refuses with a line that says so and everything else runs.

Which model to run is usually a property of the machine — what is installed, what the account may
spend — rather than of the corpus, so it can be set once for every repository. Three layers, each
beating the one below it:

| Layer | Where |
| --- | --- |
| the run | `ZEROGRAPH_RERANK_MODEL`, `ZEROGRAPH_RESOURCES` |
| the repository | `zerograph.toml` |
| the machine | `$ZEROGRAPH_CONFIG`, else `$XDG_CONFIG_HOME/zerograph/config.toml`, else `~/.config/zerograph/config.toml` (`%USERPROFILE%\.config\zerograph\config.toml` on Windows) |

```toml
# ~/.config/zerograph/config.toml — every repository on this machine, unless it says otherwise
rerank_model = "sonnet"
```

Each layer still answers to its name from before the rename — `REPOGRAPH_*`, `repograph.toml`,
`~/.config/repograph/` — until the new name is there; [coming from repograph](install.md#coming-from-repograph)
has the exact rules.

A key the repository names wins even when it names the built-in value — with one exception, and it
runs the other way. **`rerank_command` is read from the machine file and never from a
repository.** A repository you cloned is untrusted input, and that key is a shell command that
would run on your machine the first time you ran `ask --rerank` in it; a `zerograph.toml` that
names one gets a line on stderr saying where the key belongs. If the machine file names its own
command, that one runs. If not, `bench --rerank` stops
instead of falling through to the built-in paid model, and `ask --rerank` answers from the fused
order: a repository naming its own transport did not ask for the default one. A repository may still say which *model* it wants,
and because that name lands in a shell string, anything outside letters, digits and `._:/@+-` is
refused the same way. What a clone chooses is the model; what runs it is yours.

The machine file may set **only** `rerank_command`, `rerank_model`, `reranker_dir` and
`resources`, and refuses any other key by name: the
corpus-shaped keys describe one repository's documents, and a global `embed_model` would rewrite
every store's vectors under a model nobody chose for it.

`threads` and `priority` were removed on 2026-09-09. Replace `threads = 6` with
`resources = "full"` and `threads = 2` with `resources = "low"`; delete `priority`, there is no
scheduling band any more. Either key left in a config file is a hard error naming the file and the
key, not a setting quietly ignored — both structs are `deny_unknown_fields`, and a key that parsed
and did nothing would leave you believing a number you wrote is still read.

`enrich_command`, `enrich_model` and `enrich_languages` went with `enrich` in 0.6.0. They still
parse, so a config written for 0.5.x keeps working, and each one present prints `zerograph.toml:
<key> is no longer read — enrich was removed in 0.6.0` (`config.toml:` from the machine file);
delete them.

Two of those keys name a model and one names a directory, and they are three different jobs.
`rerank_model` picks five ids out of a 200-deep pool (`haiku`: shown the sentence of each
candidate that matches the question it reads what sonnet reads, faster —
[ADR-004](adr/ADR-004-haiku-reranks-by-default.md)); `embed_model` is the store's own
and is weighed in [ADR-002](adr/ADR-002-two-defaults-multiplied.md); `reranker_dir` is the
local cross-encoder, measured and rejected as a floor candidate. Every one of those readings, with
what each stage asks of a model, is in
[the measurements](history.md#what-each-stage-asks-of-a-model-and-every-answer-measured).

**The contract a command has to meet** is names no vendor. `sh -c`
runs it, the prompt arrives whole on stdin, the answer is read from stdout, and `{model}` anywhere
in the command is replaced by `rerank_model`. The parser keeps only what it recognises — ids `--rerank` actually
showed, in the model's order — so a chattier command is
survivable rather than fatal, and a failing one is answered with a notice and the fused order. A
command that answers without reading its prompt to the end is fine too: the broken pipe the write
hits is ignored on purpose. The default runs headless Claude Code with thinking off: the same
answers, 4–5× faster.

**Another vendor is a different command line, not a different zerograph.** Only the first of these
is what this repository runs; the second is verified against that CLI's `--help` on this machine and
the rest are shapes. All of them go in the **machine** file — `~/.config/zerograph/config.toml` —
because a command a cloned repository names is a command it runs on your machine:

```toml
# ~/.config/zerograph/config.toml
# The shipped default: headless Claude Code, thinking off.
rerank_command = "MAX_THINKING_TOKENS=0 claude -p --model {model} --output-format text --tools \"\" --system-prompt \"You write plain text. You have no tools, no files and no memory: the only thing you can do is print your answer. Do all of the task at once: never ask a question, never ask to confirm, never comment — print only the answer.\" --setting-sources \"\" --strict-mcp-config --no-session-persistence"
rerank_model = "haiku"

# OpenAI's Codex CLI. Read from `codex exec --help` here and not run: with no prompt argument
# the instructions are read from stdin, `-m, --model <MODEL>` names the model, and
# `--skip-git-repo-check` allows a directory that is not a repository. Plain stdout is the run's
# own transcript, so `-o, --output-last-message <FILE>` pointed at /dev/stdout is what guarantees
# the answer reaches stdout at all; the parsers drop the transcript lines around it.
# rerank_command = "codex exec --skip-git-repo-check -m {model} -o /dev/stdout"
# rerank_model = "<a model that install has>"

# A model on the machine, through Ollama — a shape. `ollama run --help` documents
# `ollama run MODEL [PROMPT]` and the thinking switches; nothing was run here, this machine has
# no daemon up and no model pulled, and whether a piped prompt needs the argument omitted is
# for whoever has one to confirm.
# rerank_command = "ollama run {model} --hidethinking"
# rerank_model = "<a pulled model>"

# Anything else — any API, any account: five lines that read stdin and print the answer, and the
# model stays a word in a config file.
# The command runs in an empty directory of its own, so name the script by absolute path.
# rerank_command = "python3 /home/you/tools/rerank-via-some-api.py --model {model}"
```

No non-Claude model has been read on these cases, so none of the rows above is a claim about one.
An attempt on 2026-09-10 filled none of them and wrote down why — an account, an empty Ollama
shelf, a 2.1 GB download, ~$6 of the since-removed enrichment — in
[the cross-vendor refusals](bench/2026-09-10-cross-vendor-refusals.md), which also re-checks
both shapes against the installed CLIs.
Reading one is the command the numbers here came from: `bench --rerank` measures a
`rerank_model`.

### Id families

There is no key for them. A family is the prefix of any id the corpus *defines*, and the documents
are the only place that is written down: the requirement line `**FR-PAY-22 · MUST · <title>**` and
its heading form `## FR-CAL-40 · <title>` (the modality is optional in both), a milestone document
named `BE-M01-….md` or a `## BE-M01 · <title>` head, a row in one of the `registries`. `build` and
`update` read those definitions off the documents they walk, and the graph they write is where the
set is recorded. No reader builds a list of its own: every id is read by one grammar, and which
prefixes are families is a question asked of the graph after every file is read rather than of each
line while it is scanned. Nothing is configured, nothing is stored beside the graph, and there is
nothing to keep in step with anything.

Why derived rather than configured or pinned beside the graph is in
[the measurements](history.md#id-families-why-they-are-derived-and-not-configured).

Every id is matched as `FAMILY-<1–4 digits>`, hyphen included, or `FAMILY-M<2 digits>` for a
milestone, whatever the prefix; hyphenless labels such as `B1` or `S3` are not ids in any repository
and cannot become families. A citation of a prefix no line defines — `ISO-8601`, a ticket number, a
year — is found like any other and then held aside: no reader follows it, no count reports it, and
`verify` says how many are waiting and under which prefixes. Writing a line that defines the prefix
is how it crosses that line, and the next `zerograph update` releases what was held into the graph.

```bash
zerograph families                          # families, milestones, and everything left as text
```

Run after a build, it prints every family with its node count, how many definitions stand behind it
and the `path:line` its first node stands at, then every id-like prefix no line defines, with how
often, under how many distinct ids, and where it is written. Two of those columns exist to put the
report's own edge first: `defs` counts declaring-file × id pairs — summed over the family's nodes,
so a node two documents declare counts twice — the rows sort by it ascending, and
a family with exactly one — one line, which may have been a mistake — reads `defined once`; `ids`
counts the distinct ids written under an undefined prefix, and the mention half sorts by it, because
twenty ids nobody defines is a vocabulary where twenty mentions of one id is a citation repeated.
Both are in `families --json` too, beside the fields that were already there. Both
halves are read off the graph and the tree beside it, so a definition edited away since the last
update still shows the line it was read from, and the command says on stderr how many files the
store is behind. Nothing is written to the repository.

An `update` whose documents gained or lost a family says so — `families: +REQ`, `families: -AC` —
and re-reads nothing else: the one file that changed is read, and the citations the graph was
holding for the new family are released where they lie. A resident `serve` says the same
on the poll that applies it, and `ask`'s own refresh never costs a pass over the whole corpus. A
`zerograph.toml` still naming `id_families` parses, gets one line on stderr, and is otherwise
unaffected.

## Resources

Every reader costs a fraction of a second and the model it opened. The one command that can take a
machine over is a writer that has to embed a store whole — `build`, `update` or `embed`
on rows another model wrote. One word bounds it:

| `resources` | threads | `build` wall | `update` wall | peak CPU (mean), any phase | peak RSS |
| --- | --- | --- | --- | --- | --- |
| `"full"` | 6 | 403.4 s | 83.4 s | 580% (539%) | 1.35 GB |
| `"balanced"` *(the default)* | 4 | 521.1 s | 107.2 s | 396% (390%) | 1.36 GB |
| `"low"` | 2 | 970.2 s | 198.8 s | 201% (199%) | 1.33 GB |

Measured 2026-10-09 on the bench fixture under the default model on a twelve-core Apple Silicon
machine: `build` from an empty store embeds all 11,590 rows, `update` re-reads 94 edited files and
embeds 1,011 ([the results](bench/2026-10-09-bound-build-results.md), per phase). The `balanced` row
is that day's four-thread run, the count 40% gives on twelve cores; it was taken under the earlier
one-third rule, which gave the same four, and has not been re-run on the 40% build. The embed is the
phase that sets the peak: walk, parse, settle and save run on one thread for about two seconds, and
the model open's half second stays under the embed's figure at every level.

The rule is fractions of the logical cores with one thread as the floor: `full` a half, `balanced`
40%, `low` a sixth — so the level follows the machine, and a container gets a fraction of its
quota rather than of the host. A level's threads are all a run computes on: `n` threads read as
`n` cores. `balanced` was a third until 2026-10-09 and 30% until 2026-10-17. At 30% it was three
threads on twelve cores, 301% at peak and 674.3 s on `build`, 139.2 s on `update` — **about 30% more
wall** than the four it is again at 40%, which read 396% and 521.1 s, 107.2 s. Memory is not per thread: peak RSS is
the model's memory-mapped weights, which the system may drop and re-read, plus about 0.35 GB of
the graph and the rows being written, whatever the level. Set it in `zerograph.toml`, once for the
machine in `~/.config/zerograph/config.toml`, or `ZEROGRAPH_RESOURCES=full` for one run. A build
server wants `full`; a laptop you are working on wants `low`.

`resources` is the only resource lever there is: `threads` and `priority` were removed on
2026-09-09, and the background priority that work nobody waits on has run at since 0.6.4
([keeping fresh](keeping-fresh.md#background-priority)) is not a setting and never reaches a
foreground writer; and `full` is now both the most of the machine you can ask for and the fastest setting
the tool has. A long embed checkpoints every 1,024 rows and says where it is, so an interrupt
resumes rather than restarts. What a rebuild costs the person beside it, why `full` is half the
logical cores rather than no cap, and what the removed scheduling band bought and cost are in
[the measurements](history.md#what-a-rebuild-costs-the-machine).
