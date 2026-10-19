//! Credentials written inside a file that is read. `walk` refuses the files that exist to hold
//! them (`.env`, `*.pem`); this catches the key pasted into a source file, a README or a shell
//! script, before its text reaches `graph.json`, the lexical and dense indexes, or a rerank prompt.

use aho_corasick::AhoCorasick;
use secrets_scanner::{RedactionMode, ScanConfig, Scanner};
use std::sync::OnceLock;

pub const MARKER: &str = "[REDACTED_SECRET]";

/// The bundled rules, split so a scanner can be built from any subset of them. Compiling all of
/// them costs 1.8 s, and the scanner already skips a rule whose keywords a text does not hold —
/// so a scanner of only the rules a text's keywords name finds exactly what the full one does.
struct Catalog {
    /// Everything but the rules: the global allowlist, the minimum version.
    base: toml::Table,
    rules: Vec<toml::Value>,
    keywords: AhoCorasick,
    rule_of_keyword: Vec<usize>,
    unkeyworded: Vec<usize>,
}

fn catalog() -> &'static Catalog {
    static C: OnceLock<Catalog> = OnceLock::new();
    C.get_or_init(|| {
        // The bundled rules only: `Scanner::new` would also read `$SECRETS_SCANNER_RULES` and a
        // cache in the user's data directory, so what is redacted would vary by machine.
        let mut base: toml::Table = toml::from_str(secrets_scanner::rules::BUNDLED_RULES).expect("the bundled rules parse");
        let rules = match base.remove("rules") { Some(toml::Value::Array(r)) => r, _ => Vec::new() };
        let (mut words, mut rule_of_keyword, mut unkeyworded) = (Vec::new(), Vec::new(), Vec::new());
        for (i, rule) in rules.iter().enumerate() {
            let listed: Vec<&str> = rule.get("keywords").and_then(|k| k.as_array()).into_iter().flatten().filter_map(|k| k.as_str()).collect();
            if listed.is_empty() { unkeyworded.push(i); }
            for word in listed { words.push(word.to_lowercase()); rule_of_keyword.push(i); }
        }
        let keywords = AhoCorasick::builder().ascii_case_insensitive(true).build(&words).expect("the rule keywords build an automaton");
        Catalog { base, rules, keywords, rule_of_keyword, unkeyworded }
    })
}

/// A scanner for one pass's texts, holding only the rules their keywords name.
pub struct Redactor(Option<Scanner>);

impl Redactor {
    pub fn for_texts<'a>(texts: impl IntoIterator<Item = &'a str>) -> Redactor {
        let c = catalog();
        let mut wanted = vec![false; c.rules.len()];
        for &i in &c.unkeyworded { wanted[i] = true; }
        for text in texts {
            for m in c.keywords.find_overlapping_iter(text) { wanted[c.rule_of_keyword[m.pattern().as_usize()]] = true; }
        }
        let rules: Vec<toml::Value> = c.rules.iter().zip(&wanted).filter(|(_, w)| **w).map(|(r, _)| r.clone()).collect();
        if rules.is_empty() {
            return Redactor(None);
        }
        let mut subset = c.base.clone();
        subset.insert("rules".into(), toml::Value::Array(rules));
        let toml = toml::to_string(&subset).expect("a subset of the bundled rules serializes");
        let scanner = Scanner::from_toml(&toml).expect("a subset of the bundled rules compiles");
        Redactor(Some(scanner.with_config(ScanConfig {
            redaction_mode: RedactionMode::Full,
            // A repository is content someone else wrote: a `gitleaks:allow` beside a key must not
            // be what lets it into the graph.
            honor_allow_markers: false,
            capture_context: false,
            ..ScanConfig::default()
        })))
    }

    /// `text` with every detected secret replaced by [`MARKER`], and how many were found. A secret
    /// spanning lines keeps its newlines after the marker, so every line below it keeps its number.
    pub fn redact(&self, rel: &str, text: &str) -> (String, usize) {
        let Some(scanner) = &self.0 else { return (text.to_string(), 0) };
        redact_found(text, &scanner.scan_content_detailed(rel, text).findings)
    }
}

