//! `zerograph install-agent`: writes the agent-facing surface into a consuming repository, for
//! Claude Code or for Codex, and an MCP entry and the stanza for Cursor, VS Code with Copilot,
//! Gemini CLI and opencode. The texts are embedded with `include_str!` rather than copied from a
//! checkout, so a binary and the surface it installs cannot disagree about what the tool can do.
//!
//! Idempotent by construction: a second run rewrites the same bytes and reports nothing written.
//! The settings file is merged rather than replaced — a repository's own hooks survive, and this
//! tool's entries are replaced by command match, which is what makes an upgrade a re-run.
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

const HOOK: &str = include_str!("../agent/hook.mjs");
const RULE: &str = include_str!("../agent/rule.txt");
const STANZA: &str = include_str!("../agent/stanza.md");
const SKILL: &str = include_str!("../agent/skill/SKILL.md");
/// The skill as 0.6.0–0.6.3 wrote it, under the name `repo-query`: what an upgrade may remove.
const LEGACY_SKILL: &str = include_str!("../agent/skill/repo-query.md");
const VERSION: &str = env!("CARGO_PKG_VERSION");
const SCOUT: &str = include_str!("../agent/subagent.md");

/// Where the surface is being installed. The two harnesses read different files for the same four
/// jobs, and the only thing that differs between them is which path each job goes to.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target { Claude, Codex }

impl Target {
    fn dir(self) -> &'static str {
        match self { Target::Claude => ".claude", Target::Codex => ".codex" }
    }

    /// The file each harness reads for its always-on instructions. Codex reads the repository's
    /// **root** `AGENTS.md` and has no `.codex/AGENTS.md` in its spec, so its stanza is written at
    /// the root whether or not one is already there; Claude Code reads either, and a repository
    /// that already keeps a root `CLAUDE.md` should not grow a second file beside it.
    fn instructions(self) -> &'static str {
        match self { Target::Claude => "CLAUDE.md", Target::Codex => "AGENTS.md" }
    }

    /// Whether the harness reads a settings file inside the repository. Claude Code does
    /// (`.claude/settings.json`); Codex's hooks are `CODEX_HOME`-relative — `"hooks":
    /// "./hooks.json"` — so an entry added from inside one repository would fire in every other
    /// repository on the machine. `agent/codex.md` records how both were read off the CLI.
    fn hooks_are_repository_scoped(self) -> bool { self == Target::Claude }
}

#[derive(Debug, Default)]
pub struct Report { pub written: usize, pub paths: Vec<String>, pub notes: Vec<String> }

/// A text with `{{command}}` resolved to how this repository invokes the binary — `zerograph`
/// unless a caller says otherwise, `pnpm exec zerograph` in a workspace that installs it as a
/// dependency. Every text that names a command carries it: a line naming the bare binary where
/// PATH has none tells an agent to run something that is not there.
fn with_command(template: &str, command: &str) -> String { template.replace("{{command}}", command) }

/// The hook's one declaration of the command its hints name. Replaced whole rather than through
/// `{{command}}`, because this text lands in JavaScript: a JSON string is a JavaScript string
/// literal, so no quote, backslash or `${` in a command can end the literal or open an expression.
/// The template declares the default, which is what lets the hook's own tests run it as it ships.
const HOOK_COMMAND: &str = "const COMMAND = \"zerograph\";";

fn hook(command: &str) -> String {
    let script = HOOK.replacen(HOOK_COMMAND, &format!("const COMMAND = {};", serde_json::Value::from(command)), 1);
    let (shebang, rest) = script.split_once('\n').unwrap();
    format!("{shebang}\n{}{rest}", stamp())
}

/// The line under the shebang records the matchers this release wrote, so a re-run can tell one it
/// wrote (moved to the new default) from one a person changed (kept).
fn stamp() -> String {
    let wrote: Vec<String> = hook_entries("").iter().map(|(e, m, _)| format!("{e}={m}")).collect();
    format!("// zerograph {} wrote: {}\n", env!("CARGO_PKG_VERSION"), wrote.join(" "))
}

const BEGIN: &str = "<!-- zerograph:begin -->";
const END: &str = "<!-- zerograph:end -->";
/// The markers an install made before the rename wrote: its stanza is ours to replace, so an
/// upgrade leaves one stanza and not two naming different binaries.
const LEGACY_BEGIN: &str = "<!-- repograph:begin -->";
const LEGACY_END: &str = "<!-- repograph:end -->";

/// Where a marked block starts, and where its end marker ends.
fn marked(text: &str, begin: &str, end: &str) -> Option<(usize, usize)> {
    let a = text.find(begin)?;
    let b = a + text[a..].find(end)?;
    Some((a, b + end.len()))
}

fn skip_break(text: &str) -> &str {
    text.strip_prefix("\r\n").or_else(|| text.strip_prefix('\n')).unwrap_or(text)
}

/// The instructions file with our stanza in it: replacing what is between the markers when they are
/// there, appending when they are not. Everything outside the markers is the repository's own and
/// is never touched — an installer that rewrote CLAUDE.md would be a worse citizen than no
/// installer at all.
pub fn merged_instructions(existing: &str, command: &str) -> String {
    let block = with_command(STANZA, command);
    let ours = marked(existing, BEGIN, END);
    let legacy = marked(existing, LEGACY_BEGIN, LEGACY_END);
    match (ours, legacy) {
        // Both: the old one goes with the line it ended, and with one blank line when the text
        // before it ends in one, or the gap it leaves would be two. The new one is replaced as usual.
        (Some(_), Some((a, b))) => {
            let before = &existing[..a];
            let after = skip_break(&existing[b..]);
            let blank_before = before.is_empty() || before.ends_with("\n\n") || before.ends_with("\n\r\n");
            let after = if blank_before { skip_break(after) } else { after };
            merged_instructions(&format!("{before}{after}"), command)
        }
        (Some((a, b)), None) | (None, Some((a, b))) => {
            let mut out = String::with_capacity(existing.len() + block.len());
            out.push_str(&existing[..a]);
            out.push_str(block.trim_end());
            out.push_str(&existing[b..]);
            // Replacing the first old stanza leaves a second one beside ours, and the arm above
            // removes it; stopping here would leave the file to change again on the next run.
            if marked(&out, LEGACY_BEGIN, LEGACY_END).is_some() { merged_instructions(&out, command) } else { out }
        }
        (None, None) => {
            let mut out = existing.to_string();
            if !out.is_empty() && !out.ends_with('\n') { out.push('\n'); }
            if !out.is_empty() { out.push('\n'); }
            out.push_str(&block);
            out
        }
    }
}

/// The four hook entries this tool owns, as the harness's settings file spells them.
fn hook_entries(hook_path: &str) -> Vec<(&'static str, &'static str, serde_json::Value)> {
    let entry = |timeout: u64| serde_json::json!({
        "hooks": [{ "type": "command", "command": format!("node \"{hook_path}\""), "timeout": timeout }]
    });
    vec![
        ("SessionStart", "startup|resume|clear|compact", entry(10)),
        ("SubagentStart", ".*", entry(5)),
        ("PreToolUse", "Bash|Grep|Glob|Agent", entry(10)),
        ("PostToolUse", "Edit|Write|MultiEdit", entry(10)),
    ]
}

