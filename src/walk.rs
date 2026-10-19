use crate::config::Config;
use anyhow::{Context, Result};
use globset::{Glob, GlobSet, GlobSetBuilder};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileKind { Doc, Code, Registry, Text }

/// What a `stat` says about a file: enough to decide that re-reading it would be wasted work.
/// The blake3 hash stays the truth — a stamp only ever skips recomputing one, never declares a
/// file changed, so a touched-but-identical file is still hashed and still diffs as unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stamp { pub mtime_ns: u64, pub len: u64 }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry { pub rel: String, pub kind: FileKind, pub hash: String, pub stamp: Option<Stamp> }

/// The generation of the grammar a file is read by — the id shapes, the definition heads, the
/// registry rows, the ids a source file cites, the calls it makes, and which of those a settled
/// graph admits. Bumped by hand when a change to any of them would make a re-read of a file that
/// has not moved yield a different graph, and left alone by a release that does not touch them:
/// this number is what forces one whole-tree re-read on the first writer after an upgrade (see
/// `apply_diff`), and a bump nobody needed is that walk paid for nothing. `0` belongs to no
/// generation: it is what a manifest written before the stamp existed reads as, and what a writer
/// leaves behind when a file it had to read would not open — neither store was read whole, and
/// `0` is stale against every generation there is or will be.
/// 3: a call on a helper's return value (#91), a JSX element (#93) and a function passed as an
/// argument (#98) are calls, the last with context `arg` (#104), and a tsconfig alias whose glob
/// holds `/*` resolves (#97).
/// 4: `main` with the 0.5.5 fixes above forward-ported. 0.5.5 stamps its stores 3 and reads by
/// 0.5.5's grammar, not 0.6.0's, so a 3 here would leave a store 0.5.5 wrote unread; one above
/// both makes a 0.5.4 store (2) and a 0.5.5 store (3) re-read once on the first writer.
/// 5: 0.6.0 — every language ships, read by default, and text files are a node kind; a store any
/// 0.5.x or the forward-ported `main` wrote is re-read once.
/// 6: every language read by one query engine. Some nodes' spans and bodies differ from what 5's
/// per-language readers wrote, such as a decorated field's line or the comment kept above a
/// declaration, so a store stamped 5 is re-read once.
/// 7: a credential written inside a file is replaced before the file is read, so a store stamped 6
/// still holds the ones its unmoved files carry until it is re-read once.
pub const GRAMMAR: u32 = 7;

/// Refused as text whatever `text_globs` says: machine output nobody asks a question of, any one of
/// which would outweigh the hand-written files in the lexical index. Text entries only — `skip`
/// still decides for docs and code, so nothing already in the graph leaves it (spec §11, T1).
const TEXT_REFUSED: [&str; 11] = [
    "**/package-lock.json", "**/yarn.lock", "**/pnpm-lock.yaml", "**/Cargo.lock", "**/poetry.lock",
    "**/Gemfile.lock", "**/composer.lock", "**/go.sum", "**/*.min.*", "**/*.map", "**/vendor/**",
];

/// Never read, whatever a glob or `skip` says: files that hold credentials. Their content would
/// land in `graph.json`, the BM25 and dense indexes, a rerank prompt sent to a model, and every
/// answer that seeds them. `.gitignore` keeps most of them out already; this is the floor for the
/// repository that commits one, or is not under git at all.
/// Matched without regard to case: `.ENV` and `Prod.PEM` hold the same thing.
const SECRET: [&str; 40] = [
    "**/.env", "**/.env.*", "**/.env-*", "**/.env_*", "**/*.env", "**/.envrc", "**/.dev.vars", "**/*.pem", "**/*.key",
    "**/*.p12", "**/*.pfx", "**/*.jks", "**/*.keystore", "**/*.kdbx", "**/*.ppk", "**/*.gpg",
    "**/*.asc", "**/id_{rsa,dsa,ecdsa,ed25519}", "**/.npmrc", "**/.pypirc", "**/.netrc",
    "**/.pgpass", "**/.git-credentials", "**/.htpasswd", "**/.boto", "**/.s3cfg", "**/.dockercfg",
    "**/.aws/credentials", "**/.docker/config.json", "**/kubeconfig", "**/.kube/config",
    "**/*.tfvars", "**/*.tfvars.json", "**/*.tfstate", "**/*.tfstate.*", "**/credentials*.json",
    "**/service-account*.json", "**/{secret,secrets}.{yaml,yml,json,toml,ini,conf,txt}", "**/*.{secret,secrets}", "**/.vault_pass*",
];

/// Templates that name a project's variables with placeholder values: the one shape of env file
/// worth a question, and written to be committed.
const SECRET_TEMPLATES: [&str; 4] = ["**/.env.example", "**/.env.sample", "**/.env.template", "**/.env.dist"];