fn redact_found(text: &str, found: &[secrets_scanner::Finding]) -> (String, usize) {
    let mut spans: Vec<(usize, usize)> = found.iter()
        .map(|f| (f.secret_start_offset, f.secret_end_offset))
        .filter(|(s, e)| s < e && *e <= text.len())
        .map(|(s, e)| (floor_boundary(text, s), ceil_boundary(text, e)))
        .collect();
    if spans.is_empty() {
        return (text.to_string(), 0);
    }
    spans.sort_unstable();
    // Two rules often match one key: overlapping or touching spans become one marker.
    let mut merged: Vec<(usize, usize)> = Vec::with_capacity(spans.len());
    for (start, end) in spans {
        match merged.last_mut() {
            Some(last) if start <= last.1 => last.1 = last.1.max(end),
            _ => merged.push((start, end)),
        }
    }
    let mut out = String::with_capacity(text.len());
    let mut cursor = 0;
    for (start, end) in merged {
        out.push_str(&text[cursor..start]);
        out.push_str(MARKER);
        out.extend(text[start..end].chars().filter(|&c| c == '\n'));
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    (out, found.len())
}

fn floor_boundary(text: &str, mut i: usize) -> usize {
    while !text.is_char_boundary(i) { i -= 1; }
    i
}

fn ceil_boundary(text: &str, mut i: usize) -> usize {
    while !text.is_char_boundary(i) { i += 1; }
    i
}

#[cfg(test)]
mod tests {
    use super::{Redactor, MARKER};

    fn redact(rel: &str, text: &str) -> (String, usize) {
        Redactor::for_texts([text]).redact(rel, text)
    }

    // Assembled at run time so this file holds no string a scanner of this repository flags.
    fn github_token() -> String { format!("ghp_{}", "R7xK2mQ9vT4wL8nB3cF6hJ1pS5dG0aZyEuXo") }
    fn aws_key() -> String { format!("AKIA{}", "QX4TZ7WMB2KD6RLN") }

    #[test]
    fn a_token_in_source_is_replaced_and_the_rest_of_the_line_stays() {
        let text = format!("const client = new Octokit({{ auth: \"{}\" }});\nexport default client;\n", github_token());
        let (out, n) = redact("src/github.ts", &text);
        assert_eq!(n, 1);
        assert!(!out.contains(&github_token()), "{out}");
        assert!(out.contains(MARKER) && out.contains("new Octokit") && out.contains("export default client;"), "{out}");
    }

    #[test]
    fn a_key_exported_in_a_shell_script_is_replaced() {
        let text = format!("#!/bin/sh\nexport AWS_ACCESS_KEY_ID={}\naws s3 ls\n", aws_key());
        let (out, n) = redact("scripts/deploy.sh", &text);
        assert!(n >= 1);
        assert!(!out.contains(&aws_key()), "{out}");
        assert!(out.contains("aws s3 ls"));
    }

    #[test]
    fn a_private_key_block_is_replaced_and_the_lines_below_keep_their_numbers() {
        let body = "MIIEowIBAAKCAQEA0Z3VS5JJcds3xfn/ygWyF8PbnGy0AHB7MhgHcTz6sE2I2yPB\naFDrBz9vFqU4yQmpQ6fFoXyZ0YkRh4AXKzk7b1HgGzK7v1mN3rGv5WqL8XKjR2Ys\n";
        let text = format!("# Ops\n\n-----BEGIN RSA {k}-----\n{body}-----END RSA {k}-----\n\nnext line\n", k = "PRIVATE KEY");
        let (out, n) = redact("docs/ops.md", &text);
        assert!(n >= 1);
        assert!(!out.contains("MIIEowIBAAKCAQEA"), "{out}");
        assert_eq!(out.lines().count(), text.lines().count(), "{out}");
        assert_eq!(out.lines().position(|l| l == "next line"), text.lines().position(|l| l == "next line"));
    }

    #[test]
    fn an_allow_marker_in_the_file_does_not_let_a_secret_through() {
        let text = format!("token = \"{}\" # gitleaks:allow\n", github_token());
        let (out, _) = redact("config.py", &text);
        assert!(!out.contains(&github_token()), "{out}");
    }

    #[test]
    fn text_without_a_secret_comes_back_unchanged() {
        let text = "fn main() {\n    let password = read_password();\n    println!(\"héllo\");\n}\n";
        assert_eq!(redact("src/main.rs", text), (text.to_string(), 0));
    }

    /// The subset is only an optimisation if it finds what the whole ruleset finds, on real text
    /// that names many providers' keywords: this repository's own sources and documents.
    #[test]
    fn a_scanner_of_the_rules_a_text_names_finds_what_the_whole_ruleset_finds() {
        let full = secrets_scanner::Scanner::from_toml(secrets_scanner::rules::BUNDLED_RULES).expect("the whole ruleset passes the strict gate");
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let spans = |f: Vec<secrets_scanner::Finding>| { let mut s: Vec<_> = f.into_iter().map(|f| (f.rule_id, f.secret_start_offset, f.secret_end_offset)).collect(); s.sort(); s };
        let mut paths: Vec<_> = ignore::Walk::new(root).flatten().map(|e| e.into_path())
            .filter(|p| p.extension().is_some_and(|x| x == "rs" || x == "md")).collect();
        paths.sort();
        let (mut compared, mut flagged) = (0, 0);
        for path in &paths {
            let Ok(text) = std::fs::read_to_string(path) else { continue };
            let rel = path.strip_prefix(root).unwrap().to_string_lossy();
            let expected = spans(full.scan_content_detailed(&rel, &text).findings);
            let Redactor(Some(subset)) = Redactor::for_texts([text.as_str()]) else { panic!("no rule at all for {rel}") };
            assert_eq!(spans(subset.scan_content_detailed(&rel, &text).findings), expected, "{rel}");
            compared += 1;
            flagged += usize::from(!expected.is_empty());
        }
        assert!(compared > 100 && flagged > 0, "compared {compared} files, {flagged} flagged");
    }
}
