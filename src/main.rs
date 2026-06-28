//! skillsmith CLI — a thin composition root over the `skillsmith` library crate.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "skillsmith", version, about = "Eval-gated skill optimizer")]
struct Cli {
    /// skillsmith home (where `projects/` lives). Default: $SKILLSMITH_HOME, else ~/.skillsmith.
    #[arg(long, global = true)]
    home: Option<String>,
    /// Disable color/ANSI in output (also auto-off when stdout is not a TTY,
    /// e.g. piped or relayed by a host agent).
    #[arg(long, global = true)]
    plain: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Full loop: baseline -> propose -> candidate eval -> gate -> stage.
    Run {
        /// Project name under <home>/projects/<name>/
        #[arg(long)]
        project: String,
        /// Validate config + run each verify_cmd in a worktree with NO LLM (no
        /// tokens) — catches a broken repo_path/verify_cmd before a real run.
        #[arg(long)]
        dry_run: bool,
        /// Re-run on every change to skill.md / config.toml / target files — a
        /// foreground dev loop (each pass spends tokens). Ignored with --dry-run.
        #[arg(long)]
        watch: bool,
    },
    /// Evaluate the current skill once (no optimization).
    Eval {
        #[arg(long)]
        project: String,
        /// Re-evaluate on every input change — the cheap loop (no optimize rounds).
        #[arg(long)]
        watch: bool,
    },
    /// Drift check (no LLM, no tokens): have the repo inputs changed since the last
    /// `run`, leaving the optimized skill possibly stale? Exits non-zero on drift so a
    /// git hook / CI can branch. Detection only — never re-runs or adopts.
    Check {
        #[arg(long)]
        project: String,
    },
    /// Benchmark: run the optimization N times (k-seed) and write a variance-aware
    /// scorecard + sweep.jsonl under <project>/bench/. Spends N× a run's tokens.
    Bench {
        #[arg(long)]
        project: String,
        /// Independent runs to aggregate (variance needs ≥2). Default 3.
        #[arg(long, default_value_t = 3)]
        seeds: u32,
    },
    /// Adopt the staged proposal: copy skill.staged.md over the live skill file.
    Adopt {
        /// Project name under <home>/projects/<name>/
        #[arg(long)]
        project: String,
    },
    /// List discovered projects under <home>/projects/.
    List,
    /// Seed the bundled demo project into <home> (idempotent).
    Init,
    /// Scaffold a new project. Inside a git repo, bare `skillsmith new` is enough:
    /// it goes repo-local and is named after the repo dir.
    New {
        /// Project name. Optional — defaults to the repo (or --repo) dir name.
        name: Option<String>,
        /// Path to the target git repo (selects central mode; edit config.toml later).
        #[arg(long)]
        repo: Option<String>,
        /// Force repo-local even outside a git repo or alongside --repo. (Repo-local is
        /// already the default inside a git repo when --repo is not given.)
        #[arg(long)]
        local: bool,
    },
    /// Deploy the adopted skill where a coding agent reads it — a Claude skill file
    /// (`--as skill`) or an always-on context file (`--as context`). Pure file ops, no LLM.
    Deploy {
        /// Project name under <home>/projects/<name>/
        #[arg(long)]
        project: String,
        /// "skill" -> .claude/skills/<name>/SKILL.md ; "context" -> inject into a context file.
        #[arg(long = "as", default_value = "skill")]
        as_kind: String,
        /// context: destination file (default CLAUDE.md).
        #[arg(long)]
        to: Option<String>,
        /// skill: frontmatter `description` / trigger phrases.
        #[arg(long)]
        desc: Option<String>,
        /// skill/block name (default: project name).
        #[arg(long)]
        name: Option<String>,
        /// deploy root (default: the git repo enclosing the project's .skillsmith/).
        #[arg(long)]
        root: Option<String>,
        /// context: csv of agents -> files (claude=CLAUDE.md, codex=AGENTS.md, gemini=GEMINI.md).
        #[arg(long)]
        agents: Option<String>,
    },
}

/// A resolved skillsmith home plus whether it may auto-seed the bundled demo.
struct Home {
    path: PathBuf,
    /// Only the global default (~/.skillsmith) or an explicit home auto-seeds the
    /// demo; a discovered repo-local `.skillsmith/` never does (it would land the
    /// demo fixture inside the user's repo).
    auto_seed: bool,
}

