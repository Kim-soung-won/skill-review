//! 채점 포트 + 어댑터.
//!
//! [`Judge`]가 포트. [`ExecJudge`]는 에이전트의 편집을 격리된
//! `git worktree`에 적용하고 태스크의 verify 커맨드를 실행한다. 점수는 연속값:
//! 통과한 개별 테스트 케이스의 비율 (러너 출력에서 파싱), 카운트를 찾을 수 없으면
//! 바이너리 exit code로 폴백 — 옵티마이저가 실제로 올라갈 수 있는 연속 신호.
//! 새 채점기는 [`Judge`]를 통해 추가 가능.

use crate::agent::Edit;
use crate::config::Task;
use crate::worktree;
use anyhow::Result;
use std::path::Path;
use tokio::process::Command;

pub struct Outcome {
    pub id: String,
    /// verify 커맨드 전체가 exit 0 (모든 케이스 통과).
    pub passed: bool,
    /// 채점 점수 [0,1]: 통과한 테스트 케이스 비율 (1.0 == passed).
    pub score: f64,
    /// 잘린 verify 출력 (실패 시 옵티마이저에게 피드백).
    pub detail: String,
}

/// 포트: 한 태스크에 대한 에이전트의 편집을 채점한다.
#[allow(async_fn_in_trait)]
pub trait Judge {
    async fn run(&self, repo: &Path, task: &Task, edits: &[Edit]) -> Result<Outcome>;
}

/// 기본 어댑터: 적용된 편집에 대해 레포 자체의 verify 커맨드를 실행한다.
pub struct ExecJudge;

impl Judge for ExecJudge {
    async fn run(&self, repo: &Path, task: &Task, edits: &[Edit]) -> Result<Outcome> {
        let wt = worktree::add(repo).await?;
        let result = run_in_worktree(&wt, task, edits).await;
        worktree::remove(repo, &wt).await.ok();
        let (code, detail) = result?;
        Ok(Outcome {
            id: task.id.clone(),
            passed: code == 0,
            score: grade(code, &detail),
            detail,
        })
    }
}

async fn run_shell(dir: &Path, cmd: &str) -> Result<(i32, String)> {
    let out = Command::new("bash")
        .current_dir(dir)
        .arg("-lc")
        .arg(cmd)
        .output()
        .await?;
    let mut s = String::from_utf8_lossy(&out.stdout).to_string();
    s.push_str(&String::from_utf8_lossy(&out.stderr));
    Ok((out.status.code().unwrap_or(-1), s))
}

async fn run_in_worktree(wt: &Path, task: &Task, edits: &[Edit]) -> Result<(i32, String)> {
    if !task.setup_cmd.is_empty() {
        run_shell(wt, &task.setup_cmd).await?;
    }
    for e in edits {
        let path = wt.join(&e.path);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).ok();
        }
        std::fs::write(&path, &e.content)?;
    }
    let (code, out) = run_shell(wt, &task.verify_cmd).await?;
    let detail: String = out.chars().take(2000).collect();
    Ok((code, detail))
}

/// 채점 점수 [0,1]: 러너 요약에서 파싱한 개별 테스트 케이스 통과 비율.
/// 카운트를 찾을 수 없으면 바이너리 exit code로 폴백
/// (테스트가 아닌 verify 커맨드도 1.0/0.0으로 채점됨).
pub fn grade(code: i32, output: &str) -> f64 {
    if let Some(s) = parse_pytest(output).or_else(|| parse_unittest(output)) {
        return s;
    }
    if code == 0 { 1.0 } else { 0.0 }
}

/// pytest: "... 3 passed, 1 failed, 2 errors in 0.1s ..."
fn parse_pytest(out: &str) -> Option<f64> {
    // pytest 요약은 한 줄. passed/failed가 포함된 마지막 줄만 스캔 —
    // 복합 verify 커맨드에서 린터 출력("Found N errors" 등)이 점수에 섞이지 않도록.
    let line = out
        .lines()
        .rev()
        .find(|l| l.contains(" passed") || l.contains(" failed"))?;
    let passed = int_before(line, " passed");
    let failed = int_before(line, " failed");
    let errors = int_before(line, " error"); // "error"와 "errors" 모두 일치
    if passed.is_none() && failed.is_none() {
        return None;
    }
    let p = passed.unwrap_or(0);
    let total = p
        .saturating_add(failed.unwrap_or(0))
        .saturating_add(errors.unwrap_or(0));
    if total == 0 {
        return None;
    }
    Some(p as f64 / total as f64)
}

/// unittest: "Ran 4 tests in 0.0s" + "OK" | "FAILED (failures=1, errors=2)"
fn parse_unittest(out: &str) -> Option<f64> {
    let total = int_after(out, "Ran ")?;
    if total == 0 {
        return None;
    }
    let failures = int_after(out, "failures=").unwrap_or(0);
    let errors = int_after(out, "errors=").unwrap_or(0);
    let passed = total.saturating_sub(failures.saturating_add(errors));
    Some(passed as f64 / total as f64)
}

/// `key` 마지막 출현 직후의 정수 (예: "Ran 4" -> 4).
fn int_after(s: &str, key: &str) -> Option<u32> {
    let idx = s.rfind(key)? + key.len();
    let digits: String = s[idx..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// `key` 마지막 출현 직전의 정수 (예: "3 passed" -> 3).
fn int_before(s: &str, key: &str) -> Option<u32> {
    let idx = s.rfind(key)?;
    let head = s[..idx].trim_end();
    let mut digits: Vec<char> = head
        .chars()
        .rev()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.reverse();
    if digits.is_empty() {
        return None;
    }
    digits.iter().collect::<String>().parse().ok()
}
