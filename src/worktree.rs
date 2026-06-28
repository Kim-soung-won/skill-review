//! Isolated git worktrees for the execution judge. Each task runs in a fresh
//! detached worktree of the target repo at HEAD, so parallel/repeated evals
//! never mutate the working tree. Shells out to `git` (no libgit2 dependency).

use anyhow::{Result, bail};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::process::Command;

static COUNTER: AtomicU64 = AtomicU64::new(0);

fn unique_dir() -> PathBuf {
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!(
        "skillsmith-wt-{}-{}-{}",
        std::process::id(),
        nanos,
        n
    ))
}

/// `git worktree add --detach <tmp> HEAD` — returns the worktree path.
pub async fn add(repo: &Path) -> Result<PathBuf> {
    let wt = unique_dir();
    let path = wt
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("non-utf8 worktree path"))?;
    let out = Command::new("git")
        .current_dir(repo)
        .args(["worktree", "add", "--detach", path, "HEAD"])
        .output()
        .await?;
    if !out.status.success() {
        bail!(
            "git worktree add failed (repo needs >=1 commit): {}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    Ok(wt)
}

/// `git worktree remove --force <tmp>` — best-effort cleanup.
pub async fn remove(repo: &Path, wt: &Path) -> Result<()> {
    if let Some(path) = wt.to_str() {
        let _ = Command::new("git")
            .current_dir(repo)
            .args(["worktree", "remove", "--force", path])
            .output()
            .await?;
    }
    Ok(())
}