/// Resolve the home dir: explicit `--home` > `$SKILLSMITH_HOME` > a repo-local
/// `.skillsmith/` discovered above cwd > the per-user default `~/.skillsmith`.
/// The repo-local branch is what makes a committed-in-repo project work with a bare
/// `skillsmith run` (like `.git`); the per-user default keeps the installed binary
/// working from any cwd with no env var (the demo auto-seeds there on first run).
fn resolve_home(opt: Option<String>) -> Home {
    if let Some(h) = opt {
        return Home { path: PathBuf::from(h), auto_seed: true };
    }
    if let Ok(h) = std::env::var("SKILLSMITH_HOME")
        && !h.is_empty()
    {
        return Home { path: PathBuf::from(h), auto_seed: true };
    }
    let global = std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".skillsmith"));
    // A repo-local `.skillsmith/` that is NOT the global home wins (nearest first).
    // If the only one found IS ~/.skillsmith, fall through so it auto-seeds.
    if let Ok(cwd) = std::env::current_dir()
        && let Some(local) = skillsmith::config::discover_dot_skillsmith(&cwd)
        && Some(&local) != global.as_ref()
    {
        return Home { path: local, auto_seed: false };
    }
    if let Some(g) = global {
        return Home { path: g, auto_seed: true };
    }
    Home { path: PathBuf::from(".skillsmith"), auto_seed: true }
}

/// `skillsmith new` with maximal DX: auto-pick repo-local vs central, and auto-derive
/// the project name. Inside a git repo, bare `new` => repo-local project named after the
/// repo. `--repo <path>` selects central; an explicit name or `--local` always wins.
fn cmd_new(home: &Home, name: Option<String>, repo: Option<String>, local_flag: bool) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let git_root = skillsmith::config::enclosing_git_root(&cwd);
    // Repo-local by default inside a git repo, unless --repo points elsewhere (central).
    let local = local_flag || (repo.is_none() && git_root.is_some());
    let target_home = if local {
        git_root.clone().unwrap_or_else(|| cwd.clone()).join(".skillsmith")
    } else {
        home.path.clone()
    };
    // Derive the name when omitted: from --repo's dir, else the enclosing git repo dir.
    let name = match name {
        Some(n) => n,
        None => {
            let basis = repo
                .as_deref()
                .map(PathBuf::from)
                .or_else(|| if local { git_root.clone().or_else(|| Some(cwd.clone())) } else { None });
            basis
                .as_deref()
                .and_then(skillsmith::config::slug_from_path)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "couldn't derive a project name — run inside a git repo, or pass one: \
`skillsmith new <name> [--repo <path>]`"
                    )
                })?
        }
    };
    skillsmith::optimize::new_project(&target_home, &name, repo.as_deref(), local)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let plain = cli.plain;
    let home = resolve_home(cli.home);
    let path = home.path.as_path();
    // Auto-seed the demo only for the global/explicit home, never for a discovered
    // repo-local `.skillsmith/` (it must not pull the demo fixture into the repo).
    let maybe_seed = || -> Result<()> {
        if home.auto_seed {
            skillsmith::seed::ensure_seeded(path)?;
        }
        Ok(())
    };
    match cli.cmd {
        Cmd::Init => skillsmith::seed::init(path),
        Cmd::Run { project, dry_run, watch } => {
            maybe_seed()?;
            if dry_run {
                skillsmith::optimize::dry_run(path, &project).await
            } else if watch {
                skillsmith::optimize::run_watch(path, &project, plain).await
            } else {
                skillsmith::optimize::run(path, &project, plain).await
            }
        }
        Cmd::Eval { project, watch } => {
            maybe_seed()?;
            if watch {
                skillsmith::optimize::eval_watch(path, &project, plain).await
            } else {
                skillsmith::optimize::eval_only(path, &project, plain).await
            }
        }
        Cmd::Check { project } => {
            maybe_seed()?;
            skillsmith::optimize::check(path, &project)
        }
        Cmd::Bench { project, seeds } => {
            maybe_seed()?;
            skillsmith::optimize::bench(path, &project, seeds, plain).await
        }
        Cmd::Adopt { project } => {
            maybe_seed()?;
            skillsmith::optimize::adopt(path, &project)
        }
        Cmd::List => {
            maybe_seed()?;
            skillsmith::optimize::list(path)
        }
        Cmd::New { name, repo, local } => cmd_new(&home, name, repo, local),
        Cmd::Deploy { project, as_kind, to, desc, name, root, agents } => {
            maybe_seed()?;
            skillsmith::deploy::deploy(
                path,
                &project,
                &skillsmith::deploy::DeployOpts { as_kind, to, desc, name, root, agents },
            )
        }
    }
}
