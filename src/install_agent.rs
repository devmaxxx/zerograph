//! `zerograph install-agent`: wires a coding agent to this repository's index. Claude Code and
//! Codex get the zerograph plugin, installed through their own `plugin` commands — the MCP server,
//! the session hook, the commands and the skill all ship in it — and an older install's files in
//! the repository are taken out once the plugin is in. Cursor, VS Code with Copilot, Gemini CLI and
//! opencode, which have no plugin to carry them, get an MCP entry and the stanza.
//!
//! Idempotent by construction: a second run rewrites the same bytes and reports nothing written.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const STANZA: &str = include_str!("../agent/stanza.md");
const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What an earlier release wrote into a repository for Claude Code and Codex, kept so a re-run can
/// tell its own files from a team's before it takes them out.
const LEGACY_RULE: &str = include_str!("../agent/legacy/rule.txt");
const LEGACY_SKILL: &str = include_str!("../agent/legacy/skill.md");
/// The skill as 0.6.0–0.6.3 wrote it, under the name `repo-query`.
const LEGACY_REPO_QUERY: &str = include_str!("../agent/legacy/repo-query.md");
const LEGACY_SCOUT: &str = include_str!("../agent/legacy/repo-scout.md");
const LEGACY_HOOK: &str = "zerograph-hook.mjs";

/// The repository both harnesses read the marketplace from, and the plugin's id in it.
const MARKETPLACE: &str = "devmaxxx/zerograph";
const PLUGIN: &str = "zerograph@zerograph";

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target { Claude, Codex }

impl Target {
    fn dir(self) -> &'static str {
        match self { Target::Claude => ".claude", Target::Codex => ".codex" }
    }

    pub fn cli(self) -> &'static str {
        match self { Target::Claude => "claude", Target::Codex => "codex" }
    }

    /// The commands that install the plugin, each safe to repeat. Claude Code's marketplace pins
    /// the plugin to a release tag itself; Codex reads the plugin out of the marketplace's own
    /// checkout, so the release is pinned on the marketplace, to the one this binary is.
    fn plugin_steps(self) -> Vec<Vec<String>> {
        let s = |args: &[&str]| args.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        match self {
            Target::Claude => vec![s(&["plugin", "marketplace", "add", MARKETPLACE]), s(&["plugin", "install", PLUGIN])],
            Target::Codex => vec![
                s(&["plugin", "marketplace", "add", MARKETPLACE, "--ref", &format!("v{VERSION}")]),
                s(&["plugin", "add", PLUGIN]),
            ],
        }
    }
}

#[derive(Debug, Default)]
pub struct Report { pub written: usize, pub paths: Vec<String>, pub notes: Vec<String> }

/// How a CLI is run: `program`, in `cwd`, with `args`; `None` when it is not installed. A test
/// passes its own.
pub type Run<'a> = &'a dyn Fn(&str, &Path, &[String]) -> Option<std::process::Output>;

pub fn run_cli(program: &str, cwd: &Path, args: &[String]) -> Option<std::process::Output> {
    std::process::Command::new(program).args(args).current_dir(cwd).stdin(std::process::Stdio::null()).output().ok()
}

/// Installs the plugin through the harness's own CLI, and says whether it is in. A missing CLI or
/// a failed step is a note naming the commands to run by hand, not an error: the other targets
/// and the git clean-up still run.
pub fn install_plugin(root: &Path, target: Target, run: Run) -> (bool, Report) {
    let mut report = Report::default();
    let cli = target.cli();
    let steps = target.plugin_steps();
    let by_hand = steps.iter().map(|s| format!("  {cli} {}", s.iter().map(|a| shell_word(a)).collect::<Vec<_>>().join(" "))).collect::<Vec<_>>().join("\n");
    for step in &steps {
        match run(cli, root, step) {
            None => {
                report.notes.push(format!("`{cli}` is not on PATH, so the plugin was not installed; run these where it is, then run this again to take out the older install:\n{by_hand}"));
                return (false, report);
            }
            Some(out) if !out.status.success() => {
                let said = [&out.stderr[..], &out.stdout[..]].iter().map(|b| String::from_utf8_lossy(b).trim().to_string()).find(|s| !s.is_empty()).unwrap_or_default();
                report.notes.push(format!("`{cli} {}` failed ({said}), so the plugin was not installed; the commands are:\n{by_hand}", step.join(" ")));
                return (false, report);
            }
            Some(_) => {}
        }
    }
    report.notes.push(format!("the {PLUGIN} plugin is installed for this user; it starts with the next session"));
    (true, report)
}

const BEGIN: &str = "<!-- zerograph:begin -->";
const END: &str = "<!-- zerograph:end -->";

/// Where a marked block starts, and where its end marker ends.
fn marked(text: &str, begin: &str, end: &str) -> Option<(usize, usize)> {
    let a = text.find(begin)?;
    let b = a + text[a..].find(end)?;
    Some((a, b + end.len()))
}

fn skip_break(text: &str) -> &str {
    text.strip_prefix("\r\n").or_else(|| text.strip_prefix('\n')).unwrap_or(text)
}

/// A text with `{{command}}` resolved to how this repository invokes the binary — `zerograph`
/// unless a caller says otherwise, `pnpm exec zerograph` in a workspace that installs it as a
/// dependency. A line naming the bare binary where PATH has none tells an agent to run something
/// that is not there.
fn with_command(template: &str, command: &str) -> String { template.replace("{{command}}", command) }

/// The instructions file with our stanza in it: replacing what is between the markers when they are
/// there, appending when they are not. Everything outside the markers is the repository's own and
/// is never touched — an installer that rewrote CLAUDE.md would be a worse citizen than no
/// installer at all.
pub fn merged_instructions(existing: &str, command: &str) -> String {
    let block = with_command(STANZA, command);
    match marked(existing, BEGIN, END) {
        Some((a, b)) => {
            let mut out = String::with_capacity(existing.len() + block.len());
            out.push_str(&existing[..a]);
            out.push_str(block.trim_end());
            out.push_str(&existing[b..]);
            out
        }
        None => {
            let mut out = existing.to_string();
            if !out.is_empty() && !out.ends_with('\n') { out.push('\n'); }
            if !out.is_empty() { out.push('\n'); }
            out.push_str(&block);
            out
        }
    }
}

