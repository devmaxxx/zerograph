//! Which id families a repository has. A family is the prefix of any id the corpus *defines* —
//! a requirement line, its heading form, a milestone document, a registry row — so the documents
//! that already say it are the only place it is written down. Nothing here reads them a second
//! time to find out which they are: a definition is already a node, so the set is a view over
//! the graph (`of_graph`), and the only pass over the tree left is the mention tally that
//! `zerograph families` prints beside it.
//!
//! Nothing configures this and nothing stores it. A default is the answer for a repository about
//! which nothing is known, and one corpus's list of families was never that answer for anybody
//! else's; a pin beside the graph would have been a second thing to keep in step with the
//! documents. The two losses are worth naming: a prefix a document only ever mentions —
//! `ISO-8601`, a ticket number, a year — is cited and then held aside rather than linked, and
//! there is no way to take a family away by hand. `zerograph families` prints both halves so a
//! prefix on the wrong side of that line is something a reader can see.

use crate::model::Graph;
use crate::store::Store;
use crate::walk::{Entry, FileKind};
use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::OnceLock;

/// Which of the two kinds an id is written in, and the family it belongs to. They are counted
/// apart because the extractor matches them apart: `X-01` is an id, `X-M01` a milestone.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family<'a> {
    Id(&'a str),
    Milestone(&'a str),
}

/// The id slot of the definition grammar, with any family in place of a known one: `FR-PAY-22`
/// is `FR-PAY`, `N-151` is `N`, `SECURITY-12` is `SECURITY`, `FR-PAY-EU-1` is `FR-PAY-EU`. The
/// slot is wide because a corpus's prefixes are its own and nothing else writes them down; the
/// hyphen before the digits is what keeps `B1` and `C11` out. What holds it together is the fixed
/// point: a family this reads out of a definition head goes into the matcher the extractor writes
/// nodes with, and `classify` must read the same family back off those nodes or an update would
/// find one set in the documents and another in the graph on every run, for ever.
pub(crate) const FAMILY: &str = r"[A-Z][A-Z0-9]{0,11}(?:-[A-Z][A-Z0-9]{0,11}){0,3}";
/// The milestone slot, shared with the extractor's own `milestone_file` and written as `kind_for`
/// writes it: what those accept is what becomes a milestone node, and a scan that admitted one
/// letter more would derive a family no node is ever written in.
pub(crate) const MILESTONE: &str = r"[A-Z]+";

fn shapes() -> &'static (Regex, Regex) {
    static RE: OnceLock<(Regex, Regex)> = OnceLock::new();
    RE.get_or_init(|| {
        (
            Regex::new(&format!(r"^({FAMILY})-\d{{1,4}}$")).unwrap(),
            // A task is written in its milestone's family and hangs off it: `BE-M10/T05`.
            Regex::new(&format!(r"^({MILESTONE})-M\d{{2}}(?:/T\d{{1,4}})?$")).unwrap(),
        )
    })
}

/// The family an id is written in, or `None` for anything that is not an id at all — a file node,
/// an entity, a legacy concept, a symbol.
pub fn classify(id: &str) -> Option<Family<'_>> {
    let (ids, milestones) = shapes();
    if let Some(c) = milestones.captures(id) {
        return Some(Family::Milestone(c.get(1).unwrap().as_str()));
    }
    ids.captures(id).map(|c| Family::Id(c.get(1).unwrap().as_str()))
}

/// Where a family was first defined, or where a prefix nobody defines was first written.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Site {
    pub file: String,
    pub line: u32,
    pub text: String,
}

/// A prefix the documents write and no line defines: a standard, a ticket number, a year.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Mention {
    pub prefix: String,
    pub mentions: usize,
    /// Distinct ids written under the prefix. A prefix cited a thousand times as one id and one
    /// cited a thousand times as a hundred ids are different things to look at.
    pub ids: usize,
    pub files: usize,
    pub example: Option<Site>,
}

/// How much of a family the graph holds. `definitions` is declaring-file × id pairs — a node two
/// documents declare counts twice — read off each node's own declaring-file set, because that set
/// is what a definition writes and an edge scan is not: a milestone's tasks are declared by the
/// milestone node, not by the file, so nothing under a milestone family is the target of a
/// file-sourced `Declares` edge at all.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Count {
    pub nodes: usize,
    pub definitions: usize,
}

