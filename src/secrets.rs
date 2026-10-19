//! Credentials written inside a file that is read. `walk` refuses the files that exist to hold
//! them (`.env`, `*.pem`); this catches the key pasted into a source file, a README or a shell
//! script, before its text reaches `graph.json`, the lexical and dense indexes, or a rerank prompt.
//!
//! Coverage is by provider pattern: a token whose own shape names who issued it, a private key
//! block, a JWT, a database URL with a password in it. The catch-all rules that guess from the
//! word written beside a value are left out, so `password = "hunter2"` is not caught — on a real
//! repository they flagged 53 spans and none was a secret, while one of them cost a requirement
//! its citation.

use aho_corasick::AhoCorasick;
use secrets_scanner::{RedactionMode, ScanConfig, Scanner};
use std::sync::OnceLock;

pub const MARKER: &str = "[REDACTED_SECRET]";

/// How much file text is held before it is scanned for secrets: one scanner is compiled per batch,
/// and every text of a tree at once would be the largest allocation of a build.
pub const BATCH_BYTES: usize = 64 << 20;

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
        let rules: Vec<toml::Value> = match base.remove("rules") { Some(toml::Value::Array(r)) => r, _ => Vec::new() };
        let rules: Vec<toml::Value> = rules.into_iter().filter(|r| r.get("id").and_then(|i| i.as_str()).is_some_and(|id| !is_catch_all(id))).collect();
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

/// Rules that match a value by the word beside it — `key`, `password`, `Authorization`, a URL's
/// `user:pass@` on any host — rather than by a shape a provider issues. On the bench fixture
/// they read `key: 'visit.cancel'` and a type parameter as keys, and one ran across lines into
/// the code below it.
fn is_catch_all(id: &str) -> bool {
    const RULES: &[&str] = &[
        "generic-api-key",
        "hashicorp-tf-password",
        "curl-auth-header",
        "curl-auth-user",
        "kingfisher.credentials.1",
        "kingfisher.uri.1",
    ];
    const FAMILIES: &[&str] = &["kingfisher.generic.", "kingfisher.http.", "kingfisher.auth.", "kingfisher.curl."];
    RULES.contains(&id) || FAMILIES.iter().any(|f| id.starts_with(f))
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

    /// A redactor that changes nothing, for a project whose `zerograph.toml` says
    /// `redact_secrets = false`.
    pub fn off() -> Redactor { Redactor(None) }
}

fn redact_found(text: &str, found: &[secrets_scanner::Finding]) -> (String, usize) {
    redact_spans(text, found.iter().map(|f| (f.secret_start_offset, f.secret_end_offset, is_database_url(&f.rule_id))).collect())
}

/// Rules that match a database connection URL by its scheme, whatever the password in it. Only
/// these may be let through for a local host or a placeholder password: a provider's token in
/// the same place is still matched by its own rule, and that span is never let through.
fn is_database_url(id: &str) -> bool {
    const FAMILIES: &[&str] = &[
        "kingfisher.postgres.", "kingfisher.mysql.", "kingfisher.mariadb.", "kingfisher.mongodb.", "kingfisher.redis.",
        "kingfisher.rabbitmq.", "kingfisher.jdbc.", "kingfisher.mssql.", "kingfisher.db2.", "kingfisher.odbc.",
    ];
    const RULES: &[&str] = &["mongodb-atlas-db-uri-with-credentials", "snowflake-postgres-connection-string"];
    RULES.contains(&id) || FAMILIES.iter().any(|f| id.starts_with(f))
}

/// Spans as `(start, end, from a database URL rule)`.
fn redact_spans(text: &str, found: Vec<(usize, usize, bool)>) -> (String, usize) {
    let mut spans: Vec<(usize, usize, bool)> = found.into_iter()
        .filter(|(s, e, _)| s < e && *e <= text.len())
        .map(|(s, e, db)| (floor_boundary(text, s), ceil_boundary(text, e), db))
        .collect();
    if spans.is_empty() {
        return (text.to_string(), 0);
    }
    // An id is what the graph is made of: a span holding one would take the node or the edge
    // with it, and no provider issues a token shaped like `FR-CAL-155`.
    let ids: Vec<(usize, usize)> = crate::ids::generic().find_all(text).into_iter().map(|h| (h.start, h.start + h.id.len())).collect();
    spans.retain(|&(s, e, db)| !ids.iter().any(|&(a, b)| a < e && s < b) && !(db && local_or_placeholder(text, s, e)));
    let mut spans: Vec<(usize, usize)> = spans.into_iter().map(|(s, e, _)| (s, e)).collect();
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
    let count = merged.len();
    for (start, end) in merged {
        out.push_str(&text[cursor..start]);
        out.push_str(MARKER);
        out.extend(text[start..end].chars().filter(|&c| c == '\n'));
        cursor = end;
    }
    out.push_str(&text[cursor..]);
    (out, count)
}

