//! 제로 설정 시딩: 번들 `demo` 프로젝트를 홈 디렉토리(기본값 `~/.skillsmith`)에
//! 구체화 — 설치된 바이너리가 env var 없이 어떤 cwd에서도 동작하도록.
//! 데모의 eval fixture는 여기서 생성되는 독립 git 레포 (skillsmith 레포에는 커밋되지 않음)로,
//! 실행 judge가 `git worktree`할 수 있게 한다.

use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

const DEMO_CONFIG: &str = include_str!("../assets/demo/config.toml");
const DEMO_SKILL: &str = include_str!("../assets/demo/skill.md");
const FX_STRINGS: &str = include_str!("../assets/demo/fixture/strings.py");
const FX_TEST_STRINGS: &str = include_str!("../assets/demo/fixture/test_strings.py");
const FX_FORMAT: &str = include_str!("../assets/demo/fixture/format.py");
const FX_TEST_FORMAT: &str = include_str!("../assets/demo/fixture/test_format.py");

/// 홈에 프로젝트가 없을 때만 데모를 시드 (idempotent, 저렴).
/// run/eval/list 전에 호출해 새로 설치해도 "바로 동작"하도록.
pub fn ensure_seeded(home: &Path) -> Result<()> {
    if crate::config::list_projects(home)?.is_empty() {
        write_demo(home)?;
    }
    Ok(())
}

/// `skillsmith init` — 깨끗한 데모를 (재)구체화하고 경로를 출력.
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

/// fixture를 `git init` + 커밋 (idempotent) — ExecJudge가 worktree할 수 있도록.
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
