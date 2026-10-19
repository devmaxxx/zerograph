pub mod cache;
pub mod cross;
pub mod dense;
pub mod embed;
pub mod fuse;
pub mod lexical;

use anyhow::Context as _;

/// `~/.cache/zerograph/<leaf>`: one of the caches this process keeps, the embedder's files and the
/// exported reranker. Read through the platform's idea of home rather than `$HOME`, which a Windows
/// console never sets; there it is the profile directory.
pub fn cache_dir(leaf: &str) -> anyhow::Result<std::path::PathBuf> {
    let home = std::env::home_dir().context("no home directory to keep the model cache under")?;
    Ok(home.join(".cache").join("zerograph").join(leaf))
}

#[cfg(test)]
mod tests {
    /// The cache is under whatever the platform calls home, which on a Windows console is not
    /// `$HOME`; on unix the two lookups agree, so this asserts the shape and, on Windows CI where
    /// `HOME` may be unset, that there is an answer at all.
    #[test]
    fn the_cache_is_under_the_home_the_platform_names() {
        let base = std::env::home_dir().expect("a home directory").join(".cache");
        assert_eq!(super::cache_dir("fastembed").unwrap(), base.join("zerograph/fastembed"));
    }
}