/// Whether the span sits in a connection URL that opens nothing outside the reader's machine: a
/// loopback host, or a password that is an interpolation or a mask. CI files, compose files and
/// `.env.example` write these by the dozen, and every one of them is meant to be read.
fn local_or_placeholder(text: &str, start: usize, end: usize) -> bool {
    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
    let Some(scheme_end) = text[line_start..end].rfind("://").map(|i| line_start + i + 3) else { return false };
    let authority_len = text[scheme_end..].find(|c: char| c.is_whitespace() || matches!(c, '/' | '"' | '\'' | '`' | '?' | '#')).unwrap_or(text.len() - scheme_end);
    let authority = &text[scheme_end..scheme_end + authority_len];
    if start >= scheme_end + authority_len { return false; }
    let Some((userinfo, host)) = authority.rsplit_once('@') else { return false };
    let Some((_, password)) = userinfo.split_once(':') else { return false };
    let host = match host.strip_prefix('[') {
        Some(v6) => v6.split(']').next().unwrap_or(""),
        None => host.split(':').next().unwrap_or(""),
    };
    let loopback = host.eq_ignore_ascii_case("localhost")
        || host.parse::<std::net::IpAddr>().is_ok_and(|ip| ip.is_loopback() || ip.is_unspecified());
    loopback || placeholder(password)
}