/// `existing` without our stanza and the blank line that set it off, or `None` when it holds none.
pub fn without_stanza(existing: &str) -> Option<String> {
    let (a, b) = marked(existing, BEGIN, END)?;
    let head = existing[..a].trim_end_matches(['\n', '\r']);
    let tail = skip_break(&existing[b..]);
    Some(match (head.is_empty(), tail.trim().is_empty()) {
        (true, _) => tail.trim_start_matches(['\n', '\r']).to_string(),
        (false, true) => format!("{head}\n"),
        (false, false) => format!("{head}\n\n{}", tail.trim_start_matches(['\n', '\r'])),
    })
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// A repository can ship a file as a link to one elsewhere on the machine, and a write through it
/// would overwrite that file with ours.
fn refuse_symlink(path: &Path) -> Result<()> {
    if is_symlink(path) {
        anyhow::bail!("{} is a symlink; refusing to write through it — replace it with a regular file and run again", path.display());
    }
    Ok(())
}

/// Writes `text` at `path` unless the same bytes are already there, and says which it did.
fn write_if_changed(path: &Path, text: &str, report: &mut Report) -> Result<()> {
    // Checked after the no-op test: a `CLAUDE.md` linked to `AGENTS.md` that already holds our
    // block needs no write, and refusing it would fail every re-run for nothing.
    if std::fs::read_to_string(path).is_ok_and(|old| old == text) { return Ok(()); }
    refuse_symlink(path)?;
    if let Some(parent) = path.parent() { std::fs::create_dir_all(parent)?; }
    std::fs::write(path, text).with_context(|| format!("write {}", path.display()))?;
    report.written += 1;
    report.paths.push(path.display().to_string());
    Ok(())
}

/// Removes `path`, counted and named, or says why it stayed.
fn remove(path: &Path, why: &str, report: &mut Report) {
    match std::fs::remove_file(path) {
        Ok(()) => {
            report.written += 1;
            report.paths.push(path.display().to_string());
            report.notes.push(format!("removed {}: {why}", path.display()));
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => report.notes.push(format!("{} could not be removed ({e}); {why}, so remove it by hand", path.display())),
    }
}

/// Removes `dir` when nothing is left in it.
fn remove_if_empty(dir: &Path) {
    if std::fs::read_dir(dir).is_ok_and(|mut d| d.next().is_none()) { let _ = std::fs::remove_dir(dir); }
}

/// The release a generated skill names in its front matter.
fn skill_version(text: &str) -> Option<&str> {
    let front = text.strip_prefix("---\n")?.split("\n---\n").next()?;
    front.lines().skip_while(|l| *l != "metadata:").find_map(|l| l.trim().strip_prefix("version:")).map(|v| v.trim().trim_matches('"'))
}

/// Whether `text` is `template` with `{{command}}` resolved to one command throughout, whatever
/// command the install that wrote it was given.
fn generated_from(template: &str, text: &str) -> bool {
    let mut parts = template.split("{{command}}");
    let (Some(head), Some(next)) = (parts.next(), parts.next()) else { return template == text };
    let Some(rest) = text.strip_prefix(head) else { return false };
    let Some(end) = rest.find(next) else { return false };
    with_command(template, &rest[..end]) == text
}

/// Removes a skill directory an earlier release wrote: only one holding nothing but the `SKILL.md`
/// `ours` recognises. An edited one, or one with files of its own, is a team's skill under that
/// name, and is left and named.
fn retire_skill(dir: &Path, ours: &dyn Fn(&str) -> bool, report: &mut Report) {
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return };
    let file = dir.join("SKILL.md");
    let only_skill = meta.is_dir() && std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.filter_map(|e| e.ok()).all(|e| e.file_name() == "SKILL.md" && e.file_type().is_ok_and(|t| t.is_file()))
    });
    if !(only_skill && std::fs::read_to_string(&file).is_ok_and(|t| ours(&t))) {
        report.notes.push(format!("{} is not the skill an earlier zerograph wrote, so it was left; the plugin's skill replaces it", dir.display()));
        return;
    }
    remove(&file, "the plugin carries the skill now", report);
    let _ = std::fs::remove_dir(dir);
}

/// The settings file without the hook entries an earlier release merged in, found by the script
/// they run, or `None` when it holds none. An event left with no entry goes, and `hooks` with it.
pub fn without_hook_entries(existing: &str) -> Result<Option<String>> {
    if existing.trim().is_empty() { return Ok(None); }
    let mut root: serde_json::Value = serde_json::from_str(existing).context("not JSON")?;
    let Some(hooks) = root.get_mut("hooks").and_then(|h| h.as_object_mut()) else { return Ok(None) };
    let mut found = false;
    for list in hooks.values_mut() {
        let Some(arr) = list.as_array_mut() else { continue };
        let before = arr.len();
        arr.retain(|e| !e.to_string().contains(LEGACY_HOOK));
        found |= arr.len() < before;
    }
    if !found { return Ok(None); }
    hooks.retain(|_, list| list.as_array().is_none_or(|a| !a.is_empty()));
    if hooks.is_empty() { root.as_object_mut().unwrap().remove("hooks"); }
    Ok(Some(serde_json::to_string_pretty(&root)? + "\n"))
}

/// Takes our hook entries out of a settings file, removing the file when they were all it held.
fn retire_hook_entries(path: &Path, report: &mut Report) {
    let Ok(existing) = std::fs::read_to_string(path) else { return };
    match without_hook_entries(&existing) {
        Ok(None) => {}
        Ok(Some(rest)) if rest.trim() == "{}" => remove(path, "it held only the hook entries an earlier release wrote, and the plugin runs the hook now", report),
        Ok(Some(_)) if is_symlink(path) => report.notes.push(format!("{} is a symlink, so the hook entries an earlier release wrote were left in it; take out the ones that run {LEGACY_HOOK}", path.display())),
        Ok(Some(rest)) => match std::fs::write(path, rest) {
            Ok(()) => {
                report.written += 1;
                report.paths.push(path.display().to_string());
                report.notes.push(format!("{}: took out the hook entries an earlier release wrote; the plugin runs the hook now", path.display()));
            }
            Err(e) => report.notes.push(format!("{} could not be rewritten ({e}); take out the entries that run {LEGACY_HOOK}", path.display())),
        },
        Err(e) => report.notes.push(format!("{} is left as it is ({e:#}); take out the entries that run {LEGACY_HOOK}", path.display())),
    }
}

/// Takes our stanza out of an instructions file, removing the file when it was all it held.
fn retire_stanza(path: &Path, report: &mut Report) {
    let Ok(existing) = std::fs::read_to_string(path) else { return };
    let Some(rest) = without_stanza(&existing) else { return };
    if is_symlink(path) {
        report.notes.push(format!("{} is a symlink, so the stanza an earlier release wrote was left in what it points to; the plugin's skill replaces it", path.display()));
        return;
    }
    if rest.trim().is_empty() {
        remove(path, "it held only the stanza an earlier release wrote, and the plugin's skill replaces it", report);
        return;
    }
    match std::fs::write(path, rest) {
        Ok(()) => {
            report.written += 1;
            report.paths.push(path.display().to_string());
            report.notes.push(format!("{}: took out the stanza an earlier release wrote; the plugin's skill replaces it", path.display()));
        }
        Err(e) => report.notes.push(format!("{} could not be rewritten ({e}); remove the lines between `{BEGIN}` and `{END}` by hand", path.display())),
    }
}

/// Whether opencode in `root` still runs our server, which reads the stanza from `AGENTS.md`.
fn opencode_wants_stanza(root: &Path) -> bool {
    std::fs::read_to_string(root.join(Editor::Opencode.mcp_file())).ok()
        .and_then(|t| serde_json::from_str::<serde_json::Value>(&t).ok())
        .is_some_and(|v| v[Editor::Opencode.servers_key()].get(MCP_NAME).is_some())
}

/// Removes the hook script an earlier release wrote into `hooks`, and its rule while it is still the
/// text that release generated.
pub fn retire_hook_script(hooks: &Path, report: &mut Report) {
    remove(&hooks.join(LEGACY_HOOK), "the plugin runs the session hook now", report);
    let rule = hooks.join("rule.txt");
    match std::fs::read_to_string(&rule) {
        Ok(t) if generated_from(LEGACY_RULE, &t) => remove(&rule, "the plugin's hook carries the rule", report),
        Ok(_) => report.notes.push(format!("{} was edited, so it was left; the plugin's hook carries its own rule and never reads it", rule.display())),
        Err(_) => {}
    }
    remove_if_empty(hooks);
}