/// Every family the graph's own nodes are written in, ids and milestones apart, with how much of
/// each it holds. What `of_graph` returns, and the whole of what a writer compares before and
/// after an apply to say which families moved.
pub type Families = (BTreeMap<String, Count>, BTreeMap<String, Count>);

/// What a build says it found. The whole list where it is short and its head with a count where
/// it is not: a build's stderr is read at a glance, and a large corpus names dozens.
pub fn line(f: &Families) -> String {
    const SHOWN: usize = 20;
    let listed = |m: &BTreeMap<String, Count>| {
        let v: Vec<&str> = m.keys().map(String::as_str).collect();
        match (v.len(), v.len() > SHOWN) {
            (0, _) => "(none)".to_string(),
            (n, true) => format!("{} … +{}", v[..SHOWN].join(", "), n - SHOWN),
            _ => v.join(", "),
        }
    };
    format!("families: {} · milestones: {}", listed(&f.0), listed(&f.1))
}

/// Families the graph gained or lost between two readings of it, each as the word an update
/// prints. Empty is the ordinary case.
pub fn moved(before: &Families, after: &Families) -> Vec<String> {
    let mut out = Vec::new();
    for (was, is, mark) in [(&before.0, &after.0, ""), (&before.1, &after.1, " milestone")] {
        for f in is.keys().filter(|f| !was.contains_key(*f)) {
            out.push(format!("+{f}{mark}"));
        }
        for f in was.keys().filter(|f| !is.contains_key(*f)) {
            out.push(format!("-{f}{mark}"));
        }
    }
    out
}

/// Every family the graph's own nodes are written in, with how much of each it holds. Code nodes
/// are excluded: a symbol named after an id is not one, and a file is not an id at all. One pass:
/// the report's `definitions` column is this walk's other half, so no row goes back over the
/// nodes or over the edges to find it.
pub fn of_graph(graph: &Graph) -> Families {
    let (mut ids, mut milestones) = (BTreeMap::new(), BTreeMap::new());
    for n in graph.nodes.values().filter(|n| !n.is_code()) {
        let entry = match classify(&n.id) {
            Some(Family::Id(f)) => ids.entry(f.to_string()),
            Some(Family::Milestone(f)) => milestones.entry(f.to_string()),
            None => continue,
        };
        let c: &mut Count = entry.or_default();
        c.nodes += 1;
        c.definitions += n.files.len();
    }
    (ids, milestones)
}

fn site(rel: &str, line_no: u32, line: &str) -> Site {
    Site { file: rel.to_string(), line: line_no, text: crate::query::headline(line.trim()) }
}

#[derive(Default)]
struct Tally {
    mentions: usize,
    ids: BTreeSet<String>,
    files: BTreeSet<String>,
    first: Option<Site>,
}

/// Every id-like token the tree writes, counted per prefix. What is left once the graph's own
/// families are taken out is what the report calls text.
#[derive(Default)]
struct Scan {
    seen: BTreeMap<String, Tally>,
}

impl Scan {
    fn doc(&mut self, rel: &str, text: &str) {
        // An editor's byte-order mark rides on the first line: left there it hides an opening
        // fence from the check below, and rides into the example line the report prints.
        let mut fenced = false;
        for (i, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
            if line.trim_start().starts_with("```") {
                fenced = !fenced;
                continue;
            }
            // A head quoted inside a code fence is an example of the dialect and yields no node
            // and no citation, so it may not be tallied either: this repository's own README
            // quotes the dialect in a fence, and listing those prefixes as mention-only points a
            // reader at an example line under a footer telling them to write exactly that line to
            // make it a family.
            if !fenced {
                self.mentions(rel, i as u32 + 1, line);
            }
        }
    }

    /// Every line, fences and all: a source file's ids are citations from comments, and a
    /// registry's are its own rows and the `basis:` prose that cites others. A prefix written
    /// only in one of those and defined nowhere is exactly the case the mention half of the
    /// report exists to surface.
    fn tally_lines(&mut self, rel: &str, text: &str) {
        for (i, line) in text.trim_start_matches('\u{feff}').lines().enumerate() {
            self.mentions(rel, i as u32 + 1, line);
        }
    }

    fn mentions(&mut self, rel: &str, line_no: u32, line: &str) {
        // The extractor's own reading of the line — ranges and slash lists expanded, boundaries
        // applied — so a mention is counted the way the graph would have cited it.
        for hit in crate::ids::generic().find_all(line) {
            if let Some(Family::Id(f) | Family::Milestone(f)) = classify(&hit.id) {
                // Bound first: `f` borrows `hit.id`, and the id can only move once that ends.
                let prefix = f.to_string();
                self.record(prefix, hit.id, rel, line_no, line);
            }
        }
    }

