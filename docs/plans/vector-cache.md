# A machine-wide vector cache replaces borrowing from siblings

Issue #241; closes #236 and #239 by deleting the code they are about.

## Design

A vector is a function of the model, its width and the row's text. The cache keys a row by
exactly that — `blake3(row text)` (the hash `vectors.json` already records), the model id and the
dims — and holds the normalised f32 row a sync stored.

- **Read:** before a sync counts what it owes, it asks the cache for every owed hash
  (`DenseIndex::take_cached`, which replaces `borrow_owed`). Hits join the index as live rows;
  only misses are embedded.
- **Write-through:** every chunk a sync embeds is appended to the cache as it is embedded.
  Nothing else is ever written: not rows loaded from a store, not rows read back from the cache.
- A sync that claimed no model and width (`written_by`) neither reads nor writes the cache.

## On-disk format

```
<root>/<model-slug>-<8 hex of model id>@<dims>/<xx>.rows   # xx = first key byte, 256 shards
<root>/<model-slug>-<8 hex of model id>@<dims>/<xx>.lock   # the shard's writer lock
<root>/prune.lock
```

Root: `~/.cache/zerograph/vectors` on Linux and macOS, `%LOCALAPPDATA%\zerograph\vectors` on
Windows, `$ZEROGRAPH_VECTOR_CACHE` when set (benches and tests).

A shard is a header — magic `ZGVCACHE`, format version, dims, the lower-cased model id — then
fixed-width records: `key[32] | used u32 | f32 × dims | check[8]`, where `check` is the first 8
bytes of `blake3(key ‖ row)` and `used` is minutes since the epoch. The fixed stride is the index:
a reader walks the key column of the one shard a hash falls in, with no second file to tear.

## Concurrency

- Readers take no lock. They count whole records only and verify each record's check, so a
  record being appended, cut short by a crash, or rewritten under them is skipped, never
  returned. A row is returned only when its width is the directory's and every float is finite.
- Writers lock `<xx>.lock` (a short blocking lock held for one `write_all`), cut a torn tail back
  to the last whole record, and append. A shard whose header does not parse or names another
  model or width is started over.
- No store's writer lock is taken or consulted.
- `prune` holds `prune.lock` (try-lock; a second pruner skips) and each shard's lock while it
  rewrites that shard to a temp file renamed over it. A reader holding the old file reads the old
  rows.

## Trust

Only rows this user embedded on this machine enter the cache: the single write site is the
embed loop of `sync_chunked`. No store's `vectors.f32` is ever imported, so a `.zerograph` a
branch committed (#236) seeds nothing. The model id is part of the key and of every shard's
header, so a model or width mismatch never crosses, even across a slug collision.

The root is made like the socket directory (`prepare_private_dir`, shared with `serve`):
created 0700, then refused unless it is a real directory, owned by this uid, with no group or
other bits. Shard and lock files are opened with `O_NOFOLLOW`. A refused or unusable root
disables the cache for the run with one line on stderr; nothing fails because of it.

## Eviction

`vector_cache_max` in the machine config (`~/.config/zerograph/config.toml`), default `4G`; `0`
turns the cache off. A repository's `zerograph.toml` cannot set it — a clone must not be able to
empty or bloat a machine-wide cache. After a write that takes the cache over the cap, the writer
prunes to 80 % of it, least recently used first. A hit bumps its record's `used` in place when it
is over an hour old, which is the only write a reader makes and needs no lock: `used` is outside
the check, so any value it holds is valid.

`zerograph cache status` reports the root, the cap and rows and bytes per model; `zerograph cache
prune` trims to the cap and drops torn and corrupt records.

## Windows

The root comes from `%LOCALAPPDATA%`, whose profile ACL keeps others out, as for the socket.
Nothing here calls git, so no `canonicalize()` path can reach it. File locks are `LockFileEx`
through std; std opens with `FILE_SHARE_DELETE`, so prune's rename is not blocked by a reader.

## Migration

None needed. The cache starts empty and fills from the next embed. `src/index/siblings.rs`,
`DenseIndex::borrow`, `borrow_owed`, the `home`/`borrowed` fields, their tests and the
"Borrowing from sibling worktrees" docs are deleted; a new worktree is served from the cache the
main checkout's own embeds filled.

## Tests

Cache (`src/index/cache.rs`):
1. A put row comes back; another model, another width, or a slug collision gets nothing.
2. A torn tail is skipped and the next put realigns it; both rows read back.
3. A flipped byte in a record drops that record only; a garbage or short shard is no panic.
4. A non-finite row is never stored and never returned.
5. Concurrent writers on one shard lose no row and tear none.
6. Prune drops the least recently used first and a hit refreshes its row.
7. The root is made 0700 and refused when group-readable or a link (unix).
8. Status counts rows and bytes per model.

Dense (`src/index/dense.rs`):
1. Rows one checkout embedded are taken from the cache by an unrelated checkout: nothing
   embedded, the same vectors.
2. `take_cached` rows are not owed, and the sync after it embeds exactly the rest.
3. A row of another model or width is never taken.
4. Only embedded rows are written: a store's own rows never reach the cache.
5. A sync that claimed no model neither reads nor writes.
6. A checkpoint keeps the cached rows beside the embedded ones.

Config: `vector_cache_max` parses `4G`/bytes/`0`, is machine-only, and a project file naming it
is told so.