/// Takes out of `root` what an earlier `install-agent` wrote for `target`, now that the plugin
/// carries it: the hook script and its rule, the hook entries in `.claude/settings.json`, the skill
/// and the subagent where they are still the text that release generated, and the stanza. Run only
/// once the plugin is in, so a session is never left with neither.
pub fn retire_repo_install(root: &Path, target: Target) -> Report {
    let mut report = Report::default();
    let base = root.join(target.dir());
    retire_hook_script(&base.join("hooks"), &mut report);
    if target == Target::Claude { retire_hook_entries(&base.join("settings.json"), &mut report); }

    let skills = base.join("skills");
    let generated = |t: &str| skill_version(t).is_some_and(|v| generated_from(&LEGACY_SKILL.replace("{{version}}", v), t));
    if skills.join("zerograph").exists() { retire_skill(&skills.join("zerograph"), &generated, &mut report); }
    if skills.join("repo-query").exists() { retire_skill(&skills.join("repo-query"), &|t| generated_from(LEGACY_REPO_QUERY, t), &mut report); }
    remove_if_empty(&skills);
    let scout = base.join("agents").join("repo-scout.md");
    match std::fs::read_to_string(&scout) {
        Ok(t) if generated_from(LEGACY_SCOUT, &t) => remove(&scout, "the plugin carries the subagent now", &mut report),
        Ok(_) => report.notes.push(format!("{} is not the subagent an earlier zerograph wrote, so it was left; the plugin's repo-scout replaces it", scout.display())),
        Err(_) => {}
    }
    remove_if_empty(&base.join("agents"));

    match target {
        Target::Claude => for file in [root.join("CLAUDE.md"), base.join("CLAUDE.md")] { retire_stanza(&file, &mut report) },
        Target::Codex if opencode_wants_stanza(root) => {}
        Target::Codex => retire_stanza(&root.join("AGENTS.md"), &mut report),
    }
    remove_if_empty(&base);
    report
}

/// Codex's home: `CODEX_HOME` when set, `~/.codex` otherwise.
pub fn codex_home() -> Option<PathBuf> {
    match std::env::var_os("CODEX_HOME").filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir)),
        None => Some(std::env::home_dir()?.join(".codex")),
    }
}

/// Takes out of Codex's machine hooks file the entries an earlier release printed for a person to
/// paste in: with the plugin's hook beside them, the hook would answer every event twice.
pub fn retire_codex_hooks(home: &Path) -> Report {
    let mut report = Report::default();
    let path = home.join("hooks.json");
    let Ok(seen) = std::fs::metadata(&path) else { return report };
    let Ok(existing) = std::fs::read_to_string(&path) else { return report };
    match without_hook_entries(&existing) {
        Ok(None) => {}
        Ok(Some(rest)) => match replace_config(&path, &seen, &rest) {
            Ok(()) => {
                report.written += 1;
                report.paths.push(path.display().to_string());
                report.notes.push(format!("{}: took out the entries that ran {LEGACY_HOOK}; the plugin runs the hook now", path.display()));
            }
            Err(e) => report.notes.push(format!("{} could not be rewritten ({e:#}); take out the entries that run {LEGACY_HOOK}", path.display())),
        },
        Err(e) => report.notes.push(format!("{} is left as it is ({e:#}); take out the entries that run {LEGACY_HOOK}", path.display())),
    }
    report
}

/// An editor or CLI that reads a project MCP file and an instructions file, and none of the
/// hooks, skills or subagents above: for these the install is the server entry and the stanza.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Editor { Cursor, Copilot, Gemini, Opencode }

impl Editor {
    fn mcp_file(self) -> &'static str {
        match self {
            Editor::Cursor => ".cursor/mcp.json",
            Editor::Copilot => ".vscode/mcp.json",
            Editor::Gemini => ".gemini/settings.json",
            Editor::Opencode => "opencode.json",
        }
    }

    fn servers_key(self) -> &'static str {
        match self {
            Editor::Cursor | Editor::Gemini => "mcpServers",
            Editor::Copilot => "servers",
            Editor::Opencode => "mcp",
        }
    }

    /// The server entry for `argv`. These files are committed and shared, so no absolute path
    /// goes in them: Cursor and VS Code expand `${workspaceFolder}` themselves, and Gemini CLI
    /// and opencode start the server in the directory they were started in, which is the
    /// repository root `zerograph` already defaults to.
    fn server(self, argv: &[String]) -> serde_json::Value {
        let (program, rest) = argv.split_first().expect("argv is never empty");
        let workspace = ["--repo", "${workspaceFolder}", "mcp"];
        let args = |tail: &[&str]| rest.iter().cloned().chain(tail.iter().map(|s| s.to_string())).collect::<Vec<String>>();
        match self {
            Editor::Cursor => serde_json::json!({ "command": program, "args": args(&workspace) }),
            Editor::Copilot => serde_json::json!({ "type": "stdio", "command": program, "args": args(&workspace) }),
            Editor::Gemini => serde_json::json!({ "command": program, "args": args(&["mcp"]) }),
            Editor::Opencode => serde_json::json!({
                "type": "local",
                "command": std::iter::once(program.clone()).chain(args(&["mcp"])).collect::<Vec<_>>(),
                "enabled": true,
            }),
        }
    }

    /// The always-on instructions file. Cursor ignores plain markdown under `.cursor/rules/`, so
    /// its stanza is a rule file of our own; opencode reads the root `AGENTS.md`, as Codex does.
    fn instructions(self) -> &'static str {
        match self {
            Editor::Cursor => ".cursor/rules/zerograph.mdc",
            Editor::Copilot => ".github/copilot-instructions.md",
            Editor::Gemini => "GEMINI.md",
            Editor::Opencode => "AGENTS.md",
        }
    }
}

const CURSOR_RULE_HEADER: &str = "---\ndescription: Query the zerograph index before reading files\nalwaysApply: true\n---\n\n";

/// The project MCP file with our server entry in it, every other key and server kept. A file
/// that is not plain JSON (VS Code allows comments in `mcp.json`) is an error the caller turns
/// into a note, so a hand-kept file is never rewritten without its comments.
pub fn merged_mcp_file(existing: &str, editor: Editor, argv: &[String]) -> Result<String> {
    let mut root: serde_json::Value = match existing.trim().is_empty() {
        true => match editor {
            Editor::Opencode => serde_json::json!({ "$schema": "https://opencode.ai/config.json" }),
            _ => serde_json::json!({}),
        },
        false => serde_json::from_str(existing).context("not plain JSON")?,
    };
    let Some(object) = root.as_object_mut() else { anyhow::bail!("not a JSON object") };
    let key = editor.servers_key();
    let servers = object.entry(key).or_insert_with(|| serde_json::json!({}));
    let Some(servers) = servers.as_object_mut() else { anyhow::bail!("`{key}` is not an object") };
    servers.insert(MCP_NAME.into(), editor.server(argv));
    Ok(serde_json::to_string_pretty(&root)? + "\n")
}

/// Installs the MCP entry and the stanza for one editor. Neither failure stops the other: a
/// commented `mcp.json` still gets its instructions, and says what to paste.
pub fn install_editor(root: &Path, editor: Editor, command: &str) -> Result<Report> {
    let mut report = Report::default();
    let argv = split_words(command);
    if argv.is_empty() { anyhow::bail!("--command is empty"); }

    let mcp = root.join(editor.mcp_file());
    // Only a missing file is empty: one that exists but cannot be read must not be mistaken for
    // blank and rewritten from scratch over whatever it holds.
    let existing = match std::fs::read_to_string(&mcp) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(anyhow::Error::from(e).context("unreadable")),
    };
    let merged = existing.and_then(|text| merged_mcp_file(&text, editor, &argv));
    let unchanged = merged.as_ref().is_ok_and(|m| std::fs::read_to_string(&mcp).is_ok_and(|old| &old == m));
    match merged {
        Ok(_) if !unchanged && is_symlink(&mcp) => report.notes.push(format!(
            "{} is a symlink, so the server entry was not written through it", mcp.display())),
        Ok(merged) => write_if_changed(&mcp, &merged, &mut report)?,
        Err(e) => report.notes.push(format!(
            "{} is left as it is ({e:#}); add this under `{}`:\n\"{MCP_NAME}\": {}",
            mcp.display(), editor.servers_key(), serde_json::to_string_pretty(&editor.server(&argv))?)),
    }

    let instructions = root.join(editor.instructions());
    let text = match editor {
        Editor::Cursor => format!("{CURSOR_RULE_HEADER}{}", with_command(STANZA, command)),
        _ => merged_instructions(&std::fs::read_to_string(&instructions).unwrap_or_default(), command),
    };
    let unchanged = std::fs::read_to_string(&instructions).is_ok_and(|old| old == text);
    match !unchanged && is_symlink(&instructions) {
        true => report.notes.push(format!(
            "{} is a symlink, so the stanza was not written through it", instructions.display())),
        false => write_if_changed(&instructions, &text, &mut report)?,
    }
    Ok(report)
}