/// Over this a text file is data, not something written to be read, and is left out and counted.
pub const TEXT_MAX_BYTES: u64 = 1 << 20;

/// How far `git` looks for a NUL before calling a file binary.
const BINARY_PROBE: usize = 8_000;

fn binary(head: &[u8]) -> bool { head[..head.len().min(BINARY_PROBE)].contains(&0) }

/// Only the head is read: a binary over the limit is left out without being counted as text,
/// and without reading the rest of it.
fn binary_at(path: &Path) -> std::io::Result<bool> {
    use std::io::Read;
    let mut head = Vec::with_capacity(BINARY_PROBE);
    std::fs::File::open(path)?.take(BINARY_PROBE as u64).read_to_end(&mut head)?;
    Ok(binary(&head))
}

#[derive(Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct Manifest {
    pub files: BTreeMap<String, String>,
    /// A manifest written before the stat cache existed has none; every file is hashed once more
    /// and the stamps are there from the next save on.
    #[serde(default)] pub stamps: BTreeMap<String, Stamp>,
    /// The grammar generation the graph saved beside this manifest was read whole by, or `0` for
    /// none. It belongs here and not on the graph because it answers the same question the hashes
    /// and stamps do — must this file be read again — and every writer holds the manifest at the
    /// moment it asks.
    #[serde(default)] pub grammar: u32,
    /// Which reader each file went to. The hash alone cannot tell that a glob moved a file from one
    /// reader to another — `text_globs` reaching a `.py` file a later `code_globs` claims — so a file
    /// whose kind moved diffs as changed. Empty in a manifest written before kinds were recorded,
    /// which compares nothing until the next save writes them.
    #[serde(default)] pub kinds: BTreeMap<String, FileKind>,
    /// The zerograph release that read the graph beside this manifest, so a store another release
    /// wrote can be named by more than its grammar number. Empty in a manifest written before it
    /// was recorded.
    #[serde(default)] pub release: String,
}

#[derive(Debug, Default)]
pub struct Diff { pub changed: Vec<Entry>, pub removed: Vec<String> }

pub(crate) fn globs(globs: &[String]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for g in globs {
        b.add(Glob::new(g).with_context(|| format!("glob {g}"))?);
    }
    Ok(b.build()?)
}

/// `include` as globs: a plain directory reaches everything under it, and `./` or a trailing `/`
/// mean nothing. `None` for an empty list, which is the whole repository.
pub(crate) fn included(include: &[String]) -> Result<Option<GlobSet>> {
    let mut out = Vec::new();
    for raw in include {
        let p = raw.trim().trim_start_matches("./").trim_start_matches('/').trim_end_matches('/');
        if p.is_empty() || p == "." { return Ok(None); }
        // A glob can name a directory too (`packages/*/src`), and its files are under it.
        out.push(p.to_string());
        out.push(format!("{p}/**"));
    }
    if out.is_empty() { return Ok(None); }
    globs(&out).map(Some)
}

fn globs_any_case(globs: &[&str]) -> Result<GlobSet> {
    let mut b = GlobSetBuilder::new();
    for g in globs { b.add(globset::GlobBuilder::new(g).case_insensitive(true).build()?); }
    Ok(b.build()?)
}

pub(crate) fn stamp_of(meta: &std::fs::Metadata) -> Option<Stamp> {
    let ns = meta.modified().ok()?.duration_since(std::time::UNIX_EPOCH).ok()?.as_nanos();
    Some(Stamp { mtime_ns: u64::try_from(ns).ok()?, len: meta.len() })
}

/// `prev` is the manifest of the last walk; a file whose stamp still matches keeps its recorded
/// hash instead of being read. Pass `&Manifest::default()` to hash everything.
pub fn walk(repo: &Path, cfg: &Config, prev: &Manifest) -> Result<Vec<Entry>> {
    Ok(walk_inner(repo, cfg, prev, false)?.0)
}

/// As `walk`, with the number of text files left out for size. Only a caller that reports the
/// count pays for it: telling an oversized text file from a binary opens every one of them.
pub fn walk_counted(repo: &Path, cfg: &Config, prev: &Manifest) -> Result<(Vec<Entry>, usize)> {
    walk_inner(repo, cfg, prev, true)
}

/// The store at the repository root. Root only, as the default `skip` is: a `.zerograph` deeper
/// in the tree belongs to some other project and is content like the rest.
fn is_store(e: &ignore::DirEntry) -> bool {
    e.depth() == 1 && e.file_name() == crate::store::DIR
}

/// A directory below the root that holds a `.git` of its own — a worktree, a clone, a submodule —
/// is another checkout with an index of its own, and walking it would fold its files into this
/// one's. Kept out whether or not the repository's ignore file names it.
fn is_nested_checkout(e: &ignore::DirEntry) -> bool {
    e.depth() > 0 && e.file_type().is_some_and(|t| t.is_dir()) && std::fs::symlink_metadata(e.path().join(".git")).is_ok()
}

