//! Grading ports + adapters.
//!
//! [`Judge`] is the port. [`ExecJudge`] applies the agent's edits in an isolated
//! `git worktree` and runs the task's verify command. The score is GRADED: the
//! fraction of individual test cases that pass (parsed from the runner output),
//! falling back to the binary exit code when no count is found — a continuous
//! signal the optimizer can actually climb. New graders plug in via [`Judge`].

use crate::agent::Edit;
use crate::config::Task;
use crate::worktree;
use anyhow::Result;
use std::path::Path;
use tokio::process::Command;

pub struct Outcome {
    pub id: String,
    /// Whole verify command exited 0 (every case green).
    pub passed: bool,
    /// Graded score in [0,1]: fraction of test cases passing (1.0 == passed).
    pub score: f64,
    /// Truncated verify output (fed back to the optimizer on failure).
    pub detail: String,
}

/// Port: grade an agent's edits for one task.
#[allow(async_fn_in_trait)]
pub trait Judge {
    async fn run(&self, repo: &Path, task: &Task, edits: &[Edit]) -> Result<Outcome>;
}

/// Default adapter: run the repo's own verify command on the applied edits.
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

/// Graded score in [0,1]: fraction of individual test cases passing, parsed from
/// the runner summary. Falls back to the binary exit code when no count is found
/// (so non-test verify commands still grade 1.0/0.0).
pub fn grade(code: i32, output: &str) -> f64 {
    if let Some(s) = parse_pytest(output).or_else(|| parse_unittest(output)) {
        return s;
    }
    if code == 0 { 1.0 } else { 0.0 }
}

/// pytest: "... 3 passed, 1 failed, 2 errors in 0.1s ..."
fn parse_pytest(out: &str) -> Option<f64> {
    // pytest's summary is a single line. Scan only the LAST line mentioning
    // passed/failed so unrelated tool output in a combined verify command (a
    // linter printing "Found N errors", etc.) can't leak counts into the score.
    let line = out
        .lines()
        .rev()
        .find(|l| l.contains(" passed") || l.contains(" failed"))?;
    let passed = int_before(line, " passed");
    let failed = int_before(line, " failed");
    let errors = int_before(line, " error"); // matches "error" and "errors"
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

/// Integer immediately following `key`'s last occurrence (e.g. "Ran 4" -> 4).
fn int_after(s: &str, key: &str) -> Option<u32> {
    let idx = s.rfind(key)? + key.len();
    let digits: String = s[idx..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// Integer immediately preceding `key`'s last occurrence (e.g. "3 passed" -> 3).
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
