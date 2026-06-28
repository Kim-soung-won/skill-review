//! Project config + eval task model. A "project" is a per-repo adapter living at
//! `projects/<name>/config.toml` — this is the cross-project reuse seam: the
//! optimizer core is project-agnostic; only these files are project-specific.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    /// Git repo to worktree for the execution judge. Omit (repo-local mode) to
    /// default to the git repo that encloses the project's `.skillsmith/` home.
    #[serde(default)]
    pub repo_path: String,
    /// Skill file (relative to the project dir) being optimized.
    pub skill_file: String,
    #[serde(default = "default_agent_model")]
    pub agent_model: String,
    #[serde(default = "default_optimizer_model")]
    pub optimizer_model: String,
    /// LLM backend: "claude" | "codex" | "gemini" (installed CLI, no API key) |
    /// "genai" (raw API via ANTHROPIC_API_KEY) | "cli" (custom `provider_cmd`).
    #[serde(default = "default_provider")]
    pub provider: String,
    /// Custom CLI base command (the prompt is appended as the final arg). Used
    /// when `provider = "cli"`, or to override a preset's command.
    #[serde(default)]
    pub provider_cmd: Vec<String>,
    /// Per-stage CLI command override for the cheap agent (eval) stage. Lets a CLI
    /// provider tier down to a smaller model there, e.g.
    /// `["claude","-p","--model","claude-haiku-4-5"]` or `["codex","exec","-m","gpt-5-mini"]`.
    /// Empty -> use `provider_cmd`. Ignored by `genai` (which tiers by `agent_model`).
    #[serde(default)]
    pub agent_provider_cmd: Vec<String>,
    /// Per-stage CLI command override for the optimizer (propose) stage. Empty ->
    /// use `provider_cmd`. Ignored by `genai` (which tiers by `optimizer_model`).
    #[serde(default)]
    pub optimizer_provider_cmd: Vec<String>,
    #[serde(default = "default_rounds")]
    pub rounds: u32,
    #[serde(default, rename = "task")]
    pub tasks: Vec<Task>,
    /// Optional `[deploy]` defaults for `skillsmith deploy` (skill name + the
    /// frontmatter `description` trigger phrases). Both optional; CLI flags override.
    #[serde(default)]
    pub deploy: DeployConfig,
}

/// `[deploy]` defaults — let a project pin its skill name + trigger phrases so
/// `skillsmith deploy` needs no flags (and works headless / in CI).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeployConfig {
    /// Skill/block name override (default: the project `name`).
    #[serde(default)]
    pub name: String,
    /// `description:` trigger phrases for `--as skill` (default: `--desc`, else a placeholder).
    #[serde(default)]
    pub description: String,
}

/// Which split a task belongs to. **train** — the optimizer sees its failures and
/// proposes edits against them. **val** — held out from the optimizer; the gate
/// scores accept/reject on these. **test** — never run during optimization; the best
/// skill is evaluated on these ONCE at the end for an unbiased final number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskSplit {
    Train,
    Val,
    Test,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub id: String,
    /// What the agent must do (the test file is held out, not shown).
    pub intent: String,
    /// Files shown to the agent as context (relative to repo_path).
    #[serde(default)]
    pub context_files: Vec<String>,
    /// Files the agent is expected to write (advisory hint in the prompt).
    #[serde(default)]
    pub target_files: Vec<String>,
    /// Held out from training: the optimizer never sees this task's failures, and
    /// the gate scores ONLY held-out tasks (when any exist). Default false.
    /// Back-compat alias for `split = "val"` — `split` wins if both are set.
    #[serde(default)]
    pub holdout: bool,
    /// Explicit train/val/test split. Omitted -> `val` when `holdout = true`, else `train`.
    #[serde(default)]
    pub split: Option<TaskSplit>,
    /// Optional shell run in the worktree before applying edits (seed state).
    #[serde(default)]
    pub setup_cmd: String,
    /// Shell run in the worktree after edits; exit code 0 == pass.
    pub verify_cmd: String,
}

impl Task {
    /// Resolved split: explicit `split` wins; else `val` if `holdout`, else `train`.
    pub fn split(&self) -> TaskSplit {
        self.split.unwrap_or(if self.holdout {
            TaskSplit::Val
        } else {
            TaskSplit::Train
        })
    }
}

fn default_agent_model() -> String {
    "claude-sonnet-4-6".to_string()
}
fn default_optimizer_model() -> String {
    "claude-opus-4-8".to_string()
}
fn default_provider() -> String {
    "claude".to_string()
}
fn default_rounds() -> u32 {
    3
}

/// A loaded project: its directory + parsed config.
pub struct Project {
    pub dir: PathBuf,
    pub cfg: ProjectConfig,
}

impl Project {
    pub fn load(home: &Path, name: &str) -> Result<Self> {
        let dir = home.join("projects").join(name);
        let cfg_path = dir.join("config.toml");
        let text = std::fs::read_to_string(&cfg_path)
            .with_context(|| format!("reading {}", cfg_path.display()))?;
        let cfg: ProjectConfig =
            toml::from_str(&text).with_context(|| format!("parsing {}", cfg_path.display()))?;
        Ok(Self { dir, cfg })
    }

