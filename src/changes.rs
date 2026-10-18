//! The working tree's diff, read onto the graph: which symbols the hunks sit in, and who reaches
//! those. Line numbers come from the new side of `git diff -U0`, so the graph must have been
//! refreshed against the working tree first — `main` does that before calling in.
use crate::code::spec::CommentSyntax;
use crate::impact::{self, Dependent};
use crate::model::{Graph, NodeKind};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hunk { pub file: String, pub start: u32, pub end: u32, pub comment_only: bool }

impl Hunk {
    pub fn code(file: impl Into<String>, start: u32, end: u32) -> Hunk {
        Hunk { file: file.into(), start, end, comment_only: false }
    }
}

/// New-side ranges of a zero-context unified diff. A pure deletion (`+c,0`) has no new lines;
/// it is recorded as the line the cut lands on, so the symbol around it still counts as changed.
pub fn parse(diff: &str) -> Vec<Hunk> {
    let mut out: Vec<Hunk> = Vec::new();
    let mut file: Option<String> = None;
    let mut syntax: Option<CommentSyntax> = None;
    // Header lines only come between `diff --git` and the first `@@`, and a removed SQL comment
    // reads `--- note` inside a hunk, so a line is content only once a hunk has opened.
    let mut in_hunk = false;
    // A hunk with no body line to judge, whatever the diff holds instead, is walked as code.
    let mut judged = false;
    for line in diff.lines() {
        if line.starts_with("diff ") { in_hunk = false; continue; }
        if in_hunk && !line.starts_with("@@ ") {
            if let (Some(body), Some(h)) = (line.strip_prefix(['+', '-']), out.last_mut()) {
                h.comment_only = (h.comment_only || !judged) && syntax.is_some_and(|s| is_comment(s, body));
                judged = true;
            }
            continue;
        }
        if let Some(p) = line.strip_prefix("+++ ") {
            file = p.strip_prefix("b/").map(str::to_string);
            continue;
        }
        let Some(rest) = line.strip_prefix("@@ ") else { continue };
        // A header that cannot be read opens no hunk, and its body must not be judged into the last one.
        in_hunk = false;
        let Some(f) = &file else { continue };
        let Some(plus) = rest.split_whitespace().find(|w| w.starts_with('+')) else { continue };
        let (c, d) = match plus[1..].split_once(',') {
            Some((c, d)) => (c.parse::<u32>().unwrap_or(0), d.parse::<u32>().unwrap_or(1)),
            None => (plus[1..].parse::<u32>().unwrap_or(0), 1),
        };
        let (start, end) = if d == 0 { (c.max(1), c.max(1)) } else { (c, c + d - 1) };
        out.push(Hunk::code(f.clone(), start, end));
        syntax = comment_syntax(f);
        judged = false;
        in_hunk = true;
    }
    out
}

/// `None` for markup around a host language, where whether `//` opens a comment depends on where the
/// line sits, and for a file with no grammar, which has no symbol nodes for its judgement to spare.
fn comment_syntax(file: &str) -> Option<CommentSyntax> {
    crate::code::lang::Lang::of(file).and_then(|lang| crate::code::spec::spec(lang).comments)
}

// A comment that steers a tool changes what the file does even though it declares nothing.
const DIRECTIVES: &[&str] = &[
    "@ts-", "eslint", "prettier-ignore", "biome-ignore", "istanbul", "c8 ignore", "@flow", "@jsx",
    "noqa", "type: ignore", "pyright:", "pylint:", "mypy:", "shellcheck", "tflint-ignore", "checkov:",
    "v8 ignore", "@vitest", "@jest", "deno-lint", "/// <", "pragma", "nosec", "fmt:", "isort:", "-*-",
    "swiftlint", "ktlint", "detekt", "nopmd", "nosonar", "checkstyle", "ignore:", "ignore_for_file",
    "coverage:ignore", "dart format", "tfsec", "trivy", "+goose", "migrate:", "-- name:", "# import ",
    "/*!", "/*+",
];

