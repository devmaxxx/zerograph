# Asking a resident process: serve

Most of a fused `ask` is the process opening things it then throws away: the embedding model
alone costs about 220 ms of it on the shipped default. `serve` opens
them once and answers over a Unix socket — on Windows too, where the same socket file has existed
since Windows 10 1803 and is protected the way your profile directory is:

```bash
zerograph serve                  # ~/.cache/zerograph/sock/<hash>.sock, poll every 30 s, exit after 30 min idle
zerograph serve --idle 86400     # a day rather than half an hour before it gives up
zerograph serve --every 15       # poll the tree every 15 s rather than 30
zerograph serve --batch 5        # save once five files are waiting, not on every one
zerograph serve --idle-model 60  # forget the model after a minute unasked; the process stays
zerograph serve --no-dense       # lexical only; a fused `ask` is told so and answers in its own process
zerograph serve --detach         # in a process of its own, output in .zerograph/serve.log; returns once it listens
zerograph stop                   # end the server answering here, socket file and all
zerograph ask --no-serve отмена  # answer here even while one is listening
```

A poll that applies an edit rewrites the whole graph, so `--batch` spends that once on a burst of
edits rather than once per file. Fewer than `--batch` files waiting are carried to the next poll, at
most three times in a row, so a lone edit lands within four polls whatever the batch says. A poll
that finds a refresh holding the store's writer lock skips its cycle.

Two idles, one clock. `--idle` ends the process; `--idle-model` (default 300 s) drops only the
model weights and keeps everything that answers without them — the graph, the ids, the lexical
indexes, the vectors — so a server left up overnight is cheap to leave up. On a 33.5k-row store the
resident size goes 908.5 MB → 28.8 MB on the first drop and 277.6 MB on later ones (the allocator
keeps some of what the reopen took), and the first fused `ask` after a drop pays the open: 0.771 s
against 0.083 s warm. The local reranker is held the same way: `serve` opens the cross-encoder the
first time an `ask --rerank-local` asks for it, keeps it between questions and drops it on the same
`--idle-model`.

`ask --rerank` and `ask --rerank-local` are both answered by the resident process, and a client
waits five minutes rather than thirty seconds for either — a local rerank of a 200-deep pool runs
70-80 s cold on CPU, and a client that gave up at thirty seconds would score the same pool itself
behind its own cold open, paying the whole cost a second time. The five minutes are bought: the
thread that accepts a connection writes an acknowledgement on a line of its own straight away,
before the question is even read, and the client waits thirty seconds for that line. Accepting is
what the server can promise while it is still answering somebody else, so a question queued behind
a 70-80 s rerank keeps its five minutes. A server that accepted the connection and will never
answer — a wedged worker, a process stopped under a debugger — hands the question back as
promptly as it does for every other request.

The socket lives at `~/.cache/zerograph/sock/<hash>.sock`, the hash taken of the repository's
canonical path, in a directory `serve` makes private to you (mode `0700` on unix; a directory there
that is open to others is refused). One place for every repository means every client finds it
however deep the checkout sits and whatever `$TMPDIR` the client was started under — a shell and an
editor's agent can disagree on that. Only when the path under your home would be longer than 100 bytes — a
margin under the Unix socket name limit, 104 bytes on macOS including the terminating NUL — or there is no home
directory, does it go in a private directory of yours under the temporary directory as
`zerograph-<uid>/zerograph-<hash>.sock` (the temporary directory itself on Windows, which is already
per-user). Server and clients compute the name the same way, so nothing has to be told which one is
in use: the startup line prints it, `zerograph status` prints the path it checked, and `status
--json` carries it as `serve.socket`, for a tool that looks for a server on its own.

A `serve` that is killed with `SIGTERM` or `SIGINT` takes its socket file with it, because it
leaves through the same exit as an idle timeout, and so does one ended by `zerograph stop`, on
every platform. On Windows a `Stop-Process` sends no signal and the file is left for the
next `serve`, which removes a dead socket before it binds.

`ask` uses it without being told to and answers in this process whenever it cannot — no socket, a
socket nobody listens on, a server of another version or another build of the same version, a
`--no-dense` server asked a fused question, a timeout, `--no-serve`, or `ZEROGRAPH_NO_SERVE`. Each
of those prints a line: a question that quietly costs a cold process, or quietly gets a lexical
answer, looks like nothing at all. A client never deletes the socket file, since a refused connect
is also what a live server with a full backlog gives; only `serve` removes one, and only the one it
bound itself — told from a replacement's by device and inode on unix, by NTFS's file reference
number on Windows.

**Upgrading from 0.6.4 or earlier.** A server started by zerograph 0.6.4 or earlier listens at the
old place — `.zerograph/serve.sock`, or the temporary directory for a deep checkout — so a newer
release does not find it: `ask` answers in its own process, or a new `serve` starts beside the old
one, and the old one leaves at its own `--idle` timeout. To end it sooner, run the old binary's
`zerograph stop`.

The socket name is bounded by the platform's own limit — 104 bytes on macOS, 108 elsewhere — so
only a very deep home directory reaches the fallback; the arithmetic of a deep Windows path, and the check that a resident
answer is byte-identical to a cold one across all 142 recorded questions, are in
[the measurements](history.md#the-socket-path-and-the-same-bytes-either-way).

The server refreshes before every answer with the same walk a one-shot `ask` does, and polls
the tree between them, so its **graph** is never staler than a fresh process's; `--stale` skips
that walk and reads the stored graph as it is on disk. It answers one question at a time, and a
second client waits rather than being turned away. Both reads are decided by `manifest.json`, so a
running server does not see `embed` writing `vectors.*` — stop
it, or ask with `--no-serve`, to answer from the new ones.

The configuration is read once, at start-up. Editing `zerograph.toml` stops the server after its
next poll — the next `ask` answers in its own process under the new file, and a new `serve` starts
under it too.

Measured on the bench corpus, median of eleven with socket and in-process runs interleaved: a fused
question is **66 ms** resident against 326 ms in a fresh process, and the lexical arm **6.8 ms**
against 106 ms once the BM25 indexes stopped being rebuilt per question. The first question after a
start still pays the model open. The full tables, and the evidence that the answer bytes do not
move, are in [the measurements](history.md#the-resident-answer-measured).

