//! The other worktrees of the same repository, whose stores may already hold the rows a sync is
//! about to embed. A vector depends on the model and the row's text alone, and a cold build is
//! almost all embedding, so a second checkout of the same code can borrow nearly every row.
//!
//! A sibling's store is someone else's: it is read and never written — not even its lock file is
//! created — and it is read only while its writer lock is held here, so a pair being rewritten is
//! never read torn. Anything that stops a sibling from being read just means it lends nothing.

use super::dense::DenseIndex;
use crate::store::{self, Store};
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

/// The sibling's rows as they stand, or `None` when its writer lock is held, its store or lock
/// file is missing or a link, its pair does not read, or its metadata names a model other than
/// `model`. The lock is released as soon as the read is done, which is what keeps a borrow from
/// stalling the sibling's own refresh.
pub(crate) fn read(sibling: &Path, model: &str) -> Option<DenseIndex> {
    let dir = store::dir_of(sibling);
    if !std::fs::symlink_metadata(&dir).is_ok_and(|m| m.is_dir()) { return None; }
    let lock = dir.join("writer.lock");
    if !std::fs::symlink_metadata(&lock).is_ok_and(|m| m.is_file()) { return None; }
    let f = store::open_unlinked(std::fs::OpenOptions::new().read(true), &lock).ok()?;
    f.try_lock().ok()?;
    let rows = read_locked(&dir, &Store::new(sibling), model);
    drop(f);
    rows
}

fn read_locked(dir: &Path, store: &Store, model: &str) -> Option<DenseIndex> {
    // A sibling built by an older release may have saved without the writer lock, and a save
    // renames the rows in before the metadata: rows newer than their metadata are a save caught
    // halfway, whose offsets the metadata may no longer describe.
    let written = || -> Option<_> {
        let at = |name: &str| std::fs::metadata(dir.join(name)).and_then(|m| m.modified()).ok();
        Some((at("vectors.f32")?, at("vectors.json")?))
    };
    let before = written()?;
    if before.0 > before.1 { return None; }
    // The model first, from the metadata alone: a sibling of another model is not worth its rows'
    // read, nor the time its lock is held for it.
    if !DenseIndex::recorded_model(store).ok()??.eq_ignore_ascii_case(model) { return None; }
    let rows = DenseIndex::load(store).ok()?;
    (written()? == before).then_some(rows)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn git(dir: &Path, args: &[&str]) {
        let out = std::process::Command::new("git").arg("-C").arg(dir)
            .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").env_remove("GIT_INDEX_FILE")
            .env_remove("GIT_COMMON_DIR").env_remove("GIT_OBJECT_DIRECTORY")
            .env_remove("GIT_ALTERNATE_OBJECT_DIRECTORIES")
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