/// A blank line, or a comment that holds no code and steers no tool. Anything it cannot be sure of
/// is code: reading code as a comment hides callers, reading a comment as code only walks wide. So
/// a block comment's `* body` line is code here, because a wrapped `* rate` looks the same.
fn is_comment(syntax: CommentSyntax, body: &str) -> bool {
    let t = body.trim();
    if t.is_empty() { return true }
    let lower = t.to_ascii_lowercase();
    if t.starts_with("#!") || DIRECTIVES.iter().any(|d| lower.contains(d)) { return false }
    if syntax.lines.iter().any(|m| t.starts_with(m)) { return true }
    if !syntax.blocks { return false }
    if t == "*" || t == "*/" { return true }
    // `/* note */` is a comment, `/* note */ call();` is a call.
    t.strip_prefix("/*").is_some_and(|rest| rest.split_once("*/").is_none_or(|(_, after)| after.trim().is_empty()))
}

/// Symbols whose span meets a hunk; a hunk outside every symbol falls to its file node. A class
/// whose member matched is dropped — the member is the change, the class only contains it.
/// Alongside, the files whose code outside every symbol changed.
fn touch<'h>(graph: &Graph, hunks: &'h [Hunk]) -> (Vec<String>, BTreeSet<&'h str>) {
    let mut out: BTreeSet<String> = BTreeSet::new();
    let mut code_outside: BTreeSet<&str> = BTreeSet::new();
    for h in hunks {
        let mut any = false;
        for n in graph.nodes.values().filter(|n| n.kind == NodeKind::Symbol && n.file == h.file && n.line > 0) {
            let end = n.end.max(n.line);
            if n.line <= h.end && end >= h.start { out.insert(n.id.clone()); any = true; }
        }
        if any { continue }
        // A file the graph never indexed — a `.kt`, a `.sql`, a lockfile — is still a file the
        // diff changed. Reported as its file id, so the answer says "this changed, I cannot say
        // which symbol" instead of saying nothing: on the bench corpus the silence was a third of
        // a large diff (2026-09-03, 11 of 18 files named). A deleted file never gets here — its
        // `+++ /dev/null` yields no hunk — so every id emitted is a file on the new side.
        out.insert(format!("file:{}", h.file));
        if !h.comment_only { code_outside.insert(&h.file); }
    }
    // Only symbol ids nest: `sym:f::C` contains `sym:f::C.m`. Two file ids that share a prefix
    // across a dot are unrelated files — `Dockerfile` and `Dockerfile.dev`, `index.d.ts` and
    // `index.d.ts.map` — and suppressing either would drop a file the diff really changed.
    let members: Vec<String> = out.iter().filter(|id| id.starts_with("sym:")).cloned().collect();
    out.retain(|id| !id.starts_with("sym:")
        || !members.iter().any(|m| m.len() > id.len() && m.starts_with(id.as_str()) && m[id.len()..].starts_with('.')));
    (out.into_iter().collect(), code_outside)
}