    fn record(&mut self, prefix: String, id: String, rel: &str, line_no: u32, line: &str) {
        let t = self.seen.entry(prefix).or_default();
        t.mentions += 1;
        t.ids.insert(id);
        t.files.insert(rel.to_string());
        t.first.get_or_insert_with(|| site(rel, line_no, line));
    }

    /// Every prefix the tree writes, the widest first — which of them are families is asked of
    /// the graph in `report`, not here. Distinct ids order the list ahead of raw mentions: a
    /// prefix written under twenty ids is a vocabulary nothing defines, where one written twenty
    /// times under a single id is one citation repeated.
    fn finish(self) -> Vec<Mention> {
        let mut cited: Vec<Mention> = self.seen.into_iter()
            .map(|(prefix, t)| Mention { prefix, mentions: t.mentions, ids: t.ids.len(), files: t.files.len(), example: t.first })
            .collect();
        cited.sort_by(|a, b| b.ids.cmp(&a.ids).then(b.mentions.cmp(&a.mentions)).then(a.prefix.cmp(&b.prefix)));
        cited
    }
}

/// The id-like prefixes the walked tree writes, with where each was first written. The only pass
/// over the tree this module still makes, and `zerograph families` is its one caller.
pub fn survey(repo: &Path, entries: &[Entry], redact_secrets: bool) -> Result<Vec<Mention>> {
    let mut scan = Scan::default();
    // Each prefix prints the line it was first written on, so the text is redacted as the
    // extractor redacts it: a key on the line that cites an id would otherwise reach the terminal.
    let mut batch: Vec<(&Entry, String)> = Vec::new();
    let mut batch_bytes = 0;
    let tally = |scan: &mut Scan, batch: &mut Vec<(&Entry, String)>| {
        if batch.is_empty() { return; }
        let redactor = match redact_secrets {
            true => crate::secrets::Redactor::for_texts(batch.iter().map(|(_, t)| t.as_str())),
            false => crate::secrets::Redactor::off(),
        };
        for (e, text) in batch.drain(..) {
            let (text, _) = redactor.redact(&e.rel, &text);
            match e.kind {
                FileKind::Doc => scan.doc(&e.rel, &text),
                FileKind::Code | FileKind::Registry | FileKind::Text => scan.tally_lines(&e.rel, &text),
            }
        }
    };
    for e in entries {
        // A text node has no edge out of it, so what it says cites nothing a reader can follow,
        // and reading every one would make `families` pay for the whole tree.
        if e.kind == FileKind::Text { continue; }
        let bytes = match (std::fs::read(repo.join(&e.rel)), e.kind) {
            (Ok(b), _) => b,
            // The mention half of the report is the whole of what this command says about the
            // prefixes nothing defines, and a document skipped in silence is a prefix missing
            // from it — so a document or a registry that cannot be read fails the report by name
            // instead of quietly shortening it.
            (Err(err), FileKind::Doc | FileKind::Registry) =>
                return Err(err).with_context(|| format!("families: read {}", e.rel)),
            (Err(err), FileKind::Code | FileKind::Text) => { eprintln!("{}", crate::terminal_safe(&format!("families: skipping {:?}: {err}", e.rel))); continue; }
        };
        // A file the extractor refuses for the same reason cites nothing in the graph either, so
        // leaving it out of the tally cannot hide a prefix a reader could have followed.
        let Ok(text) = String::from_utf8(bytes) else {
            eprintln!("families: skipping {:?}: not UTF-8", e.rel);
            continue;
        };
        batch_bytes += text.len();
        batch.push((e, text));
        if batch_bytes >= crate::secrets::BATCH_BYTES {
            tally(&mut scan, &mut batch);
            batch_bytes = 0;
        }
    }
    tally(&mut scan, &mut batch);
    Ok(scan.finish())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Row {
    pub family: String,
    pub nodes: usize,
    /// Declaring-file × id pairs, summed over the family's nodes: a node defined in two files
    /// counts twice, a family with one is one line that may have been a mistake, and the report
    /// puts it first for that reason.
    pub definitions: usize,
    pub defined: Site,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Report {
    pub families: Vec<Row>,
    pub milestones: Vec<Row>,
    pub mention_only: Vec<Mention>,
}

/// Whether a node is written in this family, in the id form or the milestone form as asked.
fn in_family(id: &str, family: &str, milestone: bool) -> bool {
    match classify(id) {
        Some(Family::Id(f)) => !milestone && f == family,
        Some(Family::Milestone(f)) => milestone && f == family,
        None => false,
    }
}

/// Where a family is defined: its first node in path-then-line order, since the graph keeps a
/// node's primary declaring file and line.
fn site_of(graph: &Graph, family: &str, milestone: bool) -> Site {
    let n = graph.nodes.values()
        .filter(|n| !n.is_code() && in_family(&n.id, family, milestone))
        .min_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)))
        .expect("a counted family has a node");
    Site { file: n.file.clone(), line: n.line, text: crate::query::headline(&n.label) }
}