/// The settings file with this tool's entries in it and everything else left alone. An entry whose
/// command names our hook script is replaced rather than added, so a re-run after the hook changed
/// is an upgrade and not a second copy firing twice per event.
///
/// Returns the file and the events an entry running the *retired* `repograph-notice` hook was
/// dropped from. That script is the previous surface and this one replaces it, but it is a file
/// the repository owns and a person put in that list: removing it without a word would be the one
/// silent edit in an installer that otherwise touches nothing it did not write.
///
/// `prior_hook` is the hook script on disk before this run; its stamp says which matchers we wrote.
pub fn merged_settings(existing: &str, hook_path: &str, prior_hook: &str) -> Result<(String, Vec<String>)> {
    let mut root: serde_json::Value = match existing.trim().is_empty() {
        true => serde_json::json!({}),
        false => serde_json::from_str(existing).context("the settings file is not JSON")?,
    };
    if !root.is_object() { anyhow::bail!("the settings file is not a JSON object"); }
    let hooks = root.as_object_mut().unwrap().entry("hooks").or_insert_with(|| serde_json::json!({}));
    if !hooks.is_object() { anyhow::bail!("`hooks` in the settings file is not an object"); }
    let mut retired = Vec::new();
    // A hook from before the stamp has none, and its release wrote today's defaults.
    let stamp = prior_hook.lines().nth(1).unwrap_or_default();
    for (event, matcher, mut ours) in hook_entries(hook_path) {
        let list = hooks.as_object_mut().unwrap().entry(event).or_insert_with(|| serde_json::json!([]));
        let Some(arr) = list.as_array_mut() else { anyhow::bail!("`hooks.{event}` is not an array") };
        if arr.iter().any(|e| e.to_string().contains("repograph-notice")) { retired.push(event.to_string()); }
        let theirs = arr.iter()
            .filter(|e| ["zerograph-hook", "repograph-hook"].iter().any(|s| e.to_string().contains(s)))
            .find_map(|e| e["matcher"].as_str())
            .filter(|m| *m != matcher && !stamp.split(' ').any(|t| t == format!("{event}={m}")));
        ours["matcher"] = serde_json::json!(theirs.unwrap_or(matcher));
        // Ours by the script it runs, never by position: a repository's own entries keep theirs.
        // `repograph-hook` is this hook under the name it had before the rename.
        arr.retain(|e| !["zerograph-hook", "repograph-hook", "repograph-notice"].iter().any(|s| e.to_string().contains(s)));
        arr.push(ours);
    }
    Ok((serde_json::to_string_pretty(&root)? + "\n", retired))
}

fn is_symlink(path: &Path) -> bool {
    std::fs::symlink_metadata(path).is_ok_and(|m| m.file_type().is_symlink())
}

/// A repository can ship `.claude/settings.json` or `zerograph.toml` as a link to a file elsewhere
/// on the machine, and a write through it would overwrite that file with ours.
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

/// The block a reader pastes into `~/.codex/hooks.json` to wire the hook on a machine, printed
/// rather than written: that file is machine-level, and a `--repo` command does not get to change
/// how every other repository on the machine behaves. Three events, not four — the Bash
/// interceptor's verdict is being read on Claude Code and is not carried over blind.
pub fn codex_hooks_block(hook_abs: &str) -> String {
    let entry = |matcher: &str, timeout: u64| serde_json::json!({
        "matcher": matcher,
        "hooks": [{ "type": "command", "command": format!("node '{hook_abs}'"), "timeout": timeout }]
    });
    serde_json::to_string_pretty(&serde_json::json!({
        "hooks": {
            "SessionStart": [entry("startup|resume|clear|compact", 10)],
            "SubagentStart": [entry(".*", 5)],
            "PostToolUse": [entry("apply_patch|Edit|Write", 10)],
        }
    })).unwrap_or_default()
}

/// Removes the script an install made before the rename, which nothing runs once the settings
/// name the new one. Only a regular file that still carries the hook's debug tag, which every
/// release of it printed, is ours to remove: a link, or a file someone rewrote into something
/// else, is left and named. So is one that `local_settings`, a file this install does not merge,
/// still runs, and one the filesystem will not let go of: the settings already name the new
/// script by now, and an error here would take the rest of the install's report with it.
fn retire_legacy_hook(path: &Path, local_settings: &Path, report: &mut Report) {
    let Ok(meta) = std::fs::symlink_metadata(path) else { return };
    if std::fs::read_to_string(local_settings).is_ok_and(|t| t.contains("repograph-hook")) {
        report.notes.push(format!("{} is still run from {}, which is not edited here, so it was left; point that entry at zerograph-hook.mjs, then remove it",
            path.display(), local_settings.display()));
        return;
    }
    let ours = meta.is_file() && std::fs::read_to_string(path).is_ok_and(|t| t.contains("[repograph-hook]"));
    if !ours {
        report.notes.push(format!("{} is not the hook this tool wrote, so it was left; nothing runs it now", path.display()));
        return;
    }
    if let Err(e) = std::fs::remove_file(path) {
        report.notes.push(format!("{} could not be removed ({e}); nothing runs it now, so remove it by hand", path.display()));
        return;
    }
    report.written += 1;
    report.notes.push(format!("removed {}, the hook from before the rename; zerograph-hook.mjs replaces it", path.display()));
}

/// The skill under `skills/zerograph/`, stamped with the release that wrote it — the
/// `metadata.version` the Agent Skills format keeps for that — so a re-run by the same release
/// writes nothing and one by another says what it replaced. The `repo-query` skill an earlier
/// release wrote goes with it, when it is still the text that release wrote.
fn write_skill(skills: &Path, command: &str, report: &mut Report) -> Result<()> {
    let path = skills.join("zerograph").join("SKILL.md");
    let before = std::fs::read_to_string(&path).ok().and_then(|t| skill_version(&t).map(String::from));
    let written = report.written;
    write_if_changed(&path, &skill(command), report)?;
    if let Some(old) = before.filter(|v| v != VERSION && report.written > written) {
        report.notes.push(format!("{}: rewritten from zerograph {old} to {VERSION}", path.display()));
    }
    retire_legacy_skill(&skills.join("repo-query"), report);
    Ok(())
}

fn skill(command: &str) -> String { with_command(SKILL, command).replace("{{version}}", VERSION) }

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