const MCP_NAME: &str = "zerograph";

/// The words of a command line the way a shell would split it, for the quotes and backslashes a
/// `--command` like `pnpm exec "my zerograph"` needs: the registered argv has to be the one the
/// person meant, not whatever whitespace splitting makes of it.
fn split_words(command: &str) -> Vec<String> {
    let (mut words, mut word, mut quote, mut started) = (Vec::new(), String::new(), None::<char>, false);
    let mut chars = command.chars();
    while let Some(c) = chars.next() {
        match (c, quote) {
            ('\\', Some('"') | None) => { word.extend(chars.next()); started = true; }
            (q, Some(open)) if q == open => quote = None,
            (q @ ('"' | '\''), None) => { quote = Some(q); started = true; }
            (c, None) if c.is_whitespace() => if started { words.push(std::mem::take(&mut word)); started = false; },
            (c, _) => { word.push(c); started = true; }
        }
    }
    if started { words.push(word); }
    words
}

/// The worktrees `git worktree list --porcelain` names, as (path, bare), main worktree first.
fn parse_worktrees(porcelain: &str) -> Vec<(PathBuf, bool)> {
    porcelain.split("\n\n").filter_map(|block| {
        let path = block.lines().find_map(|l| l.strip_prefix("worktree "))?;
        Some((PathBuf::from(path), block.lines().any(|l| l == "bare")))
    }).collect()
}

