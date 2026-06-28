//! `skillsmith deploy` — the last mile: take an adopted skill and place it where a
//! coding agent actually reads it. The optimizer + `adopt` produce a project-local
//! `skill.md`, but nothing auto-reads that path. `deploy` is pure file mechanics — **no
//! LLM, no tokens**:
//!
//! - `--as skill`   → wrap the body with `name`/`description` frontmatter and write
//!   `<repo>/.claude/skills/<name>/SKILL.md` (Claude auto-loads it on-demand when the
//!   description matches).
//! - `--as context` → inject the body, between idempotent markers, into an always-on
//!   context file (`CLAUDE.md` / `AGENTS.md` / `GEMINI.md`) — the path all three agents
//!   read.
//!
//! The deploy root is the **real repo** enclosing the project's `.skillsmith/` home —
//! NOT `repo_path`, which for a fixture-based project is the synthetic eval sandbox.

use crate::config::{Project, enclosing_git_root};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// Options for `deploy` (parsed from the CLI flags).
pub struct DeployOpts {
    /// "skill" (a Claude `.claude/skills` file) | "context" (an always-on context file).
    pub as_kind: String,
    /// context: destination file (default `CLAUDE.md`).
    pub to: Option<String>,
    /// skill: frontmatter `description` / trigger phrases.
    pub desc: Option<String>,
    /// skill/block name (default: project name).
    pub name: Option<String>,
    /// deploy root override (default: the git repo enclosing the project's `.skillsmith/`).
    pub root: Option<String>,
    /// context: csv of agents → files (`claude`=CLAUDE.md, `codex`=AGENTS.md, `gemini`=GEMINI.md).
    pub agents: Option<String>,
}

/// YAML double-quote a one-line scalar (the description goes on a single line).
fn yaml_quote(s: &str) -> String {
    let esc = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], " ");
    format!("\"{esc}\"")
}

/// Build a Claude `SKILL.md`: `name`/`description` frontmatter + the skill body. Pure.
pub fn wrap_skill(name: &str, description: &str, body: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {desc}\n---\n\n{body}\n",
        desc = yaml_quote(description),
        body = body.trim()
    )
}

/// Inject `body` into `existing` between idempotent markers for `name`. If the markers
/// already exist, the block between them is replaced (re-deploy updates in place);
/// otherwise the block is appended. Pure.
pub fn inject_context(existing: &str, name: &str, body: &str) -> String {
    let start = format!("<!-- skillsmith:{name} START -->");
    let end = format!("<!-- skillsmith:{name} END -->");
    let block = format!("{start}\n{}\n{end}", body.trim());
    let (s, e) = match (existing.find(&start), existing.find(&end)) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => {
            let mut out = existing.trim_end().to_string();
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&block);
            out.push('\n');
            return out;
        }
    };
    format!("{}{}{}", &existing[..s], block, &existing[e + end.len()..])
}

/// The real repo to deploy into: `--root` > the git repo enclosing the project's
/// `.skillsmith/` home > cwd. NOT `repo_path` (which may be a fixture sandbox).
fn deploy_root(project: &Project, override_root: Option<&str>) -> Result<PathBuf> {
    if let Some(r) = override_root {
        return Ok(PathBuf::from(r));
    }
    if let Some(git) = enclosing_git_root(&project.dir) {
        return Ok(git);
    }
    std::env::current_dir().context("no --root and no enclosing git repo found; pass --root <path>")
}

fn resolve_name(opts: &DeployOpts, project: &Project) -> String {
    if let Some(n) = &opts.name {
        return n.clone();
    }
    if !project.cfg.deploy.name.is_empty() {
        return project.cfg.deploy.name.clone();
    }
    project.cfg.name.clone()
}

fn resolve_desc(opts: &DeployOpts, project: &Project, name: &str) -> String {
    if let Some(d) = &opts.desc {
        return d.clone();
    }
    if !project.cfg.deploy.description.is_empty() {
        return project.cfg.deploy.description.clone();
    }
    eprintln!(
        "warning: no --desc and no [deploy] description in config.toml — wrote a placeholder. \
Edit the SKILL.md `description:` with real trigger phrases (what the user would say) so the agent \
auto-loads it."
    );
    format!("Use when working on {name} in this repo. Replace with concrete trigger phrases.")
}

/// context targets: `--agents` csv → files, else `--to`, else `CLAUDE.md`.
fn context_targets(opts: &DeployOpts) -> Vec<String> {
    if let Some(a) = &opts.agents {
        return a
            .split(',')
            .filter_map(|s| match s.trim() {
                "" => None,
                "claude" => Some("CLAUDE.md".to_string()),
                "codex" => Some("AGENTS.md".to_string()),
                "gemini" => Some("GEMINI.md".to_string()),
                other => Some(other.to_string()),
            })
            .collect();
    }
    vec![opts.to.clone().unwrap_or_else(|| "CLAUDE.md".to_string())]
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

/// `skillsmith deploy` — place a project's adopted skill where the agent reads it.
pub fn deploy(home: &Path, project_name: &str, opts: &DeployOpts) -> Result<()> {
    let project = Project::load(home, project_name)?;
    let staged = project.dir.join("skill.staged.md");
    if staged.exists() {
        eprintln!(
            "warning: an unadopted staged proposal exists ({}). deploy uses the LIVE skill — run \
`skillsmith adopt --project {}` first to deploy the optimized one.",
            staged.display(),
            project_name
        );
    }
    let body = std::fs::read_to_string(project.skill_path())
        .with_context(|| format!("reading live skill {}", project.skill_path().display()))?;
    let root = deploy_root(&project, opts.root.as_deref())?;
    let name = resolve_name(opts, &project);

    match opts.as_kind.as_str() {
        "skill" => {
            let desc = resolve_desc(opts, &project, &name);
            let dest = root.join(".claude/skills").join(&name).join("SKILL.md");
            write_file(&dest, &wrap_skill(&name, &desc, &body))?;
            println!("deployed skill -> {}", dest.display());
            println!(
                "  Claude auto-loads it when the description matches; edit the trigger phrases if needed."
            );
        }
        "context" => {
            for f in context_targets(opts) {
                let dest = root.join(&f);
                let existing = std::fs::read_to_string(&dest).unwrap_or_default();
                write_file(&dest, &inject_context(&existing, &name, &body))?;
                println!(
                    "deployed context -> {} (block `skillsmith:{}`)",
                    dest.display(),
                    name
                );
            }
        }
        other => bail!("unknown --as '{other}' (use: skill | context)"),
    }
    Ok(())
}
