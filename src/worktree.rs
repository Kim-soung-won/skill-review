//! 실행 judge용 격리된 git worktree. 각 태스크는 HEAD의 대상 레포에 대한
//! 새로운 detached worktree에서 실행되므로, 병렬/반복 eval이 작업 트리를 절대
//! 수정하지 않는다. `git`을 셸 호출 (libgit2 의존성 없음).

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

/// `git worktree add --detach <tmp> HEAD` — worktree 경로 반환.
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

/// `git worktree remove --force <tmp>` — 최선 노력 정리.
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
