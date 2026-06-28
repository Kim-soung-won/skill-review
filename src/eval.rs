//! Run a full eval pass of a given skill — generic over the LLM and judge ports,
//! so the binary injects real adapters and tests inject mocks.

use crate::agent;
use crate::config::{Project, Task};
use crate::judge::{Judge, Outcome};
use crate::llm::LlmProvider;
use crate::obs::{Emitter, Event};
use anyhow::Result;
use std::path::Path;
use std::time::Instant;

pub struct EvalReport {
    pub outcomes: Vec<Outcome>,
}

impl EvalReport {
    /// Mean graded score across all tasks (continuous in [0,1]).
    pub fn score(&self) -> f64 {
        mean(self.outcomes.iter().map(|o| o.score))
    }

    /// Gate score: mean over held-out (val) tasks when any exist, else all tasks.
    /// This is what the optimizer's accept/reject gate compares — so improvements
    /// are credited only when they generalize to tasks the optimizer never saw.
    pub fn gate_score(&self, holdout: &std::collections::HashSet<String>) -> f64 {
        if holdout.is_empty() {
            return self.score();
        }
        let val = self
            .outcomes
            .iter()
            .filter(|o| holdout.contains(&o.id))
            .map(|o| o.score);
        let scored = self.outcomes.iter().any(|o| holdout.contains(&o.id));
        if scored { mean(val) } else { self.score() }
    }
}

fn mean(it: impl Iterator<Item = f64>) -> f64 {
    let (sum, n) = it.fold((0.0, 0u32), |(s, n), x| (s + x, n + 1));
    if n == 0 { 0.0 } else { sum / n as f64 }
}

fn read_ctx(repo: &Path, files: &[String]) -> Vec<(String, String)> {
    files
        .iter()
        .map(|f| {
            let content =
                std::fs::read_to_string(repo.join(f)).unwrap_or_else(|_| "(missing)".to_string());
            (f.clone(), content)
        })
        .collect()
}

pub async fn eval_skill<L: LlmProvider, J: Judge>(
    llm: &L,
    judge: &J,
    project: &Project,
    skill: &str,
    tasks: &[Task],
    emitter: &Emitter,
) -> Result<EvalReport> {
    let repo = project.repo()?;
    let mut outcomes = Vec::new();
    for task in tasks {
        let ctx = read_ctx(&repo, &task.context_files);
        let system = agent::agent_system(skill);
        let user = agent::agent_user(task, &ctx);
        let t = Instant::now();
        let response = llm
            .complete(&project.cfg.agent_model, &system, &user)
            .await?;
        emitter.note_call();
        let edits = agent::parse_edits(&response);
        let outcome = judge.run(&repo, task, &edits).await?;
        emitter.emit(&Event::Task {
            id: outcome.id.clone(),
            passed: outcome.passed,
            score: outcome.score,
            ms: t.elapsed().as_millis(),
        });
        outcomes.push(outcome);
    }
    Ok(EvalReport { outcomes })
}