/// Removes the `repo-query` skill an earlier release wrote, now that `zerograph` replaces it. Only
/// a directory holding nothing but the `SKILL.md` that release generated is ours: an edited one, or
/// one with files of its own, is a team's skill under that name, and is left and named.
fn retire_legacy_skill(dir: &Path, report: &mut Report) {
    let Ok(meta) = std::fs::symlink_metadata(dir) else { return };
    let file = dir.join("SKILL.md");
    let only_skill = meta.is_dir() && std::fs::read_dir(dir).is_ok_and(|entries| {
        entries.filter_map(|e| e.ok()).all(|e| e.file_name() == "SKILL.md" && e.file_type().is_ok_and(|t| t.is_file()))
    });
    let ours = only_skill && std::fs::read_to_string(&file).is_ok_and(|t| generated_from(LEGACY_SKILL, &t));
    if !ours {
        report.notes.push(format!("{} is not the skill an earlier zerograph wrote, so it was left; the `zerograph` skill beside it replaces it", dir.display()));
        return;
    }
    if let Err(e) = std::fs::remove_file(&file).and_then(|_| std::fs::remove_dir(dir)) {
        report.notes.push(format!("{} could not be removed ({e}); the `zerograph` skill replaces it, so remove it by hand", dir.display()));
        return;
    }
    report.written += 1;
    report.notes.push(format!("removed {}, the skill's name before 0.6.4; `zerograph` replaces it", dir.display()));
}

