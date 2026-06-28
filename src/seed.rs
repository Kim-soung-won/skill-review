//! Zero-config seeding: materialize the bundled `demo` project into the home dir
//! (default `~/.skillsmith`) so the installed binary works from any cwd with no
//! env var. The demo's eval fixture is a self-contained git repo created here (it
//! is NOT committed in the skillsmith repo) so the execution judge can
//! `git worktree` it.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

const DEMO_CONFIG: &str = include_str!("../assets/demo/config.toml");
const DEMO_SKILL: &str = include_str!("../assets/demo/skill.md");
const FX_STRINGS: &str = include_str!("../assets/demo/fixture/strings.py");
const FX_TEST_STRINGS: &str = include_str!("../assets/demo/fixture/test_strings.py");
const FX_FORMAT: &str = include_str!("../assets/demo/fixture/format.py");
const FX_TEST_FORMAT: &str = include_str!("../assets/demo/fixture/test_format.py");

/// Seed the demo only if the home has no projects yet (idempotent, cheap).
/// Called before run/eval/list so a fresh install "just works".
pub fn ensure_seeded(home: &Path) -> Result<()> {
    if crate::config::list_projects(home)?.is_empty() {
        write_demo(home)?;
    }
    Ok(())
}

/// `skillsmith init` — (re)materialize a pristine demo and report the path.
pub fn init(home: &Path) -> Result<()> {
    let dir = home.join("projects").join("demo");
    if dir.exists() {
        std::fs::remove_dir_all(&dir).with_context(|| format!("resetting {}", dir.display()))?;
    }
    write_demo(home)?;
    println!("seeded demo at {}", dir.display());
    println!("  try:  skillsmith run --project demo");
    Ok(())
}

fn write_demo(home: &Path) -> Result<()> {
    let dir = home.join("projects").join("demo");
    let fixture = dir.join("fixture");
    std::fs::create_dir_all(&fixture).context("creating demo dir")?;
    write_if_absent(&dir.join("config.toml"), DEMO_CONFIG)?;
    write_if_absent(&dir.join("skill.md"), DEMO_SKILL)?;
    write_if_absent(&fixture.join("strings.py"), FX_STRINGS)?;
    write_if_absent(&fixture.join("test_strings.py"), FX_TEST_STRINGS)?;
    write_if_absent(&fixture.join("format.py"), FX_FORMAT)?;
    write_if_absent(&fixture.join("test_format.py"), FX_TEST_FORMAT)?;
    ensure_git_repo(&fixture)?;
    Ok(())
}

fn write_if_absent(path: &Path, content: &str) -> Result<()> {
    if !path.exists() {
        std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))?;
    }
    Ok(())
}

/// `git init` + commit the fixture (idempotent) so ExecJudge can worktree it.
fn ensure_git_repo(dir: &Path) -> Result<()> {
    if dir.join(".git").exists() {
        return Ok(());
    }
    run_git(dir, &["init", "-q"])?;
    run_git(dir, &["add", "-A"])?;
    run_git(
        dir,
        &[
            "-c",
            "user.name=skillsmith",
            "-c",
            "user.email=skillsmith@local",
            "commit",
            "-q",
            "-m",
            "demo fixture",
        ],
    )?;
    Ok(())
}

fn run_git(dir: &Path, args: &[&str]) -> Result<()> {
    let ok = Command::new("git")
        .current_dir(dir)
        .args(args)
        .status()
        .with_context(|| format!("running git {args:?}"))?
        .success();
    anyhow::ensure!(ok, "git {args:?} failed in {}", dir.display());
    Ok(())
}