/// A password that names where the real one comes from, or masks it, rather than being one. The
/// whole password has to be the shape: `$Xk29…` is a password that happens to start with `$`.
fn placeholder(password: &str) -> bool {
    static SHAPES: OnceLock<regex::Regex> = OnceLock::new();
    let shapes = SHAPES.get_or_init(|| regex::Regex::new(concat!(
        r"^(?:\$\{[^{}\s]+\}",       // ${VAR}, ${VAR:-default}
        r"|\$[A-Z_][A-Z0-9_]*",       // $VAR, upper case as environment variables are named
        r"|%[A-Za-z_][A-Za-z0-9_]*%",  // %VAR%
        r"|\{\{[^{}]+\}\}",           // {{ template }}
        r"|<[^<>]+>",                 // <password>
        r"|\*{3,}|[xX]{3,}|\.{3,})$",  // ***, xxx, ...
    )).expect("the placeholder shapes compile"));
    shapes.is_match(password)
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

    /// What the catch-all rules read as keys on the bench fixture, verbatim: none is a secret, and
    /// the first cost `FR-CAL-155` its citation.
    #[test]
    fn the_catch_all_false_positives_of_the_fixture_come_back_unchanged() {
        for (rel, text) in [
            ("AvailabilityRules.kt", " * Against `when.now`, which is the whole reason that key exists: `FR-CAL-155` makes this a pure\n"),
            ("past-time.yaml", "# The comparison is against `when.now`, which is the whole reason that key exists: FR-CAL-155\n"),
            ("rbac.repository.ts", "  const scoped = await tx.query<{ key: PermissionKey }>(\n    `SELECT key FROM permissions`,\n  );\n"),
            ("permissions.ts", "  {\n    key: 'visit.cancel',\n    description: 'отменить визит',\n  },\n"),
            ("connection.spec.ts", concat!(
                "    const { username, password } = new URL(DEV_MIGRATOR_CONNECTION_STRING);\n",
                "    expect({ username, password }).toEqual({ username, password: roles.get(username) });\n",
                "    const { username, password } = new URL(DEV_SYSTEM_CONNECTION_STRING);\n",
            )),
            ("config.md", "  password: process.env.DB_PASSWORD,\n        password: config.get('database.password'),\n"),
        ] {
            assert_eq!(redact(rel, text), (text.to_string(), 0), "{rel}");
        }
    }

    #[test]
    fn a_span_holding_an_id_is_never_redacted() {
        let text = format!("token {} FR-CAL-155\n", github_token());
        assert_eq!(super::redact_spans(&text, vec![(6, text.len() - 1, false)]), (text.clone(), 0));
        assert_eq!(super::redact_spans(&text, vec![(6, 6 + github_token().len(), false)]).1, 1, "the same token without the id beside it goes");
    }

    #[test]
    fn a_database_url_is_redacted_unless_it_opens_only_the_readers_own_machine() {
        let password = "Xk29vLq7Rm4t";
        let remote = format!("DATABASE_URL=postgres://app:{password}@db.prod.example.net:5432/crm\n");
        let (out, n) = redact("deploy.sh", &remote);
        assert_eq!(n, 1, "{out}");
        assert!(!out.contains(password), "{out}");
        for local in [
            "DATABASE_URL_SALON=postgres://salon_ctx:salon_ctx@localhost:5432/beauty_crm\n",
            "url: 'postgres://app_system:app_system@127.0.0.1:5432/beauty_crm',\n",
            "      DATABASE_URL_SALON: postgres://salon_ctx:${SALON_CTX_PASSWORD:-salon_ctx}@postgres:5432/beauty_crm\n",
            "db:migrate: chain applied on postgres://app_migrator:***@db.prod.example.net:55440/beauty_crm\n",
        ] {
            assert_eq!(redact("ci.yml", local), (local.to_string(), 0), "{local}");
        }
    }

    #[test]
    fn a_provider_token_in_a_local_or_masked_url_is_still_redacted() {
        for text in [
            format!("git clone https://x-access-token:{}@localhost:3000/org/repo.git\n", github_token()),
            format!("BILLING_URL=https://api:{}@127.0.0.1:8080/v1/charges\n", aws_key()),
            format!("DATABASE_URL=postgres://app:{}@localhost:5432/crm\n", github_token()),
        ] {
            let (out, n) = redact("ops.sh", &text);
            assert!(n >= 1 && !out.contains(&github_token()) && !out.contains(&aws_key()), "{out}");
        }
    }

    #[test]
    fn only_a_real_loopback_host_or_a_whole_placeholder_lets_a_database_url_through() {
        let scheme = format!("{}://", "postgres");
        for (url, kept) in [
            (format!("{scheme}app:Xk29vLq7Rm4t@127.0.0.1:5432/crm"), true),
            (format!("{scheme}app:Xk29vLq7Rm4t@127.8.9.10:5432/crm"), true),
            (format!("{scheme}app:Xk29vLq7Rm4t@[::1]:5432/crm"), true),
            (format!("{scheme}app:Xk29vLq7Rm4t@LOCALHOST:5432/crm"), true),
            (format!("{scheme}app:Xk29vLq7Rm4t@127.attacker.example.net:5432/crm"), false),
            (format!("{scheme}app:Xk29vLq7Rm4t@localhost.example.net:5432/crm"), false),
            (format!("{scheme}app:$DB_PASSWORD@db.prod.example.net:5432/crm"), true),
            (format!("{scheme}app:%DB_PASSWORD%@db.prod.example.net:5432/crm"), true),
            (format!("{scheme}app:<password>@db.prod.example.net:5432/crm"), true),
            (format!("{scheme}app:xxxx@db.prod.example.net:5432/crm"), true),
            (format!("{scheme}app:$Xk29vLq7Rm4t@db.prod.example.net:5432/crm"), false),
            (format!("{scheme}app:{{Xk29vLq7Rm4t@db.prod.example.net:5432/crm"), false),
            (format!("{scheme}app:***Xk29vLq7Rm4t@db.prod.example.net:5432/crm"), false),
        ] {
            let text = format!("DATABASE_URL={url}\n");
            let (out, n) = redact("deploy.sh", &text);
            assert_eq!(n == 0, kept, "{url} -> {out}");
        }
    }

    /// The subset is only an optimisation if it finds what the whole ruleset finds, on real text
    /// that names many providers' keywords: this repository's own sources and documents.
    #[test]
    fn a_scanner_of_the_rules_a_text_names_finds_what_the_whole_ruleset_finds() {
        let c = super::catalog();
        let mut whole = c.base.clone();
        whole.insert("rules".into(), toml::Value::Array(c.rules.clone()));
        let full = secrets_scanner::Scanner::from_toml(&toml::to_string(&whole).unwrap()).expect("the provider ruleset passes the strict gate");
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