fn listed_worktrees(root: &Path) -> Vec<(PathBuf, bool)> {
    crate::git_command().arg("-C").arg(root).args(["worktree", "list", "--porcelain"])
        .stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()
        .filter(|o| o.status.success())
        .map(|o| parse_worktrees(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or_default()
}

/// Every checkout of the clone `root` belongs to that is still on disk, `root` among them, each
/// canonical as `--repo` is. Claude Code keys a local-scope server by the project's path, so a
/// server registered for one checkout is not there in a session opened in another.
pub fn worktrees(root: &Path) -> Vec<PathBuf> {
    let mut all: Vec<PathBuf> = listed_worktrees(root).into_iter()
        .filter(|(_, bare)| !bare)
        .filter_map(|(path, _)| canonical(&path))
        .collect();
    let root = canonical(root).unwrap_or_else(|| root.to_path_buf());
    if !all.contains(&root) { all.insert(0, root); }
    all
}

/// The path as `--repo` resolves it, without the `\\?\` prefix `canonicalize` puts on a Windows
/// path: Claude Code keys a project by the path as written, and an earlier registration said it
/// without one.
fn canonical(path: &Path) -> Option<PathBuf> {
    path.canonicalize().ok().map(crate::plain)
}

/// The main worktree of the clone `root` belongs to: the one that stays while linked worktrees
/// come and go. `None` for a bare clone, or where git cannot say.
pub fn main_worktree(root: &Path) -> Option<PathBuf> {
    let (path, bare) = listed_worktrees(root).into_iter().next()?;
    if bare { return None; }
    canonical(&path)
}

/// The file Claude Code keeps its local-scope servers in: `.claude.json` in `CLAUDE_CONFIG_DIR`
/// when that is set, in the home directory otherwise.
pub fn claude_config() -> Option<PathBuf> {
    match std::env::var_os("CLAUDE_CONFIG_DIR").filter(|v| !v.is_empty()) {
        Some(dir) => Some(PathBuf::from(dir).join(".claude.json")),
        None => Some(std::env::home_dir()?.join(".claude.json")),
    }
}

/// Whether a registered server is one an earlier `install-agent` made: it runs `… --repo <path> mcp`.
fn registered_by_us(server: &serde_json::Value) -> bool {
    let Some(args) = server["args"].as_array() else { return false };
    args.last().and_then(|a| a.as_str()) == Some("mcp") && args.iter().any(|a| a.as_str() == Some("--repo"))
}

/// Drops the local-scope server an earlier `install-agent` registered from Claude Code's `config`
/// file, for every checkout in `replaced` — the plugin's server answers there now, and two would
/// both be offered — and for any checkout no longer on disk. `claude mcp remove` cannot do either
/// in one go: it finds the project by its working directory, once per checkout, and a removed one
/// has none. Only our entry goes; everything else in the file is kept, though its keys come back in
/// sorted order. The file is written beside itself and renamed over, with its permissions, and not
/// at all when there is nothing to prune.
pub fn prune_mcp(config: &Path, replaced: &[PathBuf]) -> Report {
    let mut report = Report::default();
    // Written through a link to where it points, so a dotfile manager's link stays one.
    let config = &config.canonicalize().unwrap_or_else(|_| config.to_path_buf());
    let Ok(seen) = std::fs::metadata(config) else { return report };
    let Ok(text) = std::fs::read_to_string(config) else { return report };
    let Ok(mut root) = serde_json::from_str::<serde_json::Value>(&text) else {
        report.notes.push(format!("{} is not JSON, so no registration for a removed checkout was pruned from it", config.display()));
        return report;
    };
    let Some(projects) = root.get_mut("projects").and_then(|p| p.as_object_mut()) else { return report };
    let mut pruned = Vec::new();
    for (path, project) in projects.iter_mut() {
        // Only a path known to be gone: one that cannot be looked at now (a mount that is down, a
        // directory without permission) may still be there.
        let gone = matches!(Path::new(path).try_exists(), Ok(false));
        if !gone && !replaced.iter().any(|r| r == Path::new(path)) { continue; }
        let Some(servers) = project.get_mut("mcpServers").and_then(|s| s.as_object_mut()) else { continue };
        let before = servers.len();
        servers.retain(|name, server| !(name == MCP_NAME && registered_by_us(server)));
        if servers.len() < before { pruned.push((path.clone(), gone)); }
    }
    if pruned.is_empty() { return report; }
    let written = serde_json::to_string_pretty(&root).map_err(anyhow::Error::from).and_then(|out| replace_config(config, &seen, &(out + "\n")));
    match written {
        Ok(()) => {
            report.written = 1;
            report.paths.push(config.display().to_string());
            for (path, gone) in pruned {
                report.notes.push(match gone {
                    true => format!("removed the MCP server registered for {path}, which is no longer on disk"),
                    false => format!("removed the MCP server an earlier release registered for {path}; the plugin's server replaces it"),
                });
            }
        }
        Err(e) => report.notes.push(format!("{} could not be rewritten ({e:#}); the MCP servers registered for {} stay", config.display(),
            pruned.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>().join(", "))),
    }
    report
}

/// Puts `text` in place of `config`, unless the file changed since it was read as `seen`: a Claude
/// Code session writes the same file, and its write must not be lost to ours. The new copy is
/// created with the old one's permissions from the start, since the file holds account data, under
/// a name no other run picks, and removed when anything fails.
fn replace_config(config: &Path, seen: &std::fs::Metadata, text: &str) -> Result<()> {
    use std::io::Write;
    let name = config.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = config.with_file_name(format!(".{name}.zerograph-{}", std::process::id()));
    let mut open = std::fs::OpenOptions::new();
    open.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
        open.mode(seen.permissions().mode() & 0o777);
    }
    let result = (|| -> Result<()> {
        let mut file = open.open(&tmp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        let now = std::fs::metadata(config)?;
        if now.len() != seen.len() || now.modified().ok() != seen.modified().ok() {
            anyhow::bail!("it changed while being read, so it is left for the next run");
        }
        std::fs::rename(&tmp, config)?;
        Ok(())
    })();
    if result.is_err() { let _ = std::fs::remove_file(&tmp); }
    result
}

/// `word` as one shell word: bare when it has nothing the shell would split or expand, and
/// single-quoted otherwise, so a path with a space still pastes as the one argument it is.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty() && word.chars().all(|c| c.is_alphanumeric() || "-_./:=@%+,".contains(c));
    if plain { word.to_string() } else { format!("'{}'", word.replace('\'', "'\\''")) }
}

/// The git events an earlier release wrote a refresh block into.
const GIT_HOOKS: [&str; 2] = ["post-merge", "post-checkout"];
const GIT_MARKERS: (&str, &str) = ("# zerograph:begin", "# zerograph:end");

/// `existing` without the refresh block an earlier release wrote, or `None` when it holds none. A
/// hook left with nothing but its shebang comes back empty: the block was all that file was for.
pub fn without_git_block(existing: &str) -> Option<String> {
    let mut text = existing.to_string();
    let mut found = false;
    while let Some((a, b)) = marked(&text, GIT_MARKERS.0, GIT_MARKERS.1) {
        let tail = skip_break(&text[b..]).to_string();
        text = format!("{}{tail}", &text[..a]);
        found = true;
    }
    if !found { return None; }
    let body = text.split_once('\n').filter(|_| text.starts_with("#!")).map_or(text.as_str(), |(_, rest)| rest);
    Some(if body.trim().is_empty() { String::new() } else { text })
}

/// Takes the `post-merge` and `post-checkout` block an earlier release wrote out of the hooks git
/// runs. The block fired in every worktree, since hooks live in the common git directory, and git
/// runs `post-checkout` on `git worktree add`, so each new worktree started a full embed; every read
/// already refreshes, and the session hook builds a checkout that has no index. Both the directory
/// git runs hooks from (`core.hooksPath` when set) and the common `hooks/` are looked at, since a
/// hook manager may have moved the first since the block was written. The rest of each hook stays;
/// a hook that held nothing else is deleted.
pub fn retire_git_hooks(root: &Path) -> Report {
    let mut report = Report::default();
    let git_path = |args: &[&str]| crate::git_command().arg("-C").arg(root).args(args)
        .stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()
        .filter(|o| o.status.success())
        .map(|o| root.join(String::from_utf8_lossy(&o.stdout).trim()));
    let mut dirs: Vec<PathBuf> = [git_path(&["rev-parse", "--git-path", "hooks"]), git_path(&["rev-parse", "--git-common-dir"]).map(|d| d.join("hooks"))]
        .into_iter().flatten().collect();
    dirs.dedup_by(|a, b| a.canonicalize().ok().zip(b.canonicalize().ok()).is_some_and(|(a, b)| a == b) || a == b);
    for dir in dirs {
        for name in GIT_HOOKS {
            let path = dir.join(name);
            let Ok(existing) = std::fs::read_to_string(&path) else { continue };
            let Some(rest) = without_git_block(&existing) else { continue };
            let done = match rest.is_empty() {
                true => std::fs::remove_file(&path).map(|()| format!("removed {}, which held only the refresh block an earlier release wrote", path.display())),
                false => std::fs::write(&path, &rest).map(|()| format!("{}: took out the refresh block an earlier release wrote; the rest of the hook is as it was", path.display())),
            };
            match done {
                Ok(note) => { report.written += 1; report.paths.push(path.display().to_string()); report.notes.push(note); }
                Err(e) => report.notes.push(format!("{}: the refresh block an earlier release wrote could not be taken out ({e}); remove the lines between `# zerograph:begin` and `# zerograph:end` by hand", path.display())),
            }
        }
    }
    report
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> tempfile::TempDir { tempfile::tempdir().unwrap() }

    fn read_json(path: &Path) -> serde_json::Value {
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
    }

    #[test]
    fn each_editor_gets_its_server_entry_and_its_stanza_and_a_second_run_writes_nothing() {
        let dir = root();
        for editor in [Editor::Cursor, Editor::Copilot, Editor::Gemini, Editor::Opencode] {
            let first = install_editor(dir.path(), editor, "pnpm exec zerograph").unwrap();
            assert_eq!(first.written, 2, "{editor:?}: {:?}", first.paths);
            assert_eq!(install_editor(dir.path(), editor, "pnpm exec zerograph").unwrap().written, 0, "{editor:?}");
        }
        let cursor = read_json(&dir.path().join(".cursor/mcp.json"));
        assert_eq!(cursor["mcpServers"]["zerograph"], serde_json::json!({
            "command": "pnpm", "args": ["exec", "zerograph", "--repo", "${workspaceFolder}", "mcp"] }));
        let copilot = read_json(&dir.path().join(".vscode/mcp.json"));
        assert_eq!(copilot["servers"]["zerograph"]["type"], "stdio");
        assert_eq!(copilot["servers"]["zerograph"]["args"][3], "${workspaceFolder}");
        let gemini = read_json(&dir.path().join(".gemini/settings.json"));
        assert_eq!(gemini["mcpServers"]["zerograph"], serde_json::json!({ "command": "pnpm", "args": ["exec", "zerograph", "mcp"] }));
        let opencode = read_json(&dir.path().join("opencode.json"));
        assert_eq!(opencode["mcp"]["zerograph"], serde_json::json!({
            "type": "local", "command": ["pnpm", "exec", "zerograph", "mcp"], "enabled": true }));

        let rule = std::fs::read_to_string(dir.path().join(".cursor/rules/zerograph.mdc")).unwrap();
        assert!(rule.starts_with("---\n") && rule.contains("alwaysApply: true"), "{rule}");
        for file in [".cursor/rules/zerograph.mdc", ".github/copilot-instructions.md", "GEMINI.md", "AGENTS.md"] {
            let text = std::fs::read_to_string(dir.path().join(file)).unwrap();
            assert!(text.contains(BEGIN) && text.contains("pnpm exec zerograph ask"), "{file}");
        }
    }

    #[test]
    fn a_projects_own_servers_and_settings_survive_the_merge() {
        let dir = root();
        std::fs::create_dir_all(dir.path().join(".gemini")).unwrap();
        std::fs::write(dir.path().join(".gemini/settings.json"),
            r#"{"theme":"dark","mcpServers":{"other":{"command":"x"},"zerograph":{"command":"stale"}}}"#).unwrap();
        install_editor(dir.path(), Editor::Gemini, "zerograph").unwrap();
        let merged = read_json(&dir.path().join(".gemini/settings.json"));
        assert_eq!(merged["theme"], "dark");
        assert_eq!(merged["mcpServers"]["other"]["command"], "x");
        assert_eq!(merged["mcpServers"]["zerograph"]["command"], "zerograph");
    }

    #[test]
    fn a_commented_mcp_file_is_left_alone_and_the_entry_is_printed_instead() {
        let dir = root();
        let mcp = dir.path().join(".vscode/mcp.json");
        std::fs::create_dir_all(mcp.parent().unwrap()).unwrap();
        let mine = "{\n  // ours\n  \"servers\": {}\n}\n";
        std::fs::write(&mcp, mine).unwrap();
        let r = install_editor(dir.path(), Editor::Copilot, "zerograph").unwrap();
        assert_eq!(std::fs::read_to_string(&mcp).unwrap(), mine);
        assert!(r.notes.iter().any(|n| n.contains("\"zerograph\"") && n.contains("${workspaceFolder}")), "{:?}", r.notes);
        assert!(dir.path().join(".github/copilot-instructions.md").exists());
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_mcp_file_is_not_written_through_and_the_stanza_still_lands() {
        let dir = root();
        let elsewhere = dir.path().join("shared.json");
        std::fs::write(&elsewhere, "{}").unwrap();
        std::fs::create_dir_all(dir.path().join(".cursor")).unwrap();
        std::os::unix::fs::symlink(&elsewhere, dir.path().join(".cursor/mcp.json")).unwrap();
        let r = install_editor(dir.path(), Editor::Cursor, "zerograph").unwrap();
        assert_eq!(std::fs::read_to_string(&elsewhere).unwrap(), "{}");
        assert!(r.notes.iter().any(|n| n.contains("symlink")), "{:?}", r.notes);
        assert!(dir.path().join(".cursor/rules/zerograph.mdc").exists());
    }

    #[test]
    fn a_command_is_split_the_way_a_shell_would() {
        assert_eq!(split_words("zerograph"), ["zerograph"]);
        assert_eq!(split_words("pnpm exec  zerograph"), ["pnpm", "exec", "zerograph"]);
        assert_eq!(split_words(r#"pnpm exec "my repo graph" 'a b' c\ d"#), ["pnpm", "exec", "my repo graph", "a b", "c d"]);
        assert_eq!(split_words(r#"x "" y"#), ["x", "", "y"]);
        assert!(split_words("   ").is_empty());
    }

    fn git_repo() -> tempfile::TempDir {
        let dir = root();
        let ok = std::process::Command::new("git").arg("-C").arg(dir.path()).args(["init", "-q"])
            .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").status().unwrap().success();
        assert!(ok, "git init");
        dir
    }

    fn git(dir: &Path, args: &[&str]) {
        let ok = std::process::Command::new("git").arg("-C").arg(dir).args(args)
            .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null())
            .status().unwrap().success();
        assert!(ok, "git {args:?}");
    }

    #[test]
    fn the_porcelain_listing_is_read_main_first_with_a_bare_clone_marked() {
        let listed = parse_worktrees("worktree /a\nHEAD 1\nbranch refs/heads/main\n\nworktree /a b/wt\nHEAD 2\ndetached\n\nworktree /bare\nbare\n");
        assert_eq!(listed, [(PathBuf::from("/a"), false), (PathBuf::from("/a b/wt"), false), (PathBuf::from("/bare"), true)]);
        assert!(parse_worktrees("").is_empty());
    }

    #[test]
    fn every_checkout_of_the_clone_is_found_from_any_of_them_and_the_main_one_named() {
        let dir = git_repo();
        let main = dir.path().join("main");
        std::fs::create_dir(&main).unwrap();
        git(&main, &["init", "-q"]);
        git(&main, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-q", "--allow-empty", "-m", "x"]);
        let wt = dir.path().join("wt");
        git(&main, &["worktree", "add", "-q", "--detach", wt.to_str().unwrap()]);
        let (main, wt) = (canonical(&main).unwrap(), canonical(&wt).unwrap());
        assert_eq!(worktrees(&wt), [main.clone(), wt.clone()]);
        assert_eq!(main_worktree(&wt), Some(main.clone()));
        assert_eq!(main_worktree(&main), Some(main));
        let lone = root();
        assert_eq!(worktrees(lone.path()), [canonical(lone.path()).unwrap()], "outside git, the checkout itself");
        assert_eq!(main_worktree(lone.path()), None);
    }

    #[test]
    fn a_registration_for_a_checkout_that_is_gone_is_pruned_and_every_other_entry_kept() {
        let dir = root();
        let live = dir.path().to_str().unwrap().to_string();
        let gone = dir.path().join("removed-worktree").to_str().unwrap().to_string();
        let ours = |p: &str| serde_json::json!({ "type": "stdio", "command": "pnpm", "args": ["exec", "zerograph", "--repo", p, "mcp"] });
        let config = dir.path().join(".claude.json");
        let before = serde_json::json!({
            "numStartups": 3,
            "projects": {
                live.clone(): { "mcpServers": { "zerograph": ours(&live) } },
                gone.clone(): { "allowedTools": ["x"], "mcpServers": { "zerograph": ours(&gone), "other": { "command": "x", "args": [] } } },
                "/gone/hand-made": { "mcpServers": { "zerograph": { "command": "zerograph", "args": ["serve"] } } },
            }
        });
        std::fs::write(&config, serde_json::to_string(&before).unwrap()).unwrap();
        let r = prune_mcp(&config, &[]);
        assert_eq!(r.written, 1, "{r:?}");
        assert!(r.notes.iter().any(|n| n.contains(&gone)), "{r:?}");
        let after = read_json(&config);
        assert_eq!(after["numStartups"], 3);
        assert_eq!(after["projects"][&live], before["projects"][&live], "a checkout on disk keeps its server");
        assert_eq!(after["projects"][&gone]["mcpServers"], serde_json::json!({ "other": { "command": "x", "args": [] } }));
        assert_eq!(after["projects"][&gone]["allowedTools"], serde_json::json!(["x"]));
        assert_eq!(after["projects"]["/gone/hand-made"], before["projects"]["/gone/hand-made"], "not one `register_mcp` made");
        let text = std::fs::read_to_string(&config).unwrap();
        assert_eq!(prune_mcp(&config, &[]).written, 0, "nothing left to prune");
        assert_eq!(std::fs::read_to_string(&config).unwrap(), text, "and nothing rewritten");
        assert_eq!(prune_mcp(&dir.path().join("missing.json"), &[]).written, 0);
    }

    #[cfg(unix)]
    #[test]
    fn the_pruned_config_keeps_its_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = root();
        let config = dir.path().join(".claude.json");
        std::fs::write(&config, r#"{"projects":{"/gone/wt":{"mcpServers":{"zerograph":{"command":"zerograph","args":["--repo","/gone/wt","mcp"]}}}}}"#).unwrap();
        std::fs::set_permissions(&config, std::fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(prune_mcp(&config, &[]).written, 1);
        assert_eq!(std::fs::metadata(&config).unwrap().permissions().mode() & 0o777, 0o600);
    }

    #[cfg(unix)]
    #[test]
    fn a_config_changed_while_pruning_is_left_for_the_next_run() {
        let dir = root();
        let config = dir.path().join(".claude.json");
        let stale = r#"{"projects":{"/gone/wt":{"mcpServers":{"zerograph":{"command":"zerograph","args":["--repo","/gone/wt","mcp"]}}}}}"#;
        std::fs::write(&config, stale).unwrap();
        let seen = std::fs::metadata(&config).unwrap();
        std::fs::write(&config, format!("{stale} ")).unwrap();
        assert!(replace_config(&config, &seen, "{}\n").is_err());
        assert_eq!(std::fs::read_to_string(&config).unwrap(), format!("{stale} "), "Claude Code's write is kept");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1, "no copy left beside it");
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_config_is_pruned_through_its_link() {
        let dir = root();
        let real = dir.path().join("dotfiles.json");
        std::fs::write(&real, r#"{"projects":{"/gone/wt":{"mcpServers":{"zerograph":{"command":"zerograph","args":["--repo","/gone/wt","mcp"]}}}}}"#).unwrap();
        let link = dir.path().join(".claude.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        assert_eq!(prune_mcp(&link, &[]).written, 1);
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(read_json(&real)["projects"]["/gone/wt"]["mcpServers"], serde_json::json!({}));
    }

    /// The block as the last release that wrote one spelled it.
    const OLD_BLOCK: &str = "# zerograph:begin\n# Brings zerograph's index in line with what this pull or checkout moved, in the background.\nif [ \"$3\" != \"0\" ]; then zerograph update --detach >/dev/null 2>&1 || true; fi\n# zerograph:end\n";

    #[test]
    fn the_block_an_earlier_release_wrote_comes_out_and_the_rest_of_the_hook_stays() {
        let mine = format!("#!/bin/sh\n{OLD_BLOCK}npx lefthook run post-merge\n");
        assert_eq!(without_git_block(&mine).as_deref(), Some("#!/bin/sh\nnpx lefthook run post-merge\n"));
        let twice = format!("#!/bin/sh\n{OLD_BLOCK}{OLD_BLOCK}echo hi\n");
        assert_eq!(without_git_block(&twice).as_deref(), Some("#!/bin/sh\necho hi\n"));
        assert_eq!(without_git_block(&format!("#!/bin/sh\n{OLD_BLOCK}\n")).as_deref(), Some(""), "only the shebang left: the file was ours");
        assert_eq!(without_git_block(OLD_BLOCK).as_deref(), Some(""));
        assert_eq!(without_git_block("#!/bin/sh\necho hi\n"), None, "a hook without the block is not touched");
    }

    #[test]
    fn a_re_run_removes_the_old_block_keeps_a_shared_hook_and_deletes_one_that_was_only_ours() {
        let dir = git_repo();
        let hooks = dir.path().join(".git/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("post-merge"), format!("#!/bin/sh\n{OLD_BLOCK}npx lefthook run post-merge\n")).unwrap();
        std::fs::write(hooks.join("post-checkout"), format!("#!/bin/sh\n{OLD_BLOCK}")).unwrap();
        let r = retire_git_hooks(dir.path());
        assert_eq!(r.written, 2, "{r:?}");
        assert_eq!(std::fs::read_to_string(hooks.join("post-merge")).unwrap(), "#!/bin/sh\nnpx lefthook run post-merge\n");
        assert!(!hooks.join("post-checkout").exists(), "{r:?}");
        let again = retire_git_hooks(dir.path());
        assert_eq!(again.written, 0, "{again:?}");
        assert!(again.notes.is_empty(), "{again:?}");
    }

    #[test]
    fn a_block_under_a_hooks_path_inside_the_git_dir_is_found_too() {
        let dir = git_repo();
        assert!(std::process::Command::new("git").arg("-C").arg(dir.path()).args(["config", "core.hooksPath", ".git/my-hooks"])
            .env_remove("GIT_DIR").status().unwrap().success());
        let hooks = dir.path().join(".git/my-hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("post-merge"), format!("#!/bin/sh\n{OLD_BLOCK}echo mine\n")).unwrap();
        assert_eq!(retire_git_hooks(dir.path()).written, 1);
        assert_eq!(std::fs::read_to_string(hooks.join("post-merge")).unwrap(), "#!/bin/sh\necho mine\n");
    }

    #[test]
    fn outside_a_git_repository_there_is_no_block_to_remove() {
        let r = retire_git_hooks(root().path());
        assert_eq!(r.written, 0);
        assert!(r.notes.is_empty(), "{r:?}");
    }

    /// Measured with prettier 3.9.6, under no config and under a consumer's: the one change it made
    /// to the 0.5.0 stanza was a blank line after the begin marker, because a comment is a block of
    /// its own. A stanza carrying that line is left alone by the formatter and by a re-run of this.
    #[test]
    fn the_stanza_is_what_prettier_writes_and_a_rerun_keeps_it() {
        let block = with_command(STANZA, "zerograph");
        assert!(block.starts_with("<!-- zerograph:begin -->\n\n## zerograph\n"), "{block}");

        let fresh = merged_instructions("# House rules\n", "zerograph");
        assert_eq!(fresh, format!("# House rules\n\n{block}"));
        assert_eq!(merged_instructions(&fresh, "zerograph"), fresh, "a re-run is a fixed point");

        // A block 0.5.0 wrote gains the line; the text after the end marker stays the repository's.
        let old = "# House rules\n\n<!-- zerograph:begin -->\n## zerograph\nold\n<!-- zerograph:end -->\n\n## After\n";
        let upgraded = merged_instructions(old, "zerograph");
        assert_eq!(upgraded, format!("# House rules\n\n{}\n\n## After\n", block.trim_end()));
        assert_eq!(merged_instructions(&upgraded, "zerograph"), upgraded);
    }

    fn exited(code: i32, stdout: &str) -> std::process::Output {
        #[cfg(unix)]
        use std::os::unix::process::ExitStatusExt;
        #[cfg(windows)]
        use std::os::windows::process::ExitStatusExt;
        #[cfg(unix)]
        let status = std::process::ExitStatus::from_raw(code << 8);
        #[cfg(windows)]
        let status = std::process::ExitStatus::from_raw(code as u32);
        std::process::Output { status, stdout: stdout.as_bytes().to_vec(), stderr: Vec::new() }
    }

    #[test]
    fn each_harness_installs_the_plugin_through_its_own_cli_marketplace_first() {
        for (target, wanted) in [
            (Target::Claude, vec!["claude plugin marketplace add devmaxxx/zerograph".to_string(), "claude plugin install zerograph@zerograph".to_string()]),
            (Target::Codex, vec![format!("codex plugin marketplace add devmaxxx/zerograph --ref v{VERSION}"), "codex plugin add zerograph@zerograph".to_string()]),
        ] {
            let calls = std::cell::RefCell::new(Vec::<String>::new());
            let run = |program: &str, _: &Path, args: &[String]| { calls.borrow_mut().push(format!("{program} {}", args.join(" "))); Some(exited(0, "")) };
            let (ok, r) = install_plugin(Path::new("."), target, &run);
            assert!(ok, "{r:?}");
            assert_eq!(calls.into_inner(), wanted);
        }
    }

    #[test]
    fn a_machine_without_the_cli_gets_the_commands_to_run_and_no_install() {
        let (ok, r) = install_plugin(Path::new("."), Target::Claude, &|_, _, _| None);
        assert!(!ok);
        assert!(r.notes[0].contains("`claude` is not on PATH"), "{:?}", r.notes);
        assert!(r.notes[0].contains("  claude plugin marketplace add devmaxxx/zerograph\n  claude plugin install zerograph@zerograph"), "{:?}", r.notes);
    }

    #[test]
    fn a_failed_step_stops_the_install_and_says_what_the_cli_said() {
        let calls = std::cell::RefCell::new(0);
        let run = |_: &str, _: &Path, _: &[String]| {
            *calls.borrow_mut() += 1;
            Some(std::process::Output { stderr: b"network down".to_vec(), ..exited(1, "") })
        };
        let (ok, r) = install_plugin(Path::new("."), Target::Codex, &run);
        assert!(!ok);
        assert_eq!(calls.into_inner(), 1, "the plugin is not added from a marketplace that failed");
        assert!(r.notes[0].contains("network down") && r.notes[0].contains("codex plugin add zerograph@zerograph"), "{:?}", r.notes);
    }

    /// A repository as `install-agent --claude` and `--codex` of 0.7.0's predecessors left it.
    fn older_install(root: &Path, command: &str) {
        let w = |rel: &str, text: &str| {
            let p = root.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, text).unwrap();
        };
        for dir in [".claude", ".codex"] {
            w(&format!("{dir}/hooks/zerograph-hook.mjs"), "#!/usr/bin/env node\n");
            w(&format!("{dir}/hooks/rule.txt"), &with_command(LEGACY_RULE, command));
            w(&format!("{dir}/skills/zerograph/SKILL.md"), &with_command(LEGACY_SKILL, command).replace("{{version}}", "0.6.4"));
        }
        w(".claude/agents/repo-scout.md", &with_command(LEGACY_SCOUT, command));
        let entry = |m: &str| serde_json::json!({ "matcher": m, "hooks": [{ "type": "command", "command": "node \"$CLAUDE_PROJECT_DIR/.claude/hooks/zerograph-hook.mjs\"", "timeout": 10 }] });
        let theirs = serde_json::json!({ "matcher": "Bash", "hooks": [{ "type": "command", "command": "./lint.sh" }] });
        w(".claude/settings.json", &serde_json::to_string_pretty(&serde_json::json!({
            "permissions": { "allow": ["Bash(ls)"] },
            "hooks": { "SessionStart": [entry("startup|resume|clear|compact")], "PreToolUse": [entry("Bash|Grep|Glob|Agent"), theirs] },
        })).unwrap());
        w("CLAUDE.md", &format!("# House rules\n\nBe kind.\n\n{}\n## After\n", with_command(STANZA, command)));
        w("AGENTS.md", &with_command(STANZA, command));
    }

    #[test]
    fn a_re_run_over_an_older_install_takes_out_what_it_wrote_and_keeps_the_rest() {
        let dir = root();
        older_install(dir.path(), "pnpm exec zerograph");
        let r = retire_repo_install(dir.path(), Target::Claude);
        assert!(!dir.path().join(".claude/hooks").exists(), "{r:?}");
        assert!(!dir.path().join(".claude/skills").exists(), "{r:?}");
        assert!(!dir.path().join(".claude/agents").exists(), "{r:?}");
        let settings = read_json(&dir.path().join(".claude/settings.json"));
        assert_eq!(settings["permissions"]["allow"], serde_json::json!(["Bash(ls)"]));
        assert_eq!(settings["hooks"], serde_json::json!({ "PreToolUse": [{ "matcher": "Bash", "hooks": [{ "type": "command", "command": "./lint.sh" }] }] }));
        assert_eq!(std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap(), "# House rules\n\nBe kind.\n\n## After\n");
        assert!(dir.path().join("AGENTS.md").exists(), "Codex's stanza is the Codex pass's");

        let r = retire_repo_install(dir.path(), Target::Codex);
        assert!(!dir.path().join(".codex").exists(), "{r:?}");
        assert!(!dir.path().join("AGENTS.md").exists(), "a file that held only the stanza goes");

        let again = retire_repo_install(dir.path(), Target::Claude);
        assert_eq!((again.written, again.notes.len()), (0, 0), "{again:?}");
    }

    #[test]
    fn settings_that_held_only_our_hooks_go_and_a_team_edited_skill_or_scout_stays() {
        let dir = root();
        older_install(dir.path(), "zerograph");
        let settings = dir.path().join(".claude/settings.json");
        std::fs::write(&settings, r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"node zerograph-hook.mjs"}]}]}}"#).unwrap();
        std::fs::write(dir.path().join(".claude/skills/zerograph/SKILL.md"), "---\nname: zerograph\n---\nOur own words.\n").unwrap();
        std::fs::write(dir.path().join(".claude/agents/repo-scout.md"), "a scout of our own\n").unwrap();
        let r = retire_repo_install(dir.path(), Target::Claude);
        assert!(!settings.exists(), "{r:?}");
        assert!(dir.path().join(".claude/skills/zerograph/SKILL.md").exists());
        assert!(dir.path().join(".claude/agents/repo-scout.md").exists());
        assert_eq!(r.notes.iter().filter(|n| n.contains("was left")).count(), 2, "{r:?}");
    }

    #[test]
    fn opencode_keeps_the_agents_md_stanza_it_still_reads() {
        let dir = root();
        older_install(dir.path(), "zerograph");
        install_editor(dir.path(), Editor::Opencode, "zerograph").unwrap();
        retire_repo_install(dir.path(), Target::Codex);
        assert!(std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap().contains(BEGIN));
    }

    #[cfg(unix)]
    #[test]
    fn a_linked_claude_md_is_not_written_through() {
        let dir = root();
        older_install(dir.path(), "zerograph");
        std::fs::remove_file(dir.path().join("CLAUDE.md")).unwrap();
        std::os::unix::fs::symlink("AGENTS.md", dir.path().join("CLAUDE.md")).unwrap();
        let r = retire_repo_install(dir.path(), Target::Claude);
        assert!(r.notes.iter().any(|n| n.contains("is a symlink")), "{r:?}");
        assert!(std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap().contains(BEGIN));
    }

    #[test]
    fn the_stanza_comes_out_whole_wherever_it_sits() {
        let block = with_command(STANZA, "zerograph");
        assert_eq!(without_stanza(&block).unwrap(), "");
        assert_eq!(without_stanza(&format!("# A\n\n{block}")).unwrap(), "# A\n");
        assert_eq!(without_stanza(&format!("{block}\n# B\n")).unwrap(), "# B\n");
        assert_eq!(without_stanza(&format!("# A\n\n{}\n\n# B\n", block.trim_end())).unwrap(), "# A\n\n# B\n");
        assert_eq!(without_stanza("# nothing of ours\n"), None);
    }

    #[test]
    fn the_codex_hooks_file_loses_only_the_entries_that_ran_our_script() {
        let dir = root();
        let path = dir.path().join("hooks.json");
        std::fs::write(&path, r#"{"hooks":{"SessionStart":[{"matcher":"startup","hooks":[{"type":"command","command":"node '/r/.codex/hooks/zerograph-hook.mjs'"}]},{"hooks":[{"type":"command","command":"gitnexus"}]}],"PostToolUse":[{"hooks":[{"type":"command","command":"node /r/.codex/hooks/zerograph-hook.mjs"}]}]}}"#).unwrap();
        let r = retire_codex_hooks(dir.path());
        assert_eq!(r.written, 1, "{r:?}");
        assert_eq!(read_json(&path), serde_json::json!({ "hooks": { "SessionStart": [{ "hooks": [{ "type": "command", "command": "gitnexus" }] }] } }));
        assert_eq!(retire_codex_hooks(dir.path()).written, 0);
        assert_eq!(retire_codex_hooks(&dir.path().join("none")).written, 0);
    }

    #[test]
    fn the_server_an_earlier_release_registered_for_a_live_checkout_goes_when_the_plugin_replaces_it() {
        let dir = root();
        let live = dir.path().to_str().unwrap().to_string();
        let other = dir.path().join("other");
        std::fs::create_dir(&other).unwrap();
        let other = other.to_str().unwrap().to_string();
        let ours = |p: &str| serde_json::json!({ "command": "zerograph", "args": ["--repo", p, "mcp"] });
        let config = dir.path().join(".claude.json");
        std::fs::write(&config, serde_json::to_string(&serde_json::json!({
            "projects": { live.clone(): { "mcpServers": { "zerograph": ours(&live) } }, other.clone(): { "mcpServers": { "zerograph": ours(&other) } } }
        })).unwrap()).unwrap();
        let r = prune_mcp(&config, &[PathBuf::from(&live)]);
        assert!(r.notes.iter().any(|n| n.contains("the plugin's server replaces it")), "{r:?}");
        let after = read_json(&config);
        assert_eq!(after["projects"][&live]["mcpServers"], serde_json::json!({}));
        assert_eq!(after["projects"][&other]["mcpServers"]["zerograph"], ours(&other), "another repository's is not this install's");
    }
}
