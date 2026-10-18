pub mod cache;
pub mod cross;
pub mod dense;
pub mod embed;
pub mod fuse;
pub mod lexical;

use anyhow::Context as _;

/// `~/.cache/zerograph/<leaf>`, or `~/.cache/repograph/<leaf>` while that is the only one there:
/// one of the caches this process keeps, the embedder's files and the exported reranker. Read
/// through the platform's idea of home rather than `$HOME`, which a Windows console never sets;
/// there it is the profile directory.
pub fn cache_dir(leaf: &str) -> anyhow::Result<std::path::PathBuf> {
    let home = std::env::home_dir().context("no home directory to keep the model cache under")?;
    Ok(cache_under(&home.join(".cache"), leaf))
}

/// A cache from before the rename is used where it stands for as long as there is no new one: the
/// embedder's holds a model of over a gigabyte, and a rename should not cost a download.
fn cache_under(base: &std::path::Path, leaf: &str) -> std::path::PathBuf {
    let new = base.join("zerograph").join(leaf);
    let old = base.join("repograph").join(leaf);
    if !new.exists() && old.is_dir() { old } else { new }
}

#[cfg(test)]
mod tests {
    /// The cache is under whatever the platform calls home, which on a Windows console is not
    /// `$HOME`; on unix the two lookups agree, so this asserts the shape and, on Windows CI where
    /// `HOME` may be unset, that there is an answer at all.
    #[test]
    fn the_cache_is_under_the_home_the_platform_names() {
        let base = std::env::home_dir().expect("a home directory").join(".cache");
        let got = super::cache_dir("fastembed").unwrap();
        assert!(got == base.join("zerograph/fastembed") || got == base.join("repograph/fastembed"), "{}", got.display());
    }

    #[test]
    fn a_cache_from_before_the_rename_is_used_until_a_new_one_exists() {
        let d = tempfile::tempdir().unwrap();
        let under = |leaf| super::cache_under(d.path(), leaf);
        assert_eq!(under("fastembed"), d.path().join("zerograph/fastembed"), "neither: the new name");
        std::fs::create_dir_all(d.path().join("repograph/fastembed")).unwrap();
        assert_eq!(under("fastembed"), d.path().join("repograph/fastembed"), "only the old one: no second download");
        assert_eq!(under("reranker"), d.path().join("zerograph/reranker"), "per cache, not per root");
        std::fs::create_dir_all(d.path().join("zerograph/fastembed")).unwrap();
        assert_eq!(under("fastembed"), d.path().join("zerograph/fastembed"), "both: the new one");
    }
}