/// Installs the surface for one harness under `root`. Returns what it actually wrote: a second run
/// writes nothing, which is how a caller can tell an upgrade from a no-op.
pub fn install(root: &Path, target: Target, command: &str) -> Result<Report> {
    let mut report = Report::default();
    let base: PathBuf = root.join(target.dir());
    let hook_file = base.join("hooks").join("zerograph-hook.mjs");
    let prior_hook = std::fs::read_to_string(&hook_file).unwrap_or_default();
    write_if_changed(&hook_file, &hook(command), &mut report)?;
    write_if_changed(&base.join("hooks").join("rule.txt"), &with_command(RULE, command), &mut report)?;
    write_skill(&base.join("skills"), command, &mut report)?;
    if target == Target::Claude {
        write_if_changed(&base.join("agents").join("repo-scout.md"), &with_command(SCOUT, command), &mut report)?;
    }

    // A path the harness can resolve from wherever it runs the hook. Claude Code exports the
    // project directory; Codex runs with the repository as its working directory and exports
    // nothing, so the path is relative there.
    let hook_path = match target {
        Target::Claude => "$CLAUDE_PROJECT_DIR/.claude/hooks/zerograph-hook.mjs".to_string(),
        Target::Codex => ".codex/hooks/zerograph-hook.mjs".to_string(),
    };
    if target.hooks_are_repository_scoped() {
        let settings = base.join("settings.json");
        let existing = std::fs::read_to_string(&settings).unwrap_or_default();
        let (merged, retired) = merged_settings(&existing, &hook_path, &prior_hook)?;
        if !retired.is_empty() {
            report.notes.push(format!(
                "removed the retired `repograph-notice` hook from {} — this hook replaces it; \
                 the script itself is still on disk", retired.join(", ")));
        }
        write_if_changed(&settings, &merged, &mut report)?;
    }
    let legacy_hook = base.join("hooks").join("repograph-hook.mjs");
    match target.hooks_are_repository_scoped() {
        // Only once the settings name the new script, so a failed merge leaves a working install.
        true => retire_legacy_hook(&legacy_hook, &base.join("settings.local.json"), &mut report),
        // The machine's hooks file may still run it, and an install from here cannot see that file.
        false if std::fs::symlink_metadata(&legacy_hook).is_ok() => report.notes.push(format!(
            "{} is the hook from before the rename; point ~/.codex/hooks.json at zerograph-hook.mjs, then remove it",
            legacy_hook.display())),
        false => {}
    }

    // Codex's is the repository root or nowhere; Claude Code reads either, and a repository that
    // already keeps a root `CLAUDE.md` should not grow a second file beside it. A stanza already in
    // `.claude/CLAUDE.md` stays the one to update, though, or a root file made since would hold a
    // second stanza beside the first.
    let root_instructions = root.join(target.instructions());
    let inner_instructions = base.join(target.instructions());
    let instructions = match target == Target::Codex || holds_stanza(&root_instructions)
        || (root_instructions.exists() && !holds_stanza(&inner_instructions))
    {
        true => root_instructions,
        false => inner_instructions,
    };
    let existing = std::fs::read_to_string(&instructions).unwrap_or_default();
    let merged = merged_instructions(&existing, command);
    // Never written through a link, and not an error either: `CLAUDE.md` linked to `AGENTS.md` is
    // common, and failing here would abort an install that has already rewritten the settings and
    // removed the old hook, before the MCP server and the git hooks are reached.
    match merged != existing && is_symlink(&instructions) {
        true => report.notes.push(format!(
            "{} is a symlink, so the stanza was not written through it; `install-agent --codex` writes it \
             when it points at AGENTS.md, and otherwise the file it points to needs the update by hand",
            instructions.display())),
        false => write_if_changed(&instructions, &merged, &mut report)?,
    }
    Ok(report)
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

fn holds_stanza(path: &Path) -> bool {
    std::fs::read_to_string(path).is_ok_and(|t| t.contains(BEGIN) || t.contains(LEGACY_BEGIN))
}

const MCP_NAME: &str = "zerograph";
const LEGACY_MCP_NAME: &str = "repograph";

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

/// Registers `zerograph --repo <root> mcp` with Claude Code for the project at `root`, through
/// `claude mcp add` so the harness keeps its own registry's format. `--repo` is explicit because
/// the server's working directory is the harness's choice. `run` is how `claude` is invoked: a
/// test passes its own, and `None` from it means the binary is not on PATH. A registration that
/// already says this is left alone, which is what makes a re-run write nothing; one that says
/// something else (another command, another repository) is removed and made again. A missing
/// `claude` is a note and not a failure, since the files this install wrote are useful without it.
pub fn register_mcp(root: &Path, command: &str, run: &dyn Fn(&Path, &[&str]) -> Option<std::process::Output>) -> Report {
    let mut report = Report::default();
    let words = split_words(command);
    let Some((program, rest)) = words.split_first() else {
        report.notes.push("--command is empty, so the MCP server was not registered".to_string());
        return report;
    };
    let root_arg = root.display().to_string();
    let tail: Vec<&str> = rest.iter().map(String::as_str).chain(["--repo", &root_arg, "mcp"]).collect();
    let by_hand = format!("claude mcp add {MCP_NAME} -- {}", words.iter().map(String::as_str).chain(["--repo", &root_arg, "mcp"]).map(shell_word).collect::<Vec<_>>().join(" "));
    let Some(existing) = run(root, &["mcp", "get", MCP_NAME]) else {
        report.notes.push(format!("`claude` is not on PATH, so the MCP server was not registered; run `{by_hand}` where it is"));
        return report;
    };
    if existing.status.success() {
        let shown = String::from_utf8_lossy(&existing.stdout);
        if said_in_order(&shown, program.as_str(), &tail) {
            report.notes.push("MCP server already registered, left alone".to_string());
            retire_legacy_mcp(root, run, &mut report);
            return report;
        }
        if !run(root, &["mcp", "remove", MCP_NAME]).is_some_and(|o| o.status.success()) {
            report.notes.push(format!("the registered MCP server differs and `claude mcp remove {MCP_NAME}` failed; run it, then `{by_hand}`"));
            return report;
        }
    }
    let mut args = vec!["mcp", "add", MCP_NAME, "--", program.as_str()];
    args.extend(&tail);
    match run(root, &args) {
        Some(added) if added.status.success() => {
            report.written = 1;
            report.paths.push(by_hand);
            retire_legacy_mcp(root, run, &mut report);
        }
        Some(failed) => report.notes.push(format!("`claude mcp add` failed, the MCP server was not registered: {}", String::from_utf8_lossy(&failed.stderr).trim())),
        None => report.notes.push("`claude` went away between two calls; the MCP server was not registered".to_string()),
    }
    report
}

/// The server an install made before the rename registered would answer beside `zerograph`, every
/// tool twice. It goes only once `zerograph` is registered, so a failed `add` leaves the old one
/// answering and not nothing, and only from the local scope, where `claude mcp add` put it:
/// without a scope the CLI removes whichever single scope holds the name, a user-level or a
/// tracked `.mcp.json` entry included, and refuses when several do.
fn retire_legacy_mcp(root: &Path, run: &dyn Fn(&Path, &[&str]) -> Option<std::process::Output>, report: &mut Report) {
    if !run(root, &["mcp", "get", LEGACY_MCP_NAME]).is_some_and(|o| o.status.success()) { return; }
    match run(root, &["mcp", "remove", LEGACY_MCP_NAME, "-s", "local"]) {
        Some(o) if o.status.success() => report.notes.push(format!("removed the MCP server registered as `{LEGACY_MCP_NAME}`; `{MCP_NAME}` replaces it")),
        _ => report.notes.push(format!(
            "an MCP server is still registered as `{LEGACY_MCP_NAME}` and was not removed; `claude mcp get {LEGACY_MCP_NAME}` shows its scope, \
             and `claude mcp remove {LEGACY_MCP_NAME} -s <scope>` removes it")),
    }
}

/// `word` as one shell word: bare when it has nothing the shell would split or expand, and
/// single-quoted otherwise, so a path with a space still pastes as the one argument it is.
fn shell_word(word: &str) -> String {
    let plain = !word.is_empty() && word.chars().all(|c| c.is_alphanumeric() || "-_./:=@%+,".contains(c));
    if plain { word.to_string() } else { format!("'{}'", word.replace('\'', "'\\''")) }
}

/// Whether `shown`, what `claude mcp get` printed, names `program` and then every argument of
/// `tail` in order. Each is looked for on its own because the harness decides how it joins them,
/// and a path with a space in it does not survive being compared as one space-joined string.
fn said_in_order(shown: &str, program: &str, tail: &[&str]) -> bool {
    // A word counts only where whitespace or an end bounds it, or `/proj/app` is read inside `/proj/app2`.
    let mut rest = shown;
    std::iter::once(program).chain(tail.iter().copied()).all(|word| {
        let whole = |at: usize| rest[..at].chars().next_back().is_none_or(char::is_whitespace)
            && rest[at + word.len()..].chars().next().is_none_or(char::is_whitespace);
        match rest.match_indices(word).map(|(at, _)| at).find(|&at| whole(at)) {
            Some(at) => { rest = &rest[at + word.len()..]; true }
            None => false,
        }
    })
}

/// `claude` run in `root`, where its local-scope registration belongs; `None` when it is not
/// installed.
pub fn run_claude(root: &Path, args: &[&str]) -> Option<std::process::Output> {
    std::process::Command::new("claude").args(args).current_dir(root).stdin(std::process::Stdio::null()).output().ok()
}

/// The git events an earlier release wrote a refresh block into.
const GIT_HOOKS: [&str; 2] = ["post-merge", "post-checkout"];
/// The markers of that block, under both names the tool has had.
const GIT_MARKERS: [(&str, &str); 2] = [("# zerograph:begin", "# zerograph:end"), ("# repograph:begin", "# repograph:end")];

/// `existing` without the refresh block an earlier release wrote, or `None` when it holds none. A
/// hook left with nothing but its shebang comes back empty: the block was all that file was for.
pub fn without_git_block(existing: &str) -> Option<String> {
    let mut text = existing.to_string();
    let mut found = false;
    while let Some((a, b)) = GIT_MARKERS.iter().find_map(|(begin, end)| marked(&text, begin, end)) {
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
    let git_path = |args: &[&str]| std::process::Command::new("git").arg("-C").arg(root).args(args)
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

    #[cfg(unix)]
    #[test]
    fn a_symlinked_target_is_refused_and_what_it_points_at_is_left_alone() {
        let dir = root();
        let victim = dir.path().join("elsewhere.json");
        std::fs::write(&victim, "{}").unwrap();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::os::unix::fs::symlink(&victim, dir.path().join(".claude/settings.json")).unwrap();
        let err = install(dir.path(), Target::Claude, "zerograph").unwrap_err().to_string();
        assert!(err.contains("symlink"), "{err}");
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "{}");
    }

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

    #[cfg(unix)]
    fn exited(code: i32, stdout: &str) -> std::process::Output {
        use std::os::unix::process::ExitStatusExt;
        std::process::Output { status: std::process::ExitStatus::from_raw(code << 8), stdout: stdout.as_bytes().to_vec(), stderr: Vec::new() }
    }

    #[test]
    fn a_command_is_split_the_way_a_shell_would() {
        assert_eq!(split_words("zerograph"), ["zerograph"]);
        assert_eq!(split_words("pnpm exec  zerograph"), ["pnpm", "exec", "zerograph"]);
        assert_eq!(split_words(r#"pnpm exec "my repo graph" 'a b' c\ d"#), ["pnpm", "exec", "my repo graph", "a b", "c d"]);
        assert_eq!(split_words(r#"x "" y"#), ["x", "", "y"]);
        assert!(split_words("   ").is_empty());
    }

    /// A `claude` whose registry holds one entry per name, shown the way `mcp get` prints it.
    #[cfg(unix)]
    fn fake_claude(calls: &std::cell::RefCell<Vec<Vec<String>>>, registry: &std::cell::RefCell<std::collections::BTreeMap<String, String>>, args: &[&str]) -> Option<std::process::Output> {
        calls.borrow_mut().push(args.iter().map(|a| a.to_string()).collect());
        Some(match args {
            ["mcp", "get", name] => match registry.borrow().get(*name) { Some(shown) => exited(0, shown), None => exited(1, "") },
            ["mcp", "remove", name, ..] => { registry.borrow_mut().remove(*name); exited(0, "") }
            ["mcp", "add", name, "--", program, rest @ ..] => {
                registry.borrow_mut().insert(name.to_string(), format!("{name}:\n  Command: {program}\n  Args: {}\n", rest.join(" ")));
                exited(0, "")
            }
            _ => exited(1, ""),
        })
    }

    #[cfg(unix)]
    #[test]
    fn the_mcp_server_is_registered_once_a_second_run_leaves_it_and_a_different_one_is_replaced() {
        let calls = std::cell::RefCell::new(Vec::<Vec<String>>::new());
        let registry = std::cell::RefCell::new(std::collections::BTreeMap::new());
        let claude = |_: &Path, args: &[&str]| fake_claude(&calls, &registry, args);
        let root = Path::new("/work/proj");
        let first = register_mcp(root, "pnpm exec zerograph", &claude);
        assert_eq!(first.written, 1);
        assert!(calls.borrow().iter().any(|c| c == &["mcp", "add", "zerograph", "--", "pnpm", "exec", "zerograph", "--repo", "/work/proj", "mcp"]), "{:?}", calls.borrow());
        let asked = calls.borrow().len();
        let second = register_mcp(root, "pnpm exec zerograph", &claude);
        assert_eq!(second.written, 0);
        assert!(calls.borrow()[asked..].iter().all(|c| c[1] == "get"), "the second run only asked: {:?}", calls.borrow());
        assert!(second.notes[0].contains("already registered"));
        let moved = register_mcp(Path::new("/work/other"), "pnpm exec zerograph", &claude);
        assert_eq!(moved.written, 1, "another repository replaces the entry");
        assert!(calls.borrow().iter().any(|c| c[..2] == ["mcp", "remove"]));
        assert!(registry.borrow()["zerograph"].contains("/work/other"));
    }

    #[cfg(unix)]
    #[test]
    fn the_server_registered_before_the_rename_is_removed_so_it_does_not_answer_twice() {
        let calls = std::cell::RefCell::new(Vec::<Vec<String>>::new());
        let registry = std::cell::RefCell::new(std::collections::BTreeMap::new());
        registry.borrow_mut().insert("repograph".to_string(), "repograph:\n  Command: repograph\n  Args: --repo /work/proj mcp\n".to_string());
        let claude = |_: &Path, args: &[&str]| fake_claude(&calls, &registry, args);
        let r = register_mcp(Path::new("/work/proj"), "zerograph", &claude);
        assert_eq!(r.written, 1);
        assert_eq!(registry.borrow().keys().collect::<Vec<_>>(), ["zerograph"]);
        assert!(r.notes.iter().any(|n| n.contains("`repograph`")), "{:?}", r.notes);
        let removals: Vec<_> = calls.borrow().iter().filter(|c| c[1] == "remove").cloned().collect();
        assert_eq!(removals, [["mcp", "remove", "repograph", "-s", "local"]], "only the local scope, where the old install put it");
        let again = register_mcp(Path::new("/work/proj"), "zerograph", &claude);
        assert_eq!(again.written, 0);
        assert!(!again.notes.iter().any(|n| n.contains("`repograph`")), "nothing left to remove: {:?}", again.notes);
    }

    #[cfg(unix)]
    #[test]
    fn the_server_registered_before_the_rename_stays_when_the_new_one_cannot_be_added() {
        let calls = std::cell::RefCell::new(Vec::<Vec<String>>::new());
        let registry = std::cell::RefCell::new(std::collections::BTreeMap::new());
        registry.borrow_mut().insert("repograph".to_string(), "repograph:\n  Command: repograph\n  Args: --repo /work/proj mcp\n".to_string());
        let claude = |_: &Path, args: &[&str]| match args {
            ["mcp", "add", ..] => Some(exited(1, "")),
            _ => fake_claude(&calls, &registry, args),
        };
        let r = register_mcp(Path::new("/work/proj"), "zerograph", &claude);
        assert_eq!(r.written, 0);
        assert!(registry.borrow().contains_key("repograph"), "the old server is the only one there is");
        assert!(calls.borrow().iter().all(|c| c[1] != "remove"), "{:?}", calls.borrow());
        assert!(!r.notes.iter().any(|n| n.contains("replaces it")), "{:?}", r.notes);
    }

    #[test]
    fn a_stanza_from_before_the_rename_is_replaced_not_doubled() {
        let legacy = "# House rules\n\n<!-- repograph:begin -->\nrun `repograph ask`\n<!-- repograph:end -->\n\nMore rules.\n";
        let merged = merged_instructions(legacy, "zerograph");
        assert!(!merged.contains("repograph"), "{merged}");
        assert_eq!(merged.matches(BEGIN).count(), 1, "{merged}");
        assert!(merged.starts_with("# House rules\n\n") && merged.ends_with("\n\nMore rules.\n"), "{merged}");
        assert_eq!(merged_instructions(&merged, "zerograph"), merged, "re-merging is a no-op");
        let block = with_command(STANZA, "zerograph");
        let both = format!("{legacy}\n{block}");
        let merged = merged_instructions(&both, "zerograph");
        assert_eq!(merged, format!("# House rules\n\nMore rules.\n\n{block}"), "the old stanza goes with one of its two blank lines");
        assert_eq!(merged_instructions(&merged, "zerograph"), merged);
        let crlf = merged_instructions(&both.replace('\n', "\r\n"), "zerograph");
        assert!(crlf.starts_with("# House rules\r\n\r\nMore rules.\r\n\r\n") && !crlf.contains("repograph"), "{crlf:?}");

        let twice = "# A\n\n<!-- repograph:begin -->\nold\n<!-- repograph:end -->\n\nmid\n\n<!-- repograph:begin -->\nolder\n<!-- repograph:end -->\n";
        let merged = merged_instructions(twice, "zerograph");
        assert!(!merged.contains("repograph") && merged.matches(BEGIN).count() == 1, "{merged}");
        assert_eq!(merged_instructions(&merged, "zerograph"), merged, "one run reaches the fixed point");
    }

    #[test]
    fn an_install_from_before_the_rename_is_upgraded_in_place() {
        let dir = root();
        let hooks = dir.path().join(".claude/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("repograph-hook.mjs"), "function debug() { console.error('[repograph-hook]'); }\n").unwrap();
        std::fs::write(dir.path().join(".claude/settings.json"), r#"{"hooks":{"SessionStart":[
            {"matcher":"startup","hooks":[{"type":"command","command":"node \"$CLAUDE_PROJECT_DIR/.claude/hooks/repograph-hook.mjs\""}]}
        ]}}"#).unwrap();
        let r = install(dir.path(), Target::Claude, "zerograph").unwrap();
        assert!(!hooks.join("repograph-hook.mjs").exists(), "{r:?}");
        assert!(r.notes.iter().any(|n| n.contains("repograph-hook.mjs")), "{:?}", r.notes);
        let settings = std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap();
        assert!(!settings.contains("repograph-hook"), "{settings}");
        let v: serde_json::Value = serde_json::from_str(&settings).unwrap();
        assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 1);

        std::fs::write(hooks.join("repograph-hook.mjs"), "// the team's own script\n").unwrap();
        let r = install(dir.path(), Target::Claude, "zerograph").unwrap();
        assert!(hooks.join("repograph-hook.mjs").exists());
        assert!(r.notes.iter().any(|n| n.contains("not the hook this tool wrote")), "{:?}", r.notes);

        // Codex's machine hooks file may still run the old script, so it stays until a person moves it.
        let codex = dir.path().join(".codex/hooks");
        std::fs::create_dir_all(&codex).unwrap();
        std::fs::write(codex.join("repograph-hook.mjs"), "console.error('[repograph-hook]');\n").unwrap();
        let r = install(dir.path(), Target::Codex, "zerograph").unwrap();
        assert!(codex.join("repograph-hook.mjs").exists());
        assert!(r.notes.iter().any(|n| n.contains("~/.codex/hooks.json")), "{:?}", r.notes);
    }

    #[test]
    fn the_old_hook_stays_while_the_local_settings_still_run_it() {
        let dir = root();
        let hooks = dir.path().join(".claude/hooks");
        std::fs::create_dir_all(&hooks).unwrap();
        std::fs::write(hooks.join("repograph-hook.mjs"), "console.error('[repograph-hook]');\n").unwrap();
        std::fs::write(dir.path().join(".claude/settings.local.json"),
            r#"{"hooks":{"SessionStart":[{"hooks":[{"type":"command","command":"node \"$CLAUDE_PROJECT_DIR/.claude/hooks/repograph-hook.mjs\""}]}]}}"#).unwrap();
        let r = install(dir.path(), Target::Claude, "zerograph").unwrap();
        assert!(hooks.join("repograph-hook.mjs").exists(), "a dangling entry would fail on every session start");
        assert!(r.notes.iter().any(|n| n.contains("settings.local.json")), "{:?}", r.notes);
    }

    /// The settings name the new script before the old one is removed, so a script the filesystem
    /// will not let go of is a note and the rest of the install still runs.
    #[cfg(unix)]
    #[test]
    fn an_old_hook_that_cannot_be_removed_is_a_note_and_not_the_end_of_the_install() {
        use std::os::unix::fs::PermissionsExt;
        let dir = root();
        install(dir.path(), Target::Claude, "zerograph").unwrap();
        let hooks = dir.path().join(".claude/hooks");
        std::fs::write(hooks.join("repograph-hook.mjs"), "console.error('[repograph-hook]');\n").unwrap();
        std::fs::set_permissions(&hooks, std::fs::Permissions::from_mode(0o555)).unwrap();
        // Root, and a filesystem that ignores the mode, can remove it anyway.
        let can_write = std::fs::write(hooks.join("probe"), "").is_ok();
        let r = install(dir.path(), Target::Claude, "zerograph");
        std::fs::set_permissions(&hooks, std::fs::Permissions::from_mode(0o755)).unwrap();
        if can_write { return; }
        let r = r.expect("a hook that stays is not an error");
        assert!(hooks.join("repograph-hook.mjs").exists());
        assert!(r.notes.iter().any(|n| n.contains("could not be removed")), "{:?}", r.notes);
    }

    #[cfg(unix)]
    #[test]
    fn a_claude_md_linked_to_agents_md_is_not_written_through_and_does_not_end_the_install() {
        let dir = root();
        let legacy = "# Rules\n\n<!-- repograph:begin -->\nrun `repograph ask`\n<!-- repograph:end -->\n";
        std::fs::write(dir.path().join("AGENTS.md"), legacy).unwrap();
        std::os::unix::fs::symlink("AGENTS.md", dir.path().join("CLAUDE.md")).unwrap();
        let r = install(dir.path(), Target::Claude, "zerograph").unwrap();
        assert!(r.notes.iter().any(|n| n.contains("symlink")), "{:?}", r.notes);
        assert_eq!(std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap(), legacy, "what the link points at is left alone");
        assert!(dir.path().join(".claude/settings.json").exists(), "the rest of the install ran");

        // `install-agent --claude --codex` runs Codex first, which writes the file the link points at.
        install(dir.path(), Target::Codex, "zerograph").unwrap();
        let r = install(dir.path(), Target::Claude, "zerograph").unwrap();
        assert!(!r.notes.iter().any(|n| n.contains("symlink")), "nothing left to write: {:?}", r.notes);
        let text = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert_eq!(text.matches(BEGIN).count(), 1, "{text}");
        assert!(!text.contains("repograph"), "{text}");
    }

    #[test]
    fn a_stanza_in_the_inner_claude_md_stays_the_one_updated_when_a_root_file_appears_later() {
        let dir = root();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(dir.path().join(".claude/CLAUDE.md"), "<!-- repograph:begin -->\nrun `repograph ask`\n<!-- repograph:end -->\n").unwrap();
        std::fs::write(dir.path().join("CLAUDE.md"), "# House rules\n").unwrap();
        install(dir.path(), Target::Claude, "zerograph").unwrap();
        let inner = std::fs::read_to_string(dir.path().join(".claude/CLAUDE.md")).unwrap();
        assert!(inner.contains(BEGIN) && !inner.contains("repograph"), "{inner}");
        assert_eq!(std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap(), "# House rules\n", "no second stanza beside the first");
    }

    #[test]
    fn a_machine_without_claude_gets_a_note_and_the_command_to_run() {
        let r = register_mcp(Path::new("."), "zerograph", &|_, _| None);
        assert_eq!(r.written, 0);
        assert!(r.notes[0].contains("claude mcp add zerograph -- zerograph --repo . mcp"), "{:?}", r.notes);
    }

    fn git_repo() -> tempfile::TempDir {
        let dir = root();
        let ok = std::process::Command::new("git").arg("-C").arg(dir.path()).args(["init", "-q"])
            .env_remove("GIT_DIR").env_remove("GIT_WORK_TREE").status().unwrap().success();
        assert!(ok, "git init");
        dir
    }

    /// The block as the last release that wrote one spelled it.
    const OLD_BLOCK: &str = "# zerograph:begin\n# Brings zerograph's index in line with what this pull or checkout moved, in the background.\nif [ \"$3\" != \"0\" ]; then zerograph update --detach >/dev/null 2>&1 || true; fi\n# zerograph:end\n";

    #[test]
    fn the_block_an_earlier_release_wrote_comes_out_and_the_rest_of_the_hook_stays() {
        let mine = format!("#!/bin/sh\n{OLD_BLOCK}npx lefthook run post-merge\n");
        assert_eq!(without_git_block(&mine).as_deref(), Some("#!/bin/sh\nnpx lefthook run post-merge\n"));
        let renamed = "#!/bin/sh\n# repograph:begin\nrepograph update --detach\n# repograph:end\nexec x\n";
        assert_eq!(without_git_block(renamed).as_deref(), Some("#!/bin/sh\nexec x\n"), "the block from before the rename goes too");
        let both = format!("#!/bin/sh\n{OLD_BLOCK}# repograph:begin\nold\n# repograph:end\necho hi\n");
        assert_eq!(without_git_block(&both).as_deref(), Some("#!/bin/sh\necho hi\n"));
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

    #[test]
    fn a_path_with_a_space_is_quoted_for_hand_use_and_matches_what_claude_printed() {
        assert_eq!(shell_word("/a b/it's"), "'/a b/it'\\''s'");
        assert_eq!(shell_word("--repo"), "--repo");
        assert!(said_in_order("Command: zerograph\nArgs: --repo /a b mcp", "zerograph", &["--repo", "/a b", "mcp"]));
        assert!(!said_in_order("Args: mcp --repo /a b", "zerograph", &["--repo", "/a b", "mcp"]));
        assert!(!said_in_order("Command: zerograph\nArgs: --repo /a b2 mcp", "zerograph", &["--repo", "/a b", "mcp"]), "another root");
    }

    #[test]
    fn installing_twice_writes_the_same_tree_and_says_it_changed_nothing() {
        let dir = root();
        let first = install(dir.path(), Target::Claude, "zerograph").unwrap();
        assert!(first.written > 0, "{first:?}");
        for rel in [".claude/hooks/zerograph-hook.mjs", ".claude/skills/zerograph/SKILL.md",
                    ".claude/agents/repo-scout.md", ".claude/settings.json", ".claude/CLAUDE.md"] {
            assert!(dir.path().join(rel).exists(), "{rel} was not written");
        }
        let second = install(dir.path(), Target::Claude, "zerograph").unwrap();
        assert_eq!(second.written, 0, "a second install is a no-op: {second:?}");
    }

    #[test]
    fn the_skill_names_the_release_that_wrote_it_and_another_release_says_it_replaced_it() {
        let dir = root();
        install(dir.path(), Target::Claude, "zerograph").unwrap();
        let path = dir.path().join(".claude/skills/zerograph/SKILL.md");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("---\nname: zerograph\n"), "the name is the directory's: {text}");
        assert_eq!(skill_version(&text), Some(VERSION));
        std::fs::write(&path, text.replace(&format!("version: \"{VERSION}\""), "version: \"0.0.1\"")).unwrap();
        let r = install(dir.path(), Target::Claude, "zerograph").unwrap();
        assert_eq!(r.written, 1, "{r:?}");
        assert!(r.notes.iter().any(|n| n.contains(&format!("rewritten from zerograph 0.0.1 to {VERSION}"))), "{r:?}");
        assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    }

    /// The skill was `repo-query` until 0.6.4. What a release wrote under that name goes, under
    /// either harness and whatever command it was installed with; a skill a team keeps under the
    /// name stays.
    #[test]
    fn the_repo_query_skill_an_earlier_release_wrote_is_removed_and_one_a_team_edited_is_kept() {
        for target in [Target::Claude, Target::Codex] {
            let dir = root();
            let old = dir.path().join(target.dir()).join("skills/repo-query");
            std::fs::create_dir_all(&old).unwrap();
            std::fs::write(old.join("SKILL.md"), with_command(LEGACY_SKILL, "pnpm exec zerograph")).unwrap();
            let r = install(dir.path(), target, "zerograph").unwrap();
            assert!(!old.exists(), "{target:?}: {r:?}");
            assert!(r.notes.iter().any(|n| n.contains("removed") && n.contains("repo-query")), "{r:?}");
            assert!(dir.path().join(target.dir()).join("skills/zerograph/SKILL.md").exists());
        }
        let dir = root();
        let old = dir.path().join(".claude/skills/repo-query");
        std::fs::create_dir_all(&old).unwrap();
        let edited = with_command(LEGACY_SKILL, "zerograph") + "\nOur own rule.\n";
        std::fs::write(old.join("SKILL.md"), &edited).unwrap();
        let r = install(dir.path(), Target::Claude, "zerograph").unwrap();
        assert_eq!(std::fs::read_to_string(old.join("SKILL.md")).unwrap(), edited);
        assert!(r.notes.iter().any(|n| n.contains("repo-query") && n.contains("left")), "{r:?}");
        std::fs::write(old.join("SKILL.md"), with_command(LEGACY_SKILL, "zerograph")).unwrap();
        std::fs::write(old.join("notes.md"), "ours\n").unwrap();
        install(dir.path(), Target::Claude, "zerograph").unwrap();
        assert!(old.join("notes.md").exists() && old.join("SKILL.md").exists(), "a directory with files of its own is a team's");
    }

    /// An installer that rewrote a repository's settings file would be a worse citizen than no
    /// installer: the hooks a team wrote are the reason the file exists.
    #[test]
    fn an_existing_settings_file_keeps_its_own_hooks() {
        let dir = root();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(dir.path().join(".claude/settings.json"),
            r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"echo mine"}]}]}}"#).unwrap();
        install(dir.path(), Target::Claude, "zerograph").unwrap();
        let v: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap()).unwrap();
        assert_eq!(v["model"], "opus", "a key that is not ours is untouched");
        assert!(v["hooks"]["Stop"].is_array(), "the repository's own hook survived");
        assert_eq!(v["hooks"]["SessionStart"].as_array().unwrap().len(), 1);
        assert_eq!(v["hooks"]["PreToolUse"][0]["matcher"], "Bash|Grep|Glob|Agent");
    }

    /// The upgrade case: an older entry of ours is replaced, not joined by a second copy that
    /// would fire the hook twice on every event.
    #[test]
    fn an_older_entry_of_ours_is_replaced_rather_than_doubled() {
        let dir = root();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(dir.path().join(".claude/settings.json"), r#"{"hooks":{"SessionStart":[
            {"matcher":"startup","hooks":[{"type":"command","command":"node /old/path/zerograph-hook.mjs"}]},
            {"matcher":"startup","hooks":[{"type":"command","command":"node .claude/hooks/repograph-notice.mjs"}]}
        ]}}"#).unwrap();
        let r = install(dir.path(), Target::Claude, "zerograph").unwrap();
        let v: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap()).unwrap();
        let arr = v["hooks"]["SessionStart"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "the old entry and the retired notice hook both went: {arr:?}");
        // The retired hook is a file the repository owns and a person listed: it goes, and the
        // install says so rather than leaving them to find out from a hook that stopped firing.
        assert!(r.notes.iter().any(|n| n.contains("repograph-notice") && n.contains("SessionStart")),
                "{:?}", r.notes);
        assert!(arr[0]["hooks"][0]["command"].as_str().unwrap().contains("CLAUDE_PROJECT_DIR"));
    }

    /// A narrowed matcher is a choice someone made, such as keeping the hook off Bash calls; a
    /// re-run that put the default back would undo it on every upgrade.
    #[test]
    fn a_matcher_set_on_our_entry_survives_a_re_run() {
        let dir = root();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(dir.path().join(".claude/settings.json"), r#"{"hooks":{"PreToolUse":[
            {"matcher":"Grep|Glob|Agent","hooks":[{"type":"command","command":"node /old/path/zerograph-hook.mjs"}]}
        ]}}"#).unwrap();
        install(dir.path(), Target::Claude, "zerograph").unwrap();
        let v: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap()).unwrap();
        let arr = v["hooks"]["PreToolUse"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "{arr:?}");
        assert_eq!(arr[0]["matcher"], "Grep|Glob|Agent");
        assert!(arr[0]["hooks"][0]["command"].as_str().unwrap().contains("CLAUDE_PROJECT_DIR"), "the rest of the entry is still ours to update");
        assert_eq!(v["hooks"]["SessionStart"][0]["matcher"], "startup|resume|clear|compact", "a fresh entry gets the default");
    }

    /// A matcher the stamp says we wrote is ours to move when the default changes; any other stays.
    #[test]
    fn a_matcher_the_stamp_records_moves_to_the_new_default() {
        let settings = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash|Grep","hooks":[{"type":"command","command":"node zerograph-hook.mjs"}]}]}}"#;
        let ours = merged_settings(settings, "h", "#!/usr/bin/env node\n// zerograph 0.6.4 wrote: PreToolUse=Bash|Grep\n").unwrap().0;
        assert!(ours.contains("\"Bash|Grep|Glob|Agent\""), "{ours}");
        let theirs = merged_settings(settings, "h", "#!/usr/bin/env node\n// zerograph 0.6.4 wrote: PreToolUse=Bash\n").unwrap().0;
        assert!(theirs.contains("\"Bash|Grep\""), "{theirs}");
    }

    #[test]
    fn the_stanza_lands_in_a_root_instructions_file_when_there_is_one_and_replaces_only_itself() {
        let dir = root();
        std::fs::write(dir.path().join("CLAUDE.md"), "# House rules\n\nRun the tests.\n").unwrap();
        install(dir.path(), Target::Claude, "pnpm exec zerograph").unwrap();
        let text = std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert!(text.starts_with("# House rules\n\nRun the tests.\n"), "{text}");
        assert!(text.contains("pnpm exec zerograph ask <words>"), "{text}");
        assert!(!dir.path().join(".claude/CLAUDE.md").exists(), "the root file was the one to use");

        // A re-install under a different invocation name replaces the block and nothing else.
        install(dir.path(), Target::Claude, "zerograph").unwrap();
        let text = std::fs::read_to_string(dir.path().join("CLAUDE.md")).unwrap();
        assert!(text.starts_with("# House rules\n\nRun the tests.\n"), "{text}");
        assert!(!text.contains("pnpm exec"), "the old block went: {text}");
        assert_eq!(text.matches(BEGIN).count(), 1, "one block, not two: {text}");
    }

    /// Read off the CLI rather than assumed — `agent/codex.md` records how. Codex reads the
    /// repository's root `AGENTS.md`, so a `.codex/AGENTS.md` is a file nothing would ever open;
    /// and its hooks live at `CODEX_HOME`, so no repository-scoped hooks file is written at all.
    #[test]
    fn codex_gets_its_stanza_at_the_root_and_no_repository_hooks_file() {
        let dir = root();
        install(dir.path(), Target::Codex, "zerograph").unwrap();
        assert!(dir.path().join(".codex/hooks/zerograph-hook.mjs").exists(), "something to point at");
        assert!(dir.path().join(".codex/skills/zerograph/SKILL.md").exists());
        let stanza = std::fs::read_to_string(dir.path().join("AGENTS.md")).unwrap();
        assert!(stanza.contains("zerograph ask <words>"), "{stanza}");
        assert!(!dir.path().join(".codex/AGENTS.md").exists(), "nothing reads that path");
        assert!(!dir.path().join(".codex/hooks.json").exists(), "hooks are CODEX_HOME-relative");
        assert!(!dir.path().join(".codex/agents").exists(), "the scout is a Claude Code definition");
    }

    #[test]
    fn the_codex_block_names_three_events_and_not_the_interceptor() {
        let v: serde_json::Value = serde_json::from_str(&codex_hooks_block("/abs/hook.mjs")).unwrap();
        let events: Vec<&String> = v["hooks"].as_object().unwrap().keys().collect();
        assert_eq!(events.len(), 3, "{events:?}");
        assert!(v["hooks"]["PreToolUse"].is_null(), "the interceptor is not carried over blind");
        assert!(v["hooks"]["SessionStart"][0]["hooks"][0]["command"].as_str().unwrap().contains("/abs/hook.mjs"));
    }

    #[test]
    fn a_settings_file_that_is_not_json_is_refused_by_name_rather_than_overwritten() {
        let dir = root();
        std::fs::create_dir_all(dir.path().join(".claude")).unwrap();
        std::fs::write(dir.path().join(".claude/settings.json"), "not json").unwrap();
        let err = install(dir.path(), Target::Claude, "zerograph").unwrap_err().to_string();
        assert!(err.contains("not JSON"), "{err}");
        assert_eq!(std::fs::read_to_string(dir.path().join(".claude/settings.json")).unwrap(), "not json");
    }

    /// `--command` is how this repository runs the binary, and a pnpm workspace has no bare
    /// `zerograph` on PATH: a file that still says the bare name tells an agent to run nothing.
    /// So every text either harness installs carries it, and not only the stanza.
    #[test]
    fn the_command_reaches_every_file_either_harness_installs() {
        for (target, texts) in [
            (Target::Claude, &[".claude/hooks/rule.txt", ".claude/skills/zerograph/SKILL.md",
                               ".claude/agents/repo-scout.md", ".claude/CLAUDE.md"][..]),
            (Target::Codex, &[".codex/hooks/rule.txt", ".codex/skills/zerograph/SKILL.md", "AGENTS.md"][..]),
        ] {
            let dir = root();
            install(dir.path(), target, "pnpm exec zerograph").unwrap();
            for rel in texts {
                let text = std::fs::read_to_string(dir.path().join(rel)).unwrap();
                assert!(text.contains("`pnpm exec zerograph "), "{rel} names the command: {text}");
                assert!(!text.contains("`zerograph ") && !text.contains("{{command}}") && !text.contains("{{version}}"), "{rel} still names the bare binary: {text}");
            }
            let script = std::fs::read_to_string(dir.path().join(target.dir()).join("hooks/zerograph-hook.mjs")).unwrap();
            assert!(script.contains("\nconst COMMAND = \"pnpm exec zerograph\";\n"), "{target:?}: the hook's hints read one declaration");
        }
    }

    /// A command is whatever a person typed, and the hook is JavaScript: it goes in as one JSON
    /// string, which JavaScript reads as a string literal, so nothing in it can run.
    #[test]
    fn the_hook_takes_the_command_as_one_json_string_and_the_default_as_it_ships() {
        assert_eq!(HOOK.matches(HOOK_COMMAND).count(), 1, "the declaration the installer replaces is in the hook once");
        assert_eq!(hook("zerograph").replacen(&stamp(), "", 1), HOOK, "the default installs the template byte for byte, under the stamp");
        let script = hook(r#"pnpm exec "re'po\graph" `x` ${y}"#);
        assert!(script.contains(r#"const COMMAND = "pnpm exec \"re'po\\graph\" `x` ${y}";"#), "the command is one string literal");
        assert_eq!(script.matches("const COMMAND = ").count(), 1);
    }

    /// An upgrade is a re-run, and a re-run under the same command writes nothing — for a command
    /// other than the default, and for both harnesses.
    #[test]
    fn a_second_install_under_a_named_command_writes_nothing_for_either_harness() {
        for target in [Target::Claude, Target::Codex] {
            let dir = root();
            std::fs::write(dir.path().join(target.instructions()), "# House rules\n").unwrap();
            assert!(install(dir.path(), target, "pnpm exec zerograph").unwrap().written > 0);
            let again = install(dir.path(), target, "pnpm exec zerograph").unwrap();
            assert_eq!(again.written, 0, "{target:?}: {again:?}");
        }
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
}