pub struct Report { pub touched: Vec<String>, pub depth: usize, pub affected: Vec<Dependent>, pub files: BTreeSet<String>, pub risk: &'static str }

/// Code outside every symbol — an import line, a top-level statement — changes the whole file,
/// so every symbol declared in it is walked; a symbol hunk walks that symbol alone. A comment or
/// a blank line outside every symbol changes no symbol, and walking the whole file for it read a
/// one-line comment as the file's entire blast radius.
fn roots(graph: &Graph, touched: &[String], code_outside: &BTreeSet<&str>) -> BTreeSet<String> {
    let mut out = BTreeSet::new();
    for id in touched {
        match id.strip_prefix("file:") {
            Some(rel) if code_outside.contains(rel) => out.extend(graph.nodes.values().filter(|n| n.kind == NodeKind::Symbol && n.file == rel).map(|n| n.id.clone())),
            Some(_) => {}
            None => { out.insert(id.clone()); }
        }
    }
    out
}

pub fn report(graph: &Graph, hunks: &[Hunk], depth: usize) -> Report {
    let (touched, code_outside) = touch(graph, hunks);
    let touched_set: BTreeSet<&str> = touched.iter().map(String::as_str).collect();
    let roots = roots(graph, &touched, &code_outside);
    let root_files: BTreeSet<String> = roots.iter().filter_map(|r| graph.nodes.get(r).map(|n| n.file.clone())).collect();
    let mut affected: BTreeMap<String, Dependent> = BTreeMap::new();
    let mut files: BTreeSet<String> = BTreeSet::new();
    let index = impact::Index::new(graph, true);
    for id in &roots {
        let imp = index.upstream(id, depth);
        files.extend(imp.importers.iter().cloned());
        // A dependent that is itself being changed is not affected, it is the change — a file
        // whose top-level code both changed and calls a changed symbol included.
        for d in imp.layers.into_iter().flatten().filter(|d| !roots.contains(&d.id) && !touched_set.contains(d.id.as_str())) {
            // One row per dependent, at the shallowest depth any touched symbol reaches it, and
            // by a call rather than an argument edge at that depth.
            match affected.get_mut(&d.id) {
                Some(a) if impact::beats(&d, a) => *a = d,
                Some(_) => {}
                None => { affected.insert(d.id.clone(), d); }
            }
        }
    }
    let mut affected: Vec<Dependent> = affected.into_values().collect();
    affected.sort_by(|a, b| (a.depth, &a.id).cmp(&(b.depth, &b.id)));
    files.extend(affected.iter().map(|d| file_of(graph, &d.id)));
    files.retain(|f| !root_files.contains(f));
    let direct = affected.iter().filter(|d| d.depth == 1).count();
    let risk = impact::risk(direct, affected.len(), files.len());
    Report { touched, depth, affected, files, risk }
}

fn file_of(graph: &Graph, id: &str) -> String {
    match graph.nodes.get(id) { Some(n) => n.file.clone(), None => id.trim_start_matches("file:").to_string() }
}

fn plural(n: usize, one: &str) -> String { format!("{n} {one}{}", if n == 1 { "" } else { "s" }) }

fn span_of(graph: &Graph, id: &str) -> String {
    match graph.nodes.get(id) {
        Some(n) if n.end > n.line => format!("{}:{}-{}", n.file, n.line, n.end),
        Some(n) => format!("{}:{}", n.file, n.line),
        None if id.starts_with("file:") => "not indexed".into(),
        None => "?".into(),
    }
}

pub fn render(graph: &Graph, r: &Report) -> String {
    if r.touched.is_empty() { return "changed: 0 symbols\n".into() }
    let symbols = r.touched.iter().filter(|id| !id.starts_with("file:")).count();
    let changed_files: BTreeSet<String> = r.touched.iter().map(|id| file_of(graph, id)).collect();
    let mut out = format!("changed: {} in {}\n", plural(symbols, "symbol"), plural(changed_files.len(), "file"));
    for id in &r.touched { out.push_str(&format!("  {id}  {}\n", span_of(graph, id))); }
    out.push_str(&format!("affected (depth {}): {} in {}\n", r.depth, plural(r.affected.len(), "symbol"), plural(r.files.len(), "file")));
    for d in &r.affected {
        let at = graph.nodes.get(&d.id).map(|n| format!("{}:{}", n.file, n.line)).unwrap_or_else(|| d.id.trim_start_matches("file:").to_string());
        out.push_str(&format!("  d={}  {}  {at}  ← {}\n", d.depth, d.id, d.via));
    }
    let direct = r.affected.iter().filter(|d| d.depth == 1).count();
    out.push_str(&format!("risk: {} — {direct} direct, {} total, {}\n", r.risk, r.affected.len(), plural(r.files.len(), "file")));
    out
}

pub fn render_json(graph: &Graph, r: &Report) -> String {
    let touched: Vec<serde_json::Value> = r.touched.iter()
        .map(|id| serde_json::json!({ "id": id, "at": span_of(graph, id), "indexed": graph.nodes.contains_key(id) })).collect();
    let affected: Vec<serde_json::Value> = r.affected.iter().map(|d| serde_json::json!({
        "id": d.id, "at": graph.nodes.get(&d.id).map(|n| format!("{}:{}", n.file, n.line)), "depth": d.depth, "kind": format!("{:?}", d.kind), "passes": d.passed, "via": d.via,
    })).collect();
    serde_json::json!({ "touched": touched, "affected": affected, "files": r.files, "risk": r.risk }).to_string() + "\n"
}

pub(crate) fn git(repo: &Path, args: &[&str]) -> anyhow::Result<String> {
    let out = crate::git_command().arg("-C").arg(repo)
        // `core.quotepath` is on by default, and it C-escapes every path with a byte outside
        // ASCII and puts the opening quote *before* the `b/` — a `+++` header `parse` reads no
        // name from, which drops the file's hunks in silence, and an `ls-files` name that matches
        // no node. Off, git writes UTF-8 and leaves every ASCII path byte-identical. It governs
        // those bytes only: a name holding a newline, a quote or a backslash is quoted either way.
        .args(["-c", "core.quotepath=false"]).args(args).output()?;
    anyhow::ensure!(out.status.success(), "git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// Hunks of the working tree against `base` — staged and unstaged alike, plus every untracked
/// file as one hunk over its whole length, so a new file's symbols count as changed too.
pub fn hunks_from_git(repo: &Path, base: &str) -> anyhow::Result<Vec<Hunk>> {
    // Before `--`, git reads a leading dash as an option: `--output=<file>` would write anywhere.
    anyhow::ensure!(!base.starts_with('-'), "changes: base `{base}` is not a revision");
    // The store is written by every refresh, so in a repository that does not ignore it, it would
    // be reported as changed by the command that just wrote it. Both names: a store built before
    // the rename stays under the old one until a write moves it.
    const NEW: &str = ":(exclude).zerograph";
    const OLD: &str = ":(exclude).repograph";
    let mut hunks = parse(&git(repo, &["diff", "-U0", "--no-color", "--no-ext-diff", base, "--", ".", NEW, OLD])?);
    // NUL rather than lines: turning the quoting off reaches the bytes above ASCII and no
    // further, so a name holding a newline still arrived quoted and matched nothing. Separated
    // this way it arrives as itself, and the line it holds cannot be read as a second file.
    for f in git(repo, &["ls-files", "-z", "--others", "--exclude-standard", "--", ".", NEW, OLD])?.split('\0').filter(|f| !f.is_empty()) {
        hunks.push(Hunk::code(f, 1, u32::MAX));
    }
    Ok(hunks)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{EdgeKind, Extraction};

    fn touched(graph: &Graph, hunks: &[Hunk]) -> Vec<String> {
        touch(graph, hunks).0
    }

    const DIFF: &str = "diff --git a/s.ts b/s.ts\n--- a/s.ts\n+++ b/s.ts\n@@ -6,2 +6,3 @@ export class S {\n+  // more\n@@ -20 +21,0 @@\n-old\ndiff --git a/new.ts b/new.ts\nnew file mode 100644\n--- /dev/null\n+++ b/new.ts\n@@ -0,0 +1,2 @@\n+a\n+b\ndiff --git a/gone.ts b/gone.ts\n--- a/gone.ts\n+++ /dev/null\n@@ -1,3 +0,0 @@\n-x\n";

    #[test]
    fn a_base_that_reads_as_an_option_is_refused_before_git_sees_it() {
        let d = tempfile::tempdir().unwrap();
        let err = hunks_from_git(d.path(), "--output=/tmp/x").unwrap_err().to_string();
        assert!(err.contains("is not a revision"), "{err}");
    }

    #[test]
    fn parse_takes_new_side_ranges_and_records_a_pure_deletion_as_one_line() {
        assert_eq!(parse(DIFF), vec![
            Hunk { comment_only: true, ..Hunk::code("s.ts", 6, 8) },
            Hunk::code("s.ts", 21, 21),
            Hunk::code("new.ts", 1, 2),
        ]);
    }

    fn graph() -> Graph {
        let mut g = Graph::default();
        let mut e = Extraction::default();
        e.node(NodeKind::File, "file:s.ts", "s.ts", "", "s.ts", 1);
        e.node_span(NodeKind::Symbol, "sym:s.ts::S", "S", "", "s.ts", (3, 12));
        e.node_span(NodeKind::Symbol, "sym:s.ts::S.create", "S.create", "", "s.ts", (5, 8));
        e.node_span(NodeKind::Symbol, "sym:s.ts::S.list", "S.list", "", "s.ts", (9, 11));
        e.node_span(NodeKind::Symbol, "sym:s.ts::helper", "helper", "", "s.ts", (14, 14));
        e.edge("file:s.ts", "sym:s.ts::S", EdgeKind::Declares, "export", "s.ts");
        e.edge("sym:s.ts::S", "sym:s.ts::S.create", EdgeKind::Declares, "", "s.ts");
        e.edge("sym:s.ts::S", "sym:s.ts::S.list", EdgeKind::Declares, "", "s.ts");
        e.node(NodeKind::Symbol, "sym:c.ts::C.create", "C.create", "", "c.ts", 9);
        e.edge("sym:c.ts::C.create", "sym:s.ts::S.create", EdgeKind::Calls, "", "c.ts");
        g.apply(e);
        g
    }

    #[test]
    fn a_hunk_inside_a_member_names_the_member_not_the_class() {
        assert_eq!(touched(&graph(), &[Hunk::code("s.ts", 6, 7)]), vec!["sym:s.ts::S.create"]);
    }

    #[test]
    fn a_hunk_spanning_two_members_names_both() {
        assert_eq!(touched(&graph(), &[Hunk::code("s.ts", 8, 9)]), vec!["sym:s.ts::S.create", "sym:s.ts::S.list"]);
    }

    #[test]
    fn a_hunk_in_the_class_but_outside_every_member_names_the_class() {
        assert_eq!(touched(&graph(), &[Hunk::code("s.ts", 4, 4)]), vec!["sym:s.ts::S"]);
    }

    #[test]
    fn a_hunk_outside_every_symbol_falls_to_the_file() {
        assert_eq!(touched(&graph(), &[Hunk::code("s.ts", 1, 1)]), vec!["file:s.ts"]);
    }

    #[test]
    fn a_hunk_is_comment_only_by_the_comment_syntax_of_its_file() {
        let one = |file: &str, lines: &str| {
            let diff = format!("--- a/{file}\n+++ b/{file}\n@@ -1 +1,2 @@\n{lines}");
            parse(&diff)[0].comment_only
        };
        assert!(one("a.ts", "+// note\n+\n-/* was */\n"));
        assert!(one("a.py", "+# note\n"));
        assert!(one("a.sql", "+-- note\n"));
        assert!(one("a.mts", "+// note\n"));
        // `#count` is a private field in TypeScript, and `--` opens no comment in Python.
        assert!(!one("a.ts", "+  #count = 0;\n"));
        assert!(!one("a.py", "+-- x\n"));
        assert!(!one("a.ts", "+// note\n+import { x } from './x';\n"));
        // What might be code is code: a wrapped multiplication, code after a block comment.
        assert!(!one("a.ts", "+  * rate;\n"));
        assert!(!one("a.ts", "+/* c */ import x from 'x';\n"));
        assert!(one("a.ts", "+/* c */\n+*/\n"));
        // A comment that steers a tool is not trivia.
        assert!(!one("a.ts", "+// @ts-nocheck\n"));
        assert!(!one("a.ts", "+// eslint-disable-next-line no-console\n"));
        assert!(!one("a.py", "+import os  # noqa\n"));
        assert!(!one("a.py", "+# type: ignore\n"));
        assert!(!one("a.sh", "+#!/bin/bash\n"));
        assert!(!one("a.ts", "+/// <reference types=\"node\" />\n"));
        assert!(!one("a.ts", "+/* v8 ignore next */\n"));
        assert!(!one("a.py", "+# pragma: no cover\n"));
        assert!(!one("a.sql", "+-- +goose Down\n"));
        assert!(!one("a.sql", "+/*!40101 SET NAMES utf8 */;\n"));
        assert!(one("a.rs", "+/// Rate in percent.\n"));
        // A file whose comment syntax is not known is walked as it always was.
        assert!(!one("Makefile", "+# note\n"));
    }

    #[test]
    fn an_unreadable_hunk_header_does_not_lend_its_body_to_the_hunk_before() {
        let hunks = parse("--- a/a.ts\n+++ b/a.ts\n@@ -1 +1 @@\n+// note\n@@ junk @@\n+call();\n");
        assert_eq!(hunks.iter().map(|h| h.comment_only).collect::<Vec<_>>(), vec![true]);
    }

    #[test]
    fn a_hunk_with_no_body_line_is_not_comment_only() {
        let hunks = parse("--- a/a.ts\n+++ b/a.ts\n@@ -1 +1 @@\n\\ No newline at end of file\n@@ -5 +5 @@\n+// note\n");
        assert_eq!(hunks.iter().map(|h| h.comment_only).collect::<Vec<_>>(), vec![false, true]);
    }

    #[test]
    fn a_comment_outside_every_symbol_names_its_file_and_walks_nothing() {
        let comment = report(&graph(), &[Hunk { comment_only: true, ..Hunk::code("s.ts", 1, 1) }], 3);
        assert_eq!(comment.touched, vec!["file:s.ts"]);
        assert!(comment.affected.is_empty(), "{:?}", comment.affected);
        let import = report(&graph(), &[Hunk::code("s.ts", 1, 1)], 3);
        assert!(import.affected.iter().any(|d| d.id == "sym:c.ts::C.create"), "{:?}", import.affected);
    }

    #[test]
    fn a_comment_inside_a_symbol_still_names_that_symbol() {
        let r = report(&graph(), &[Hunk { comment_only: true, ..Hunk::code("s.ts", 6, 6) }], 3);
        assert_eq!(r.touched, vec!["sym:s.ts::S.create"]);
        assert!(r.affected.iter().any(|d| d.id == "sym:c.ts::C.create"));
    }

    #[test]
    fn a_whole_file_hunk_names_every_symbol_of_the_file_but_no_containing_class() {
        assert_eq!(touched(&graph(), &[Hunk::code("s.ts", 1, u32::MAX)]), vec!["sym:s.ts::S.create", "sym:s.ts::S.list", "sym:s.ts::helper"]);
    }

    #[test]
    fn a_hunk_in_a_file_the_graph_never_indexed_is_reported_as_that_file() {
        assert_eq!(touched(&graph(), &[Hunk::code("Foo.kt", 1, 9)]), vec!["file:Foo.kt"]);
    }

    #[test]
    fn a_file_id_is_not_suppressed_by_a_longer_file_id_that_extends_it_with_a_dot() {
        // `Dockerfile` / `Dockerfile.dev`, `index.d.ts` / `index.d.ts.map`, `LICENSE` / `LICENSE.md`:
        // the member suppression reads the longer name as a member of the shorter one and the diff
        // loses the shorter file outright.
        let hunks = [
            Hunk::code("Dockerfile", 1, 1),
            Hunk::code("Dockerfile.dev", 1, 1),
        ];
        assert_eq!(touched(&graph(), &hunks), vec!["file:Dockerfile", "file:Dockerfile.dev"]);
    }

    #[test]
    fn an_unindexed_file_renders_as_changed_with_no_span_and_reaches_nothing() {
        let g = graph();
        let r = report(&g, &[Hunk::code("Foo.kt", 1, 9)], 2);
        assert_eq!(r.touched, vec!["file:Foo.kt"]);
        assert!(r.affected.is_empty());
        assert_eq!(
            render(&g, &r),
            "changed: 0 symbols in 1 file\n  file:Foo.kt  not indexed\naffected (depth 2): 0 symbols in 0 files\nrisk: LOW — 0 direct, 0 total, 0 files\n"
        );
        let v: serde_json::Value = serde_json::from_str(&render_json(&g, &r)).unwrap();
        assert_eq!(v["touched"][0]["indexed"], false);
    }

    #[test]
    fn report_unions_the_callers_of_every_touched_symbol() {
        let g = graph();
        let r = report(&g, &[Hunk::code("s.ts", 6, 7)], 2);
        assert_eq!(r.touched, vec!["sym:s.ts::S.create"]);
        assert_eq!(r.affected.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), vec!["sym:c.ts::C.create"]);
        assert_eq!(r.files, BTreeSet::from(["c.ts".to_string()]));
        assert_eq!(r.risk, "LOW");
    }

    #[test]
    fn a_dependent_that_calls_one_changed_symbol_and_passes_another_is_listed_by_the_call() {
        let mut g = graph();
        let mut e = Extraction::default();
        e.node(NodeKind::Symbol, "sym:d.ts::D", "D", "", "d.ts", 1);
        e.edge("sym:d.ts::D", "sym:s.ts::S.create", EdgeKind::Calls, "arg", "d.ts");
        e.edge("sym:d.ts::D", "sym:s.ts::S.list", EdgeKind::Calls, "", "d.ts");
        g.apply(e);
        let r = report(&g, &[Hunk::code("s.ts", 8, 9)], 1);
        let d = r.affected.iter().find(|d| d.id == "sym:d.ts::D").unwrap();
        assert_eq!((d.via.as_str(), d.passed), ("sym:s.ts::S.list", false));
    }

    #[test]
    fn json_marks_a_dependent_that_only_passes_the_changed_symbol() {
        let mut g = graph();
        let mut e = Extraction::default();
        e.node(NodeKind::Symbol, "sym:d.ts::D", "D", "", "d.ts", 1);
        e.edge("sym:d.ts::D", "sym:s.ts::S.create", EdgeKind::Calls, "arg", "d.ts");
        g.apply(e);
        let v: serde_json::Value = serde_json::from_str(&render_json(&g, &report(&g, &[Hunk::code("s.ts", 6, 7)], 1))).unwrap();
        let rows: Vec<(&str, &serde_json::Value)> = v["affected"].as_array().unwrap().iter().map(|d| (d["id"].as_str().unwrap(), &d["passes"])).collect();
        assert_eq!(rows, vec![("sym:c.ts::C.create", &serde_json::json!(false)), ("sym:d.ts::D", &serde_json::json!(true))]);
    }

    #[test]
    fn a_diff_of_thousands_of_symbols_is_walked_in_seconds_not_minutes() {
        // Every root once regrouped and rescanned the whole edge set; 3,000 roots over 30,000
        // edges took tens of seconds that way, and take milliseconds over one index.
        let mut g = Graph::default();
        let mut e = Extraction::default();
        e.node(NodeKind::File, "file:big.ts", "big.ts", "", "big.ts", 1);
        for i in 0..3000u32 {
            let id = format!("sym:big.ts::f{i}");
            e.node_span(NodeKind::Symbol, &id, "f", "", "big.ts", (i * 10 + 2, i * 10 + 9));
            e.edge("file:big.ts", &id, EdgeKind::Declares, "export", "big.ts");
            e.edge(&format!("sym:c{i}.ts::use"), &id, EdgeKind::Calls, "", &format!("c{i}.ts"));
            for j in 0..8 { e.edge(&format!("FR-X-{i}"), &format!("FR-Y-{j}"), EdgeKind::References, "body", "d.md"); }
        }
        g.apply(e);
        let started = std::time::Instant::now();
        let r = report(&g, &[Hunk::code("big.ts", 1, u32::MAX)], 2);
        assert_eq!(r.affected.len(), 3000);
        assert!(started.elapsed() < std::time::Duration::from_secs(3), "{:?}", started.elapsed());
    }

    #[test]
    fn a_file_level_change_walks_every_symbol_of_the_file_and_lists_none_of_them_as_affected() {
        let g = graph();
        let r = report(&g, &[Hunk::code("s.ts", 1, 1)], 2);
        assert_eq!(r.touched, vec!["file:s.ts"]);
        assert_eq!(r.affected.iter().map(|d| d.id.as_str()).collect::<Vec<_>>(), vec!["sym:c.ts::C.create"]);
        assert_eq!(r.files, BTreeSet::from(["c.ts".to_string()]));
    }

    #[test]
    fn a_changed_file_whose_top_level_calls_a_changed_symbol_is_not_also_affected() {
        let mut g = graph();
        let mut e = Extraction::default();
        e.edge("file:s.ts", "sym:s.ts::helper", EdgeKind::Calls, "", "s.ts");
        g.apply(e);
        let r = report(&g, &[Hunk::code("s.ts", 1, 1)], 2);
        assert_eq!(r.touched, vec!["file:s.ts"]);
        assert!(r.affected.iter().all(|d| !r.touched.contains(&d.id)), "{:?}", r.affected);
    }

    #[test]
    fn a_file_level_change_in_an_indexed_file_renders_zero_symbols_but_names_the_file() {
        let g = graph();
        let r = report(&g, &[Hunk::code("s.ts", 1, 1)], 2);
        assert_eq!(
            render(&g, &r),
            "changed: 0 symbols in 1 file\n  file:s.ts  s.ts:1\naffected (depth 2): 1 symbol in 1 file\n  d=1  sym:c.ts::C.create  c.ts:9  ← sym:s.ts::S.create\nrisk: LOW — 1 direct, 1 total, 1 file\n"
        );
    }

    #[test]
    fn render_says_what_changed_and_what_it_reaches() {
        let g = graph();
        let out = render(&g, &report(&g, &[Hunk::code("s.ts", 6, 7)], 2));
        assert_eq!(out, "changed: 1 symbol in 1 file\n  sym:s.ts::S.create  s.ts:5-8\naffected (depth 2): 1 symbol in 1 file\n  d=1  sym:c.ts::C.create  c.ts:9  ← sym:s.ts::S.create\nrisk: LOW — 1 direct, 1 total, 1 file\n");
    }

    #[test]
    fn an_empty_diff_renders_a_clean_report() {
        let g = graph();
        assert_eq!(render(&g, &report(&g, &[], 2)), "changed: 0 symbols\n");
    }

    fn git_in(repo: &Path, args: &[&str]) {
        let out = crate::git_command().arg("-C").arg(repo)
            .args(args).output().unwrap_or_else(|e| panic!("git {args:?}: {e}"));
        assert!(out.status.success(), "git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr));
    }

    /// A repository that quotes. `core.quotepath` is git's default, but a machine that turns it
    /// off globally would make the two tests below pass without the fix. Signing, hooks and the
    /// global exclude file are pinned for the mirror-image reason: no personal or CI git
    /// configuration should be able to fail these tests for something that is not quoting. The
    /// ambient `GIT_DIR` is dropped for a third reason: a run started from a hook would otherwise
    /// initialise and commit against the repository the hook belongs to.
    fn quoting_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().unwrap();
        git_in(dir.path(), &["init", "-q"]);
        let absent = dir.path().join("absent");
        let absent = absent.to_str().unwrap();
        for (key, value) in [
            ("core.quotepath", "true"),
            ("commit.gpgsign", "false"),
            ("core.hooksPath", absent),
            ("core.excludesFile", absent),
            ("core.attributesFile", absent),
            ("user.email", "t@example.invalid"),
            ("user.name", "t"),
        ] {
            git_in(dir.path(), &["config", key, value]);
        }
        std::fs::create_dir(dir.path().join("docs")).unwrap();
        std::fs::write(dir.path().join("docs/a.ts"), "one\n").unwrap();
        git_in(dir.path(), &["add", "-A"]);
        git_in(dir.path(), &["commit", "-qm", "base"]);
        dir
    }

    #[test]
    fn a_tracked_non_ascii_path_keeps_its_hunks_under_the_unescaped_name() {
        let dir = quoting_repo();
        std::fs::write(dir.path().join("docs/Штраф.ts"), "one\ntwo\n").unwrap();
        git_in(dir.path(), &["add", "-A"]);
        assert_eq!(
            hunks_from_git(dir.path(), "HEAD").unwrap(),
            vec![Hunk::code("docs/Штраф.ts", 1, 2)]
        );
    }

    /// The shape the gap was raised about: not a file being added under a quoted name, but one
    /// already in the graph whose single line moves. Its header quotes the same way and its hunk
    /// is the one a blast radius loses.
    #[test]
    fn a_committed_non_ascii_path_keeps_the_hunk_of_the_line_that_changed() {
        let dir = quoting_repo();
        std::fs::write(dir.path().join("docs/Штраф.ts"), "one\ntwo\n").unwrap();
        git_in(dir.path(), &["add", "-A"]);
        git_in(dir.path(), &["commit", "-qm", "the file as it stands"]);
        std::fs::write(dir.path().join("docs/Штраф.ts"), "one\nthree\n").unwrap();
        assert_eq!(
            hunks_from_git(dir.path(), "HEAD").unwrap(),
            vec![Hunk::code("docs/Штраф.ts", 2, 2)]
        );
    }

    // A name with no decomposable letter, so a filesystem that hands back NFD cannot fail this
    // test for a normalization difference that has nothing to do with quoting.
    #[test]
    fn an_untracked_non_ascii_path_is_reported_as_itself() {
        let dir = quoting_repo();
        std::fs::write(dir.path().join("docs/Новое.ts"), "one\n").unwrap();
        assert_eq!(
            hunks_from_git(dir.path(), "HEAD").unwrap(),
            vec![Hunk::code("docs/Новое.ts", 1, u32::MAX)]
        );
    }

    #[test]
    fn the_store_is_not_a_change_even_when_nothing_ignores_it() {
        let dir = quoting_repo();
        for store in [".zerograph", ".repograph"] {
            std::fs::create_dir_all(dir.path().join(store)).unwrap();
            std::fs::write(dir.path().join(store).join("graph.bin"), "x").unwrap();
        }
        std::fs::write(dir.path().join("docs/new.ts"), "one\n").unwrap();
        assert_eq!(
            hunks_from_git(dir.path(), "HEAD").unwrap(),
            vec![Hunk::code("docs/new.ts", 1, u32::MAX)]
        );
    }
}
