//! The other worktrees of the same repository, whose stores may already hold the rows a sync is
//! about to embed. A vector depends on the model and the row's text alone, and a cold build is
//! almost all embedding, so a second checkout of the same code can borrow nearly every row.
//!
//! A sibling's store is someone else's: it is read and never written — not even its lock file is
//! created — and it is read only while its writer lock is held here, so a pair being rewritten is
//! never read torn. Anything that stops a sibling from being read just means it lends nothing.

use super::dense::DenseIndex;
use crate::store::{self, Store};
use crate::walk::Stamp;
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Every other worktree of `repo`'s repository whose store holds vectors, in the order git lists
/// them. Not a repository, or no git at all, is no sibling.
pub(crate) fn worktrees(repo: &Path) -> Vec<PathBuf> {
    let Ok(listed) = crate::changes::git(repo, &["worktree", "list", "--porcelain"]) else { return Vec::new() };
    let me = std::fs::canonicalize(repo).ok();
    listed.lines()
        .filter_map(|l| l.strip_prefix("worktree "))
        .map(PathBuf::from)
        .filter(|p| std::fs::canonicalize(p).ok().is_some_and(|c| Some(c) != me))
        .filter(|p| store::dir_of(p).join("vectors.json").is_file())
        .collect()
}

/// What a sibling's store had for a borrow.
pub(crate) enum Lend {
    /// Its rows, read whole: its metadata named at least one of the wanted hashes.
    Rows(Box<DenseIndex>),
    /// It holds none of the wanted rows, or rows of another model: told by its metadata alone,
    /// and remembered against its rows' stamp.
    Miss,
    /// It could not be read this time: its lock held or missing, its store or lock file a link,
    /// its pair torn or unreadable. Nothing is remembered, since the next try may read.
    Unread,
}

/// Sibling → the stamp its `vectors.f32` had and the hashes it was found not to hold then. A fresh
/// edit's rows are in no sibling, and a resident process asks for them on every answer until they
/// are embedded; rows that have not changed cannot have gained them. Held for the process's life.
type Misses = std::collections::HashMap<PathBuf, (Stamp, HashSet<String>)>;
static MISSES: std::sync::LazyLock<std::sync::Mutex<Misses>> = std::sync::LazyLock::new(Default::default);

/// The sibling's rows when they hold any of the `wanted` text hashes. Its writer lock is taken
/// without waiting and released as soon as the read is done, which is what keeps a borrow from
/// stalling the sibling's own refresh; no lock file is created to read under.
pub(crate) fn read(sibling: &Path, model: &str, wanted: &[&str]) -> Lend {
    let dir = store::dir_of(sibling);
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) { return Lend::Unread; }
    // Taken before the read, so a save racing it leaves a stamp the next look will not match.
    let Some(rows_at) = std::fs::metadata(dir.join("vectors.f32")).ok().and_then(|m| crate::walk::stamp_of(&m)) else { return Lend::Unread };
    if missed(sibling, rows_at, wanted) { return Lend::Miss; }
    let lock = dir.join("writer.lock");
    if !std::fs::symlink_metadata(&lock).is_ok_and(|m| m.is_file()) { return Lend::Unread; }
    let Ok(f) = store::open_unlinked(std::fs::OpenOptions::new().read(true), &lock) else { return Lend::Unread };
    if f.try_lock().is_err() { return Lend::Unread; }
    let lent = read_locked(&dir, &Store::new(sibling), model, wanted);
    drop(f);
    if matches!(lent, Lend::Miss) { remember_miss(sibling, rows_at, wanted); }
    lent
}

fn missed(sibling: &Path, rows_at: Stamp, wanted: &[&str]) -> bool {
    let misses = MISSES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    misses.get(sibling).is_some_and(|(at, absent)| *at == rows_at && wanted.iter().all(|h| absent.contains(*h)))
}

fn remember_miss(sibling: &Path, rows_at: Stamp, wanted: &[&str]) {
    let mut misses = MISSES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    let entry = misses.entry(sibling.to_path_buf()).or_insert_with(|| (rows_at, HashSet::new()));
    if entry.0 != rows_at { *entry = (rows_at, HashSet::new()); }
    entry.1.extend(wanted.iter().map(|h| h.to_string()));
}

fn read_locked(dir: &Path, store: &Store, model: &str, wanted: &[&str]) -> Lend {
    // A sibling built by an older release may have saved without the writer lock, and a save
    // renames the rows in before the metadata: rows newer than their metadata are a save caught
    // halfway, whose offsets the metadata may no longer describe.
    let written = || -> Option<_> {
        let at = |name: &str| std::fs::metadata(dir.join(name)).and_then(|m| m.modified()).ok();
        Some((at("vectors.f32")?, at("vectors.json")?))
    };
    let Some(before) = written() else { return Lend::Unread };
    if before.0 > before.1 { return Lend::Unread; }
    // The metadata first: a sibling of another model, or one holding none of the wanted rows, is
    // not worth its rows' read, nor the time its lock is held for it.
    let Some(holds) = DenseIndex::holds_any(store, model, wanted) else { return Lend::Unread };
    if !holds { return Lend::Miss; }
    let Ok(rows) = DenseIndex::load(store) else { return Lend::Unread };
    match written() == Some(before) { true => Lend::Rows(Box::new(rows)), false => Lend::Unread }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn git(dir: &Path, args: &[&str]) {
        let out = crate::git_command().arg("-C").arg(dir)
            .args(["-c", "user.name=t", "-c", "user.email=t@t", "-c", "commit.gpgsign=false", "-c", "core.hooksPath=/dev/null"])
            .args(args).output().unwrap();
        assert!(out.status.success(), "git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }

    /// A repository with one commit and a second worktree beside it, both under one temp dir.
    pub(crate) fn two_worktrees() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let main = root.path().join("main");
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q"]);
        std::fs::write(main.join("a.md"), "a\n").unwrap();
        git(&main, &["add", "a.md"]);
        git(&main, &["commit", "-q", "-m", "a"]);
        let other = root.path().join("other");
        git(&main, &["worktree", "add", "-q", other.to_str().unwrap()]);
        (root, main, other)
    }

    fn vectors_in(repo: &Path) {
        let dir = repo.join(".zerograph");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("vectors.json"), "{}").unwrap();
    }

    #[test]
    fn a_sibling_is_listed_once_it_holds_vectors_and_never_the_worktree_asking() {
        let (_root, main, other) = two_worktrees();
        assert!(worktrees(&other).is_empty(), "the main checkout has no store yet");
        vectors_in(&main);
        vectors_in(&other);
        let listed: Vec<PathBuf> = worktrees(&other).iter().map(|p| std::fs::canonicalize(p).unwrap()).collect();
        assert_eq!(listed, vec![std::fs::canonicalize(&main).unwrap()]);
    }

    #[test]
    fn a_directory_outside_any_repository_has_no_siblings() {
        let dir = tempfile::tempdir().unwrap();
        assert!(worktrees(dir.path()).is_empty());
    }
}