fn rows(graph: &Graph, counts: &BTreeMap<String, Count>, milestone: bool) -> Vec<Row> {
    let mut out: Vec<Row> = counts.iter().map(|(family, c)| {
        Row { family: family.clone(), nodes: c.nodes, definitions: c.definitions, defined: site_of(graph, family, milestone) }
    }).collect();
    out.sort_by(|a, b| a.definitions.cmp(&b.definitions).then(a.nodes.cmp(&b.nodes)).then(a.family.cmp(&b.family)));
    out
}

pub fn report(graph: &Graph, cited: Vec<Mention>) -> Report {
    let (ids, milestones) = of_graph(graph);
    let mention_only = cited.into_iter()
        .filter(|m| !ids.contains_key(&m.prefix) && !milestones.contains_key(&m.prefix))
        .collect();
    Report { families: rows(graph, &ids, false), milestones: rows(graph, &milestones, true), mention_only }
}

const NAME: usize = 36;

fn at(s: &Site) -> String {
    format!("{}:{}", s.file, s.line)
}

fn section(head: &str, rows: &[Row]) -> String {
    let mut out = format!("{:<NAME$}{:>7}{:>6}  {}\n", head, "nodes", "defs", "defined");
    for r in rows {
        let once = if r.definitions == 1 { "  defined once" } else { "" };
        out.push_str(&format!("{:<NAME$}{:>7}{:>6}  {}{once}\n", format!("  {}", r.family), r.nodes, r.definitions, at(&r.defined)));
    }
    if rows.is_empty() { out.push_str("  (none)\n"); }
    out
}

pub fn render(r: &Report) -> String {
    let mut out = section("families", &r.families);
    out.push_str(&section("milestones", &r.milestones));
    out.push_str(&format!("{:<NAME$}{:>7}{:>6}{:>7}  e.g.\n", "mention-only prefixes", "written", "ids", "files"));
    for m in &r.mention_only {
        let example = m.example.as_ref().map(|e| format!("  {}  {}", at(e), e.text)).unwrap_or_default();
        out.push_str(&format!("{:<NAME$}{:>7}{:>6}{:>7}{example}\n", format!("  {}", m.prefix), m.mentions, m.ids, m.files));
    }
    if r.mention_only.is_empty() { out.push_str("  (none)\n"); }
    out.push_str("\n# Not families: no line defines one of these. A prefix a document only writes —\n");
    out.push_str("# ISO-8601, RFC-7231, a ticket number, a year — is text, and stays text. To make one\n");
    out.push_str("# a family, write a line that defines it: `**PREFIX-1 · MUST · title**`.\n");
    out
}