fn walk_inner(repo: &Path, cfg: &Config, prev: &Manifest, count: bool) -> Result<(Vec<Entry>, usize)> {
    let docs = globs(&cfg.doc_globs)?;
    let code = globs(&cfg.code_globs)?;
    let skip = globs(&cfg.skip)?;
    let registries = globs(&cfg.registries)?;
    let text = globs(&cfg.text_globs)?;
    let refused = globs(&TEXT_REFUSED.map(String::from))?;
    let include = included(&cfg.include)?;
    let secret = globs_any_case(&SECRET)?;
    let template = globs_any_case(&SECRET_TEMPLATES)?;
    let mut oversized = 0usize;
    let mut out = Vec::new();
    // A repository keeps its agent rules, its hooks and its CI in dotted directories, so the
    // walk reads them and `skip` decides, as it does for every other path. `.git` has to go:
    // `ignore` gives it no special treatment once `hidden` is off (0.4.33), and its thousands of
    // objects would be walked like source. `.gitignore` keeps working either way — the hidden
    // filter and the git-ignore matcher are independent. The store goes too, under either name,
    // and not through `skip`: a project's `skip` replaces the default that names it, often with a
    // list that still names only the old one, and the store's own `.gitignore` is read inside a
    // git checkout alone.
    let walker = ignore::WalkBuilder::new(repo)
        .hidden(false)
        .filter_entry(|e| e.file_name() != ".git" && !is_store(e) && !is_nested_checkout(e))
        .git_ignore(true)
        .build();
    for dent in walker {
        let dent = match dent {
            Ok(d) => d,
            Err(e) => { eprintln!("{}", crate::log_line(&format!("walk: {e}"))); continue; }
        };
        if !dent.file_type().map(|t| t.is_file()).unwrap_or(false) {
            continue;
        }
        let rel_path = dent.path().strip_prefix(repo).unwrap_or(dent.path());
        let Some(rel) = rel_path.to_str().map(|s| s.replace('\\', "/")) else {
            eprintln!("walk: skipping non-UTF-8 path {:?}", rel_path);
            continue;
        };
        if include.as_ref().is_some_and(|i| !i.is_match(&rel))
            || skip.is_match(&rel)
            || (secret.is_match(&rel) && !template.is_match(&rel))
        {
            continue;
        }
        let kind = if registries.is_match(&rel) { FileKind::Registry }
            else if docs.is_match(&rel) { FileKind::Doc }
            else if code.is_match(&rel) { FileKind::Code }
            else if text.is_match(&rel) && !refused.is_match(&rel) { FileKind::Text }
            else { continue };
        // Stamped before the read, never after: a write racing this walk then leaves a stamp
        // older than the file, and the next walk hashes it again rather than trusting the hash.
        let stamp = dent.metadata().ok().as_ref().and_then(stamp_of);
        // Before the cache: a file that grew past the limit keeps its stamp's old hash otherwise.
        if kind == FileKind::Text && stamp.is_some_and(|s| s.len > TEXT_MAX_BYTES) {
            if count && !binary_at(dent.path()).unwrap_or(true) { oversized += 1; }
            continue;
        }
        if let (Some(s), Some(hash)) = (stamp, prev.files.get(&rel)) {
            if prev.stamps.get(&rel) == Some(&s) {
                out.push(Entry { rel, kind, hash: hash.clone(), stamp });
                continue;
            }
        }
        // A binary is never in the manifest, so the cache above never answers for it: probing the
        // head keeps every walk from reading each one whole again.
        if kind == FileKind::Text && binary_at(dent.path()).unwrap_or(false) { continue; }
        let bytes = match std::fs::read(dent.path()) {
            Ok(b) => b,
            Err(e) => { eprintln!("{}", crate::terminal_safe(&format!("walk: skipping {rel:?}: {e}"))); continue; }
        };
        if kind == FileKind::Text && (binary(&bytes) || bytes.len() as u64 > TEXT_MAX_BYTES) {
            // Reached over the limit only when the stat above failed and gave no stamp.
            if !binary(&bytes) { oversized += 1; }
            continue;
        }
        out.push(Entry { rel, kind, hash: blake3::hash(&bytes).to_hex().to_string(), stamp });
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok((out, oversized))
}

impl Manifest {
    pub fn from_entries(entries: &[Entry]) -> Manifest {
        Manifest {
            files: entries.iter().map(|e| (e.rel.clone(), e.hash.clone())).collect(),
            stamps: entries.iter().filter_map(|e| Some((e.rel.clone(), e.stamp?))).collect(),
            grammar: GRAMMAR,
            kinds: entries.iter().map(|e| (e.rel.clone(), e.kind)).collect(),
            release: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    /// Whether the graph beside this manifest was read by an older grammar than this build's — a
    /// store every hash calls current and that is still short of what a re-read would find. One
    /// walk is what makes it the store this build would have written.
    pub fn stale_grammar(&self) -> bool {
        self.grammar < GRAMMAR
    }

    /// Whether a newer release read the graph beside this manifest. This build cannot write the
    /// store it would make — re-reading it would stamp it with the older grammar and drop what the
    /// newer one found — so it answers from it as it stands and writes nothing.
    pub fn newer_grammar(&self) -> bool {
        self.grammar > GRAMMAR
    }

    /// The one line every reader and writer says about a store [`Manifest::newer_grammar`] holds.
    pub fn newer_store_line(&self) -> String {
        let by = match self.release.is_empty() { true => String::new(), false => format!(" (zerograph {})", self.release) };
        format!("grammar: this store was read by generation {}{by} and this build ({}) reads by {GRAMMAR}; it is left as it stands — answered read-only, never rewritten; a newer zerograph refreshes it",
            self.grammar, env!("CARGO_PKG_VERSION"))
    }

    pub fn diff(&self, now: &[Entry]) -> Diff {
        let mut d = Diff::default();
        for e in now {
            if self.files.get(&e.rel) != Some(&e.hash) || self.kinds.get(&e.rel).is_some_and(|k| *k != e.kind) {
                d.changed.push(e.clone());
            }
        }
        let present: std::collections::BTreeSet<&str> = now.iter().map(|e| e.rel.as_str()).collect();
        d.removed = self.files.keys().filter(|k| !present.contains(k.as_str())).cloned().collect();
        d
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn repo() -> tempfile::TempDir {
        let d = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(d.path().join(".git")).unwrap();
        std::fs::write(d.path().join(".git/config"), "[core]\n").unwrap();
        std::fs::create_dir_all(d.path().join(".claude/hooks")).unwrap();
        std::fs::write(d.path().join(".claude/CLAUDE.md"), "# rules\n").unwrap();
        std::fs::write(d.path().join(".claude/hooks/h.mjs"), "export const h = 1;\n").unwrap();
        std::fs::create_dir_all(d.path().join("docs")).unwrap();
        std::fs::create_dir_all(d.path().join("node_modules/x")).unwrap();
        std::fs::write(d.path().join("docs/a.md"), "# a\n").unwrap();
        std::fs::write(d.path().join("docs/TRACKER.md"), "generated\n").unwrap();
        std::fs::write(d.path().join("docs/constitution.yaml"), "version: 1\n").unwrap();
        std::fs::write(d.path().join("b.ts"), "export const b = 1;\n").unwrap();
        std::fs::write(d.path().join("node_modules/x/c.ts"), "export const c = 1;\n").unwrap();
        std::fs::write(d.path().join(".gitignore"), "ignored.md\n").unwrap();
        std::fs::write(d.path().join("ignored.md"), "# hidden\n").unwrap();
        d
    }

    #[test]
    fn walk_classifies_and_skips() {
        let d = repo();
        let entries = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        let rels: Vec<_> = entries.iter().map(|e| (e.rel.as_str(), e.kind)).collect();
        assert_eq!(rels, vec![
            (".claude/CLAUDE.md", FileKind::Doc),
            (".claude/hooks/h.mjs", FileKind::Code),
            ("b.ts", FileKind::Code),
            ("docs/a.md", FileKind::Doc),
            ("docs/constitution.yaml", FileKind::Registry),
        ]);
        assert_eq!(entries[2].hash, blake3::hash(b"export const b = 1;\n").to_hex().to_string());
    }

    #[test]
    fn diff_reports_changed_and_removed() {
        let d = repo();
        let before = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        let manifest = Manifest::from_entries(&before);
        std::fs::write(d.path().join("docs/a.md"), "# a changed\n").unwrap();
        std::fs::remove_file(d.path().join("b.ts")).unwrap();
        let after = walk(d.path(), &Config::default(), &manifest).unwrap();
        let diff = manifest.diff(&after);
        assert_eq!(diff.changed.iter().map(|e| e.rel.as_str()).collect::<Vec<_>>(), vec!["docs/a.md"]);
        assert_eq!(diff.removed, vec!["b.ts".to_string()]);
    }

    #[test]
    fn unchanged_tree_diffs_empty() {
        let d = repo();
        let before = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        let diff = Manifest::from_entries(&before).diff(&before);
        assert!(diff.changed.is_empty() && diff.removed.is_empty());
    }

    // Stores written before the stat cache have no stamps; they must load and cost one more
    // hashing pass, after which the walk is stamp-driven again.
    #[test]
    fn a_manifest_without_stamps_loads_and_rehashes_once() {
        let d = repo();
        let hashed = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        let old: Manifest = serde_json::from_str(&serde_json::to_string(&Manifest {
            files: Manifest::from_entries(&hashed).files,
            stamps: BTreeMap::new(),
            grammar: GRAMMAR,
            kinds: BTreeMap::new(),
            release: String::new(),
        }).unwrap()).unwrap();
        assert!(old.stamps.is_empty());
        let again = walk(d.path(), &Config::default(), &old).unwrap();
        assert_eq!(again, hashed);
        assert_eq!(Manifest::from_entries(&again).stamps.len(), hashed.len());
    }

    // A manifest from a release that stamped no grammar is behind whatever this build reads by,
    // and it is the only thing that says so: every hash in it still matches the tree.
    #[test]
    fn a_manifest_without_a_grammar_stamp_reads_as_behind_this_one() {
        let old: Manifest = serde_json::from_str("{\"files\":{}}").unwrap();
        assert_eq!(old.grammar, 0);
        assert!(old.stale_grammar());
        assert!(!Manifest::from_entries(&[]).stale_grammar());
    }

    // A newer release's store is not behind this build: re-reading it would stamp it with the
    // older grammar and drop what the newer one found.
    #[test]
    fn a_manifest_a_newer_grammar_wrote_is_newer_not_stale_and_names_both_generations() {
        let newer = Manifest { grammar: GRAMMAR + 1, release: "9.9.9".into(), ..Manifest::default() };
        assert!(newer.newer_grammar());
        assert!(!newer.stale_grammar());
        let line = newer.newer_store_line();
        assert!(line.contains(&format!("generation {}", GRAMMAR + 1)) && line.contains(&format!("reads by {GRAMMAR}")), "{line}");
        assert!(line.contains("zerograph 9.9.9") && !line.contains('\n'), "{line}");
        assert!(!Manifest::from_entries(&[]).newer_grammar());
    }

    #[test]
    fn a_manifest_records_the_release_that_wrote_it_and_one_without_it_still_loads() {
        assert_eq!(Manifest::from_entries(&[]).release, env!("CARGO_PKG_VERSION"));
        let old: Manifest = serde_json::from_str(&format!("{{\"files\":{{}},\"grammar\":{GRAMMAR}}}")).unwrap();
        assert!(old.release.is_empty());
        assert!(!old.newer_store_line().contains("(zerograph )"));
    }

    // A worktree, clone or submodule inside the checkout has a `.git` of its own; its files are
    // that checkout's, whether or not an ignore file names the directory.
    #[test]
    fn a_checkout_nested_below_the_root_is_not_walked() {
        let d = repo();
        for (nested, git_is_file) in [(".claude/worktrees/wt", true), ("vendored/clone", false)] {
            let root = d.path().join(nested);
            std::fs::create_dir_all(root.join("docs")).unwrap();
            std::fs::write(root.join("docs/n.md"), "# nested\n").unwrap();
            match git_is_file {
                true => std::fs::write(root.join(".git"), "gitdir: /elsewhere\n").unwrap(),
                false => std::fs::create_dir_all(root.join(".git")).unwrap(),
            }
        }
        let rels: Vec<String> = walk(d.path(), &Config::default(), &Manifest::default()).unwrap().into_iter().map(|e| e.rel).collect();
        assert!(rels.iter().any(|r| r == "docs/a.md"), "the root's own files stay: {rels:?}");
        assert!(rels.iter().all(|r| !r.contains("n.md")), "{rels:?}");
    }

    // The stamp is what decides whether a file is read at all: with the recorded mtime and length
    // put back, the walk hands back the hash it stored rather than the one on disk.
    #[test]
    fn a_file_whose_stamp_is_unchanged_is_not_read_again() {
        let d = repo();
        let before = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        let manifest = Manifest::from_entries(&before);
        let path = d.path().join("docs/a.md");
        let was = std::fs::metadata(&path).unwrap().modified().unwrap();
        std::fs::write(&path, "# A\n").unwrap();
        std::fs::File::options().write(true).open(&path).unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(was)).unwrap();
        let after = walk(d.path(), &Config::default(), &manifest).unwrap();
        assert!(manifest.diff(&after).changed.is_empty());
    }

    // A rewrite with identical bytes moves the mtime, so the file is hashed again — and the hash
    // is what the diff answers on, so nothing is reported as changed.
    #[test]
    fn a_touched_but_identical_file_is_not_reported_as_changed() {
        let d = repo();
        let before = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        let manifest = Manifest::from_entries(&before);
        std::fs::write(d.path().join("docs/a.md"), "# a\n").unwrap();
        let after = walk(d.path(), &Config::default(), &manifest).unwrap();
        assert_ne!(after.iter().find(|e| e.rel == "docs/a.md").unwrap().stamp, manifest.stamps.get("docs/a.md").copied());
        let diff = manifest.diff(&after);
        assert!(diff.changed.is_empty() && diff.removed.is_empty());
    }

    fn init(d: &Path) {
        std::fs::create_dir_all(d.join(".git")).unwrap();
    }

    #[test]
    fn skip_globs_win_over_doc_globs() {
        let d = tempfile::tempdir().unwrap();
        init(d.path());
        std::fs::create_dir_all(d.path().join("docs")).unwrap();
        // TRACKER.md matches the default doc glob "**/*.md" as much as it matches the skip glob.
        std::fs::write(d.path().join("docs/TRACKER.md"), "generated\n").unwrap();
        let entries = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn registry_beats_doc_for_the_same_path() {
        let d = tempfile::tempdir().unwrap();
        init(d.path());
        std::fs::create_dir_all(d.path().join("docs")).unwrap();
        std::fs::write(d.path().join("docs/constitution.yaml"), "version: 1\n").unwrap();
        let mut cfg = Config::default();
        cfg.doc_globs.push("**/*.yaml".to_string()); // now overlaps the registry glob for the same path
        let entries = walk(d.path(), &cfg, &Manifest::default()).unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, FileKind::Registry);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_non_utf8_file_name_is_skipped_without_failing() {
        use std::os::unix::ffi::OsStrExt;
        let d = tempfile::tempdir().unwrap();
        init(d.path());
        std::fs::write(d.path().join(std::ffi::OsStr::from_bytes(b"bad_\xFF\xFE.md")), "x\n").unwrap();
        std::fs::write(d.path().join("good.md"), "y\n").unwrap();
        let entries = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        assert_eq!(entries.iter().map(|e| e.rel.as_str()).collect::<Vec<_>>(), vec!["good.md"]);
    }

    // `.git` is the one dotted directory that stays out by name: nothing in `ignore` prunes it
    // once the hidden filter is off, and a repository's objects are not its source.
    #[test]
    fn the_git_directory_is_not_walked() {
        let d = repo();
        std::fs::create_dir_all(d.path().join(".git/objects")).unwrap();
        std::fs::write(d.path().join(".git/objects/a.ts"), "export const a = 1;\n").unwrap();
        let entries = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        assert!(entries.iter().all(|e| !e.rel.starts_with(".git/")));
    }

    #[test]
    fn javascript_is_code_and_a_bundle_is_not() {
        let d = tempfile::tempdir().unwrap();
        init(d.path());
        std::fs::write(d.path().join("eslint.config.js"), "export default [];\n").unwrap();
        std::fs::write(d.path().join("hook.cjs"), "module.exports = 1;\n").unwrap();
        std::fs::write(d.path().join("app.jsx"), "export const A = 1;\n").unwrap();
        std::fs::write(d.path().join("vendor.min.js"), "!function(){}();\n").unwrap();
        let entries = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        assert_eq!(
            entries.iter().map(|e| e.rel.as_str()).collect::<Vec<_>>(),
            vec!["app.jsx", "eslint.config.js", "hook.cjs"]
        );
    }

    // `.yarn/` and `.pnp.cjs` are Yarn Berry's committed, generated wiring — not gitignored, since
    // zero-installs need them in the tree — so only `skip` keeps them out now that dotted
    // directories are walked.
    #[test]
    fn yarn_berrys_generated_files_are_not_walked() {
        let d = tempfile::tempdir().unwrap();
        init(d.path());
        std::fs::create_dir_all(d.path().join(".yarn/releases")).unwrap();
        std::fs::write(d.path().join(".yarn/releases/yarn-4.5.0.cjs"), "#!/usr/bin/env node\n").unwrap();
        std::fs::write(d.path().join(".pnp.cjs"), "module.exports = {};\n").unwrap();
        std::fs::write(d.path().join("src.ts"), "export const a = 1;\n").unwrap();
        let entries = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        assert_eq!(entries.iter().map(|e| e.rel.as_str()).collect::<Vec<_>>(), vec!["src.ts"]);
    }

    #[test]
    fn a_gitignored_file_is_not_walked() {
        let d = tempfile::tempdir().unwrap();
        init(d.path());
        std::fs::write(d.path().join(".gitignore"), "secret.md\n").unwrap();
        std::fs::write(d.path().join("secret.md"), "x\n").unwrap();
        std::fs::write(d.path().join("public.md"), "y\n").unwrap();
        let entries = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        assert_eq!(entries.iter().map(|e| e.rel.as_str()).collect::<Vec<_>>(), vec!["public.md"]);
    }

    #[test]
    fn diff_puts_a_brand_new_file_in_changed_not_a_separate_list() {
        // `Diff` has no third "added" list: a file `self.files` has never seen fails the same
        // hash comparison as a changed one, so it surfaces through `changed` too.
        let d = repo();
        let before = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        let manifest = Manifest::from_entries(&before);
        std::fs::write(d.path().join("docs/new.md"), "# new\n").unwrap();
        let after = walk(d.path(), &Config::default(), &manifest).unwrap();
        let diff = manifest.diff(&after);
        assert_eq!(diff.changed.iter().map(|e| e.rel.as_str()).collect::<Vec<_>>(), vec!["docs/new.md"]);
        assert!(diff.removed.is_empty());
    }

    #[test]
    fn diff_lists_a_removed_file_only_in_removed_not_changed() {
        let d = repo();
        let before = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        let manifest = Manifest::from_entries(&before);
        std::fs::remove_file(d.path().join("docs/a.md")).unwrap();
        let after = walk(d.path(), &Config::default(), &manifest).unwrap();
        let diff = manifest.diff(&after);
        assert_eq!(diff.removed, vec!["docs/a.md".to_string()]);
        assert!(diff.changed.is_empty());
    }

    #[test]
    fn a_file_matching_no_configured_glob_is_silently_excluded() {
        let d = tempfile::tempdir().unwrap();
        init(d.path());
        std::fs::write(d.path().join("data.json"), "{}\n").unwrap();
        let entries = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        assert!(entries.is_empty());
    }

    #[test]
    fn sorted_output_order_with_cyrillic_file_names() {
        let d = tempfile::tempdir().unwrap();
        init(d.path());
        std::fs::write(d.path().join("яблоко.md"), "x\n").unwrap();
        std::fs::write(d.path().join("абрикос.md"), "y\n").unwrap();
        std::fs::write(d.path().join("a.md"), "z\n").unwrap();
        let entries = walk(d.path(), &Config::default(), &Manifest::default()).unwrap();
        assert_eq!(entries.iter().map(|e| e.rel.as_str()).collect::<Vec<_>>(), vec!["a.md", "абрикос.md", "яблоко.md"]);
    }

    fn text_cfg() -> Config { Config { text_globs: vec!["**/*".into()], ..Config::default() } }

    #[test]
    fn the_store_is_not_walked_whatever_skip_says() {
        // No `.git` here: outside a checkout the store's own `.gitignore` is not read, so only the
        // walk itself keeps it out.
        let d = tempfile::tempdir().unwrap();
        let p = d.path();
        std::fs::write(p.join("ops.yaml"), "deploy: blue\n").unwrap();
        let store = p.join(crate::store::DIR);
        std::fs::create_dir_all(&store).unwrap();
        for (f, body) in [(".gitignore", "*\n"), ("manifest.json", "{}"), ("background.log", "changed 1\n")] {
            std::fs::write(store.join(f), body).unwrap();
        }
        for skip in [vec!["dist/**".to_string()], Vec::new()] {
            let cfg = Config { skip, ..text_cfg() };
            let rels: Vec<String> = walk(p, &cfg, &Manifest::default()).unwrap().into_iter().map(|e| e.rel).collect();
            assert_eq!(rels, ["ops.yaml"]);
        }
    }

    #[test]
    fn a_file_no_glob_claims_is_text_by_its_content_and_nothing_else_decides() {
        let d = repo();
        let p = d.path();
        std::fs::write(p.join("ops.yaml"), "deploy: blue\n").unwrap();
        std::fs::write(p.join("Makefile"), "all:\n\techo hi\n").unwrap();
        let mut early = vec![b'x'; 7_999];
        early.push(0);
        std::fs::write(p.join("notes.txt"), &early).unwrap();
        // `git`'s rule: only the first 8,000 bytes are looked at.
        let mut late = vec![b'x'; 8_000];
        late.extend_from_slice(b"\0tail");
        std::fs::write(p.join("late.txt"), &late).unwrap();
        std::fs::create_dir_all(p.join("web/vendor/x")).unwrap();
        std::fs::write(p.join("web/package-lock.json"), "{}\n").unwrap();
        std::fs::write(p.join("web/vendor/x/lib.txt"), "vendored\n").unwrap();
        std::fs::write(p.join("app.min.css"), "a{}\n").unwrap();
        std::fs::write(p.join("app.js.map"), "{}\n").unwrap();
        std::fs::write(p.join("big.log"), "x".repeat(TEXT_MAX_BYTES as usize + 1)).unwrap();
        let mut png = vec![0u8; TEXT_MAX_BYTES as usize + 1];
        png[..4].copy_from_slice(b"\x89PNG");
        std::fs::write(p.join("big.png"), &png).unwrap();
        let (entries, oversized) = walk_counted(p, &text_cfg(), &Manifest::default()).unwrap();
        let text: Vec<&str> = entries.iter().filter(|e| e.kind == FileKind::Text).map(|e| e.rel.as_str()).collect();
        assert_eq!(text, [".gitignore", "Makefile", "late.txt", "ops.yaml"]);
        assert_eq!(oversized, 1, "big.log alone: a binary over the limit is not text left out");
    }

    #[test]
    fn include_reads_only_the_directories_it_names() {
        let d = repo();
        let p = d.path();
        std::fs::create_dir_all(p.join("srcx")).unwrap();
        std::fs::write(p.join("srcx/c.ts"), "export const c = 1;\n").unwrap();
        let rels = |include: &[&str]| -> Vec<String> {
            let cfg = Config { include: include.iter().map(|s| s.to_string()).collect(), ..Config::default() };
            walk(p, &cfg, &Manifest::default()).unwrap().into_iter().map(|e| e.rel).collect()
        };
        let all = rels(&[]);
        assert!(all.contains(&"b.ts".to_string()) && all.contains(&"docs/a.md".to_string()));
        assert_eq!(rels(&["./docs/"]).iter().filter(|r| !r.starts_with("docs/")).count(), 0);
        assert!(rels(&["docs"]).contains(&"docs/a.md".to_string()));
        assert_eq!(rels(&["srcx/*.ts"]), ["srcx/c.ts"], "a glob is matched as it is written");
        assert_eq!(rels(&["src"]), Vec::<String>::new(), "src is not a prefix of srcx");
        assert_eq!(rels(&["sr?x"]), ["srcx/c.ts"], "a glob naming a directory reaches the files under it");
        assert_eq!(rels(&["/srcx"]), ["srcx/c.ts"], "a leading slash is the repository root");
        assert_eq!(rels(&["."]), all);
    }

    #[test]
    fn a_credential_file_is_never_read_whatever_the_globs_say() {
        let d = repo();
        let p = d.path();
        std::fs::create_dir_all(p.join("api/.aws")).unwrap();
        for f in [".env", ".env.local", "api/.env.production", "prod.env", ".envrc", "api/tls.pem",
                  "api/tls.key", "id_ed25519", ".npmrc", "api/.aws/credentials", "infra.tfvars",
                  "terraform.tfstate", "credentials-ci.json", "secrets.yaml", ".env.md", ".ENV", ".env-prod", ".env_ci",
                  "Prod.PEM", "ID_RSA", "main.tfvars.json", "app.secret", ".pgpass"] {
            std::fs::write(p.join(f), "API_KEY=sk-live-123\n").unwrap();
        }
        for f in [".env.example", "id_ed25519.pub"] {
            std::fs::write(p.join(f), "API_KEY=\n").unwrap();
        }
        let mut cfg = text_cfg();
        // A config that claims everything as docs and code still reads none of them.
        cfg.doc_globs.push("**/*".into());
        cfg.code_globs.push("**/*".into());
        let entries = walk(p, &cfg, &Manifest::default()).unwrap();
        let read: Vec<&str> = entries.iter().map(|e| e.rel.as_str())
            .filter(|r| { let r = r.to_lowercase(); ["env", "id_", "key", "cred", "secret", "tf", ".pem", "npmrc", "pgpass"].iter().any(|k| r.contains(k)) })
            .collect();
        assert_eq!(read, [".env.example", "id_ed25519.pub"]);
    }

    #[test]
    fn a_doc_a_code_file_a_registry_or_a_skipped_path_never_becomes_text() {
        let d = repo();
        let entries = walk(d.path(), &text_cfg(), &Manifest::default()).unwrap();
        let kind = |rel: &str| entries.iter().find(|e| e.rel == rel).map(|e| e.kind);
        assert_eq!(kind("b.ts"), Some(FileKind::Code));
        assert_eq!(kind("docs/a.md"), Some(FileKind::Doc));
        assert_eq!(kind("docs/constitution.yaml"), Some(FileKind::Registry));
        assert_eq!(kind("docs/TRACKER.md"), None);
        assert_eq!(kind("node_modules/x/c.ts"), None);
        assert_eq!(kind("ignored.md"), None);
    }

    #[test]
    fn a_file_a_glob_moves_to_another_reader_diffs_as_changed() {
        let d = repo();
        std::fs::write(d.path().join("tool.lua"), "print(1)\n").unwrap();
        let mut text = text_cfg();
        text.code_globs.retain(|g| g != "**/*.lua");
        let manifest = Manifest::from_entries(&walk(d.path(), &text, &Manifest::default()).unwrap());
        let after = walk(d.path(), &text_cfg(), &manifest).unwrap();
        let changed: Vec<_> = manifest.diff(&after).changed.into_iter().map(|e| (e.rel, e.kind)).collect();
        assert_eq!(changed, [("tool.lua".to_string(), FileKind::Code)]);
    }

    #[test]
    fn a_text_file_that_grows_past_the_limit_leaves_on_the_next_walk() {
        let d = repo();
        std::fs::write(d.path().join("ops.yaml"), "deploy: blue\n").unwrap();
        let manifest = Manifest::from_entries(&walk(d.path(), &text_cfg(), &Manifest::default()).unwrap());
        std::fs::write(d.path().join("ops.yaml"), "x".repeat(TEXT_MAX_BYTES as usize + 1)).unwrap();
        let after = walk(d.path(), &text_cfg(), &manifest).unwrap();
        assert_eq!(manifest.diff(&after).removed, vec!["ops.yaml".to_string()]);
    }
}