    /// Absolute path of the repo to worktree.
    /// - Absolute `repo_path`: used as-is.
    /// - Relative `repo_path`: resolved against the project dir, so a project is
    ///   location-independent (works from any cwd), then canonicalized.
    /// - Empty/omitted (repo-local mode): the git repo enclosing the project dir
    ///   (i.e. the parent of the `.skillsmith/` home) — a committed-in-repo project
    ///   needs no path at all.
    pub fn repo(&self) -> Result<PathBuf> {
        let raw = self.cfg.repo_path.trim();
        if raw.is_empty() {
            return enclosing_git_root(&self.dir).with_context(|| {
                format!(
                    "repo_path is empty and no enclosing git repo was found above {} — \
set repo_path in config.toml",
                    self.dir.display()
                )
            });
        }
        let p = Path::new(raw);
        if p.is_absolute() {
            return Ok(p.to_path_buf());
        }
        let joined = self.dir.join(p);
        Ok(std::fs::canonicalize(&joined).unwrap_or(joined))
    }

    pub fn skill_path(&self) -> PathBuf {
        self.dir.join(&self.cfg.skill_file)
    }
}

/// Walk up from `start` to find the nearest ancestor containing a `.skillsmith/`
/// directory; return that `.skillsmith` path (the repo-local home). Nearest wins,
/// mirroring how git locates its root. Pure (no env/cwd) so it is unit-testable.
pub fn discover_dot_skillsmith(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let candidate = dir.join(".skillsmith");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

/// Walk up from `start` to the nearest ancestor that is a git repo root (has `.git`).
pub fn enclosing_git_root(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// Derive a project-name slug from a path's final component: ASCII-lowercase
/// alphanumerics kept, every other run collapsed to a single `-`, ends trimmed.
/// Used to auto-name a project after its repo dir so `skillsmith new` needs no args.
pub fn slug_from_path(p: &Path) -> Option<String> {
    let name = p.file_name()?.to_string_lossy();
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let slug = out.trim_matches('-').to_string();
    (!slug.is_empty()).then_some(slug)
}

/// Scaffold a new project adapter at `<home>/projects/<name>/` (config + skill).
/// This is what `skillsmith new` calls — no hand-creating directories. In `local`
/// (repo-local) mode `repo_path` is omitted (defaults to the enclosing git repo) and
/// a scratch `.gitignore` is written so `skill.md`/`config.toml` commit but the
/// generated `skill.staged.md`/`report.md` do not.
pub fn scaffold_project(home: &Path, name: &str, repo: Option<&str>, local: bool) -> Result<PathBuf> {
    let dir = home.join("projects").join(name);
    if dir.exists() {
        bail!("project already exists: {}", dir.display());
    }
    std::fs::create_dir_all(&dir)?;
    let repo_line = match repo {
        Some(r) => format!("repo_path = \"{r}\"\n"),
        None if local => "# repo_path omitted -> the git repo enclosing this .skillsmith/ dir\n\
repo_path = \"\"\n"
            .to_string(),
        None => "repo_path = \"/path/to/your/git/repo\"\n".to_string(),
    };
    let config = format!(
        "name = \"{name}\"\n\
{repo_line}\
skill_file = \"skill.md\"\n\
provider = \"claude\"        # claude | codex | gemini (installed CLI, no key) | genai\n\
# Tier the cheap agent stage to a smaller model (CLI providers; genai tiers by agent_model):\n\
# agent_provider_cmd = [\"claude\", \"-p\", \"--model\", \"claude-haiku-4-5\"]\n\
rounds = 3\n\
\n\
# Add eval tasks. Each runs in an isolated git worktree of repo_path (cwd = worktree root).\n\
# Keep the test file OUT of context_files (held out) so the SKILL carries the knowledge.\n\
[[task]]\n\
id = \"example\"\n\
intent = \"Describe what the agent must do (do not leak the answer).\"\n\
context_files = []\n\
target_files = []\n\
verify_cmd = \"echo replace-me; false\"   # exit 0 = pass\n"
    );
    std::fs::write(dir.join("config.toml"), config)?;
    let skill = format!(
        "# {name} — agent skill\n\n\
Seed conventions (the optimizer refines this against the eval tasks).\n\
Put durable, project-specific rules here.\n"
    );
    std::fs::write(dir.join("skill.md"), skill)?;
    if local {
        ensure_local_gitignore(home)?;
    }
    Ok(dir)
}

/// Write `<home>/.gitignore` (once) so a repo-local `.skillsmith/` commits the
/// durable artifacts (`skill.md`, `config.toml`) but ignores the generated scratch.
fn ensure_local_gitignore(home: &Path) -> Result<()> {
    let gi = home.join(".gitignore");
    if gi.exists() {
        return Ok(());
    }
    std::fs::write(
        &gi,
        "# skillsmith scratch — commit skill.md + config.toml; ignore generated artifacts\n\
**/skill.staged.md\n\
**/report.md\n\
**/results.json\n\
**/bench/\n\
**/.last-run\n",
    )
    .with_context(|| format!("writing {}", gi.display()))?;
    Ok(())
}

/// One row of `skillsmith list`.
pub struct ProjectSummary {
    pub name: String,
    pub tasks: usize,
    pub repo: String,
}

/// Discover every project under `<home>/projects/*/config.toml`. New project
/// folders appear automatically — no code or integration change needed.
pub fn list_projects(home: &Path) -> Result<Vec<ProjectSummary>> {
    let base = home.join("projects");
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(&base) {
        Ok(e) => e,
        Err(_) => return Ok(out),
    };
    for entry in entries.flatten() {
        let cfg_path = entry.path().join("config.toml");
        if !cfg_path.is_file() {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&cfg_path)
            && let Ok(cfg) = toml::from_str::<ProjectConfig>(&text)
        {
            out.push(ProjectSummary {
                name: cfg.name,
                tasks: cfg.tasks.len(),
                repo: if cfg.repo_path.trim().is_empty() {
                    "(enclosing repo)".to_string()
                } else {
                    cfg.repo_path
                },
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