pub fn run(repo: &Path, cfg: &crate::config::Config, json: bool) -> Result<()> {
    let (graph, manifest) = Store::new(repo).load()?;
    if graph.nodes.is_empty() { anyhow::bail!("graph is empty — run `zerograph build`"); }
    let entries = crate::walk::walk(repo, cfg, &manifest)?;
    // The graph is the report's source, so a definition edited since the last update is not in
    // it; the tree being ahead of the store is said once, in words, rather than left for a reader
    // to infer from a row that looks a day old.
    let diff = manifest.diff(&entries);
    let behind = diff.changed.len() + diff.removed.len();
    if behind > 0 {
        eprintln!("families: the store is {behind} file{} behind the tree — run `zerograph update`",
            if behind == 1 { "" } else { "s" });
    }
    let r = report(&graph, survey(repo, &entries, cfg.redact_secrets)?);
    match json {
        true => println!("{}", serde_json::to_string_pretty(&r)?),
        false => print!("{}", render(&r)),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Extraction, NodeKind};

    /// What the tree writes, prefix by prefix. The tally alone: which of these are families is a
    /// question for the graph, and `report` is where it is asked.
    fn scan(docs: &[(&str, &str)]) -> Vec<Mention> {
        let mut s = Scan::default();
        for (rel, text) in docs { s.doc(rel, text); }
        s.finish()
    }

    fn one(text: &str) -> Vec<Mention> {
        scan(&[("docs/a.md", text)])
    }

    fn cited(m: &[Mention]) -> Vec<&str> {
        let mut v: Vec<&str> = m.iter().map(|m| m.prefix.as_str()).collect();
        v.sort_unstable();
        v
    }

    fn written(m: &[Mention], prefix: &str) -> usize {
        m.iter().find(|m| m.prefix == prefix).unwrap_or_else(|| panic!("no {prefix} in {m:?}")).mentions
    }

    #[test]
    fn every_prefix_a_document_writes_is_counted_wherever_it_stands() {
        // A head, a heading, a list item, a citation in prose: the tally does not tell them
        // apart, because whether a line defined a node is something the graph already answers.
        let m = one("**REQ-7 · MUST · первое**\n## AC-3 · критерий\n- **OQ-1 · вопрос**\nсм. FR-PAY-22 в другом файле\n");
        assert_eq!(cited(&m), vec!["AC", "FR-PAY", "OQ", "REQ"]);
    }

    #[test]
    fn a_line_that_only_opens_with_an_id_is_counted_like_any_other() {
        let m = one("UTF-16 conversion is lossy\nsee ISO-8601 and RFC-7231\n**REQ-1 · MUST · x**\n");
        assert_eq!(cited(&m), vec!["ISO", "REQ", "RFC", "UTF"]);
    }

    #[test]
    fn a_date_is_neither_an_id_nor_a_mention() {
        assert!(one("released 2026-09-05, revised 2026-09-06\n").is_empty());
    }

    #[test]
    fn a_two_part_prefix_is_read_whole_not_from_its_last_segment() {
        assert_eq!(cited(&one("**FR-PAY-22 · MUST · окно отмены**\n")), vec!["FR-PAY"]);
    }

    #[test]
    fn a_head_inside_a_code_fence_is_not_counted() {
        // It yields no node and no citation, so listing its prefix as mention-only would point a
        // reader at an example line under a footer telling them to write exactly that line to
        // make it a family.
        let m = one("```\n**REQ-7 · MUST · пример диалекта**\n```\n**AC-1 · MUST · настоящее**\n");
        assert_eq!(cited(&m), vec!["AC"]);
    }

    #[test]
    fn a_prefix_written_only_in_code_or_only_in_a_registry_is_still_counted() {
        let mut s = Scan::default();
        s.tally_lines("src/pay.ts", "// implements NEW-1 and cites TCK-42\n");
        s.tally_lines("docs/constitution.yaml", "invariants:\n  - id: INV-01\n    basis: \"решение 1, `N-039`\"\n");
        assert_eq!(cited(&s.finish()), vec!["INV", "N", "NEW", "TCK"]);
    }

    #[test]
    fn a_milestone_only_mentioned_is_counted_under_its_own_prefix() {
        let m = one("см. QA-M01 и ещё раз QA-M01\n");
        assert_eq!((cited(&m), written(&m, "QA")), (vec!["QA"], 2));
    }

    #[test]
    fn a_range_and_a_slash_list_count_every_id_they_stand_for() {
        let m = one("см. INV-11…13 и OQ-1/2\n");
        assert_eq!((written(&m, "INV"), written(&m, "OQ")), (3, 2));
    }

    #[test]
    fn an_example_line_is_cut_on_a_character_boundary() {
        let m = one(&format!("**REQ-1 · {}**\n", "ф".repeat(200)));
        let text = &m[0].example.as_ref().unwrap().text;
        assert_eq!(text.chars().count(), crate::query::HEADLINE + 1);
        assert!(text.ends_with('…'));
    }

    fn declared(files: &[(&str, &[&str])]) -> Graph {
        let mut g = Graph::default();
        for (file, ids) in files {
            let mut e = Extraction::default();
            e.node(NodeKind::File, &format!("file:{file}"), file, "", file, 1);
            for id in *ids {
                e.node(NodeKind::Requirement, id, "label", "тело", file, 1);
                e.edge(&format!("file:{file}"), id, crate::model::EdgeKind::Declares, "", file);
            }
            g.apply(e);
        }
        g
    }

    #[test]
    fn a_family_defined_once_sorts_first_and_is_marked() {
        let g = declared(&[("docs/a.md", &["REQ-1", "REQ-2", "REQ-3"]), ("docs/b.md", &["OD-1"]), ("docs/c.md", &["REQ-1"])]);
        let r = report(&g, Vec::new());
        assert_eq!(r.families.iter().map(|f| (f.family.as_str(), f.nodes, f.definitions)).collect::<Vec<_>>(),
            vec![("OD", 1, 1), ("REQ", 3, 4)], "REQ-1 is declared by two files, so four definitions over three nodes");
        let text = render(&r);
        let od = text.lines().find(|l| l.trim_start().starts_with("OD")).unwrap();
        assert!(od.ends_with("defined once"), "{od}");
        assert!(!text.lines().find(|l| l.trim_start().starts_with("REQ")).unwrap().contains("defined once"));
    }

    /// A milestone document declares its milestone node, and the milestone node declares the
    /// tasks under it — so nothing under a milestone family is declared by a file. Counted off
    /// the `Declares` edges a file is the source of, such a family read one definition however
    /// many tasks it carried, and a one-document milestone printed `defined once` above its own
    /// tasks. Every node's declaring files are what stand behind it.
    #[test]
    fn a_milestone_that_declares_its_own_tasks_counts_every_one_of_them() {
        let mut g = Graph::default();
        let mut e = Extraction::default();
        e.node(NodeKind::File, "file:docs/m.md", "docs/m.md", "", "docs/m.md", 1);
        e.node(NodeKind::Milestone, "BE-M01", "foundation", "", "docs/m.md", 1);
        e.edge("file:docs/m.md", "BE-M01", crate::model::EdgeKind::Declares, "", "docs/m.md");
        for (tid, line) in [("BE-M01/T01", 5), ("BE-M01/T03", 6)] {
            e.node(NodeKind::Task, tid, "задача", "", "docs/m.md", line);
            e.edge("BE-M01", tid, crate::model::EdgeKind::Declares, "", "docs/m.md");
        }
        g.apply(e);
        let r = report(&g, Vec::new());
        assert_eq!(r.milestones.iter().map(|m| (m.family.as_str(), m.nodes, m.definitions)).collect::<Vec<_>>(),
            vec![("BE", 3, 3)], "three nodes, each written down once");
        let text = render(&r);
        assert!(!text.contains("defined once"), "a document that defines three things is not one line: {text}");
    }

    #[test]
    fn mention_only_prefixes_sort_by_distinct_ids_then_by_mentions() {
        let d = one("см. OQ-1, OQ-2, OQ-3 и OQ-1 ещё раз\nсм. ISO-8601, ISO-8601, ISO-8601, ISO-8601, ISO-8601\nRFC-7231\n");
        assert_eq!(d.iter().map(|m| (m.prefix.as_str(), m.ids, m.mentions)).collect::<Vec<_>>(),
            vec![("OQ", 3, 4), ("ISO", 1, 5), ("RFC", 1, 1)]);
    }

    /// `declared` with one document, plus the symbol named after an id that every row has to
    /// leave out.
    fn graph_with(ids: &[&str]) -> Graph {
        let mut g = declared(&[("docs/a.md", ids)]);
        let mut e = Extraction::default();
        e.node(NodeKind::Symbol, "src/a.ts#REQ-9", "REQ-9", "", "src/a.ts", 1);
        g.apply(e);
        g
    }

    #[test]
    fn the_graph_reports_the_same_families_its_documents_defined() {
        let g = graph_with(&["REQ-7", "AC-3", "BE-M01", "BE-M01/T05", "entity:Foo"]);
        let (ids, milestones) = of_graph(&g);
        let seen = |m: BTreeMap<String, Count>| m.into_iter().map(|(f, c)| (f, c.nodes, c.definitions)).collect::<Vec<_>>();
        assert_eq!(seen(ids), vec![("AC".to_string(), 1, 1), ("REQ".to_string(), 1, 1)]);
        assert_eq!(seen(milestones), vec![("BE".to_string(), 2, 2)]);
    }

    #[test]
    fn every_family_the_graph_holds_is_a_row_with_its_first_node_as_its_site() {
        let r = report(&graph_with(&["REQ-7", "AC-3"]), one("см. REQ-7 и даты по ISO-8601\n"));
        assert_eq!(r.families.iter().map(|f| (f.family.as_str(), f.nodes, at(&f.defined))).collect::<Vec<_>>(),
            vec![("AC", 1, "docs/a.md:1".to_string()), ("REQ", 1, "docs/a.md:1".to_string())]);
        // The other half: a prefix the tree writes and the graph holds no node in.
        assert_eq!(r.mention_only.iter().map(|m| m.prefix.as_str()).collect::<Vec<_>>(), vec!["ISO"]);
    }

    #[test]
    fn a_prefix_written_in_both_forms_takes_its_site_from_its_own_side() {
        // `BE-1` and `BE-M01` are one prefix and two families; neither row may take its line off
        // the other side's node.
        let mut g = Graph::default();
        let mut e = Extraction::default();
        e.node(NodeKind::Requirement, "BE-1", "требование", "", "docs/req.md", 4);
        e.node(NodeKind::Milestone, "BE-M01", "веха", "", "docs/plan.md", 9);
        g.apply(e);
        let r = report(&g, Vec::new());
        assert_eq!(at(&r.families[0].defined), "docs/req.md:4");
        assert_eq!(at(&r.milestones[0].defined), "docs/plan.md:9");
    }

    #[test]
    fn a_family_gained_and_one_lost_are_both_named() {
        let before = of_graph(&graph_with(&["AC-3", "REQ-7"]));
        let after = of_graph(&graph_with(&["REQ-7", "BE-M01"]));
        assert_eq!(moved(&before, &after), vec!["-AC", "+BE milestone"]);
    }

    #[test]
    fn the_build_line_names_the_families_and_elides_a_long_list() {
        assert_eq!(line(&of_graph(&graph_with(&["REQ-1", "BE-M01"]))), "families: REQ · milestones: BE");
        let ids: Vec<String> = (0..25).map(|i| format!("F{i:02}-1")).collect();
        let g = graph_with(&ids.iter().map(String::as_str).collect::<Vec<_>>());
        let l = line(&of_graph(&g));
        assert!(l.starts_with("families: F00, F01,"), "{l}");
        assert!(l.contains("… +5 · milestones: (none)"), "{l}");
    }

    #[test]
    fn a_task_is_written_in_its_milestones_family() {
        assert_eq!(classify("BE-M10/T05"), Some(Family::Milestone("BE")));
        assert_eq!(classify("BE-M10"), Some(Family::Milestone("BE")));
        assert_eq!(classify("FR-PAY-22"), Some(Family::Id("FR-PAY")));
        assert_eq!(classify("N-151"), Some(Family::Id("N")));
        assert_eq!(classify("file:docs/a.md"), None);
        // A task written with a hyphen where the dialect writes a slash is neither shape.
        assert_eq!(classify("MOB-M01-T3"), None);
        // The hyphen before the digits is what keeps a bare label out of the id slot.
        assert_eq!(classify("B1"), None);
        assert_eq!(classify("C11"), None);
        assert_eq!(classify("SECURITY-12"), Some(Family::Id("SECURITY")));
        assert_eq!(classify("FR-PAY-EU-1"), Some(Family::Id("FR-PAY-EU")));
    }

    /// The fixed point every row rests on: the prefix a definition head puts into a node's id is
    /// the family `classify` reads back off that node. Broken, the report names families no id in
    /// the graph is written in, for ever.
    #[test]
    fn a_long_prefix_and_a_three_part_one_survive_the_round_trip() {
        use crate::model::Extractor;
        let text = "**SECURITY-12 · MUST · доступ**\n\nтело\n\n## FR-PAY-EU-1 · возврат в ЕС\n\nтело\n";
        assert_eq!(cited(&one(text)), vec!["FR-PAY-EU", "SECURITY"]);

        let ex = crate::doc::DocExtractor::new().extract("docs/a.md", text);
        let mut written: Vec<&str> = ex.nodes.iter().map(|n| n.id.as_str()).filter(|id| !id.contains(':')).collect();
        written.sort_unstable();
        assert_eq!(written, vec!["FR-PAY-EU-1", "SECURITY-12"]);

        let mut g = Graph::default();
        g.apply(ex);
        assert_eq!(of_graph(&g).0.keys().collect::<Vec<_>>(), vec!["FR-PAY-EU", "SECURITY"]);
    }
}
