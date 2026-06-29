//! 주어진 스킬에 대해 전체 eval 패스를 실행한다 — LLM과 judge 포트에 대해 제네릭으로 구현되어
//! 바이너리에서는 실제 어댑터를, 테스트에서는 목(mock)을 주입할 수 있다.

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
    /// 전체 태스크의 평균 점수 (연속값, 범위 [0,1]).
    pub fn score(&self) -> f64 {
        mean(self.outcomes.iter().map(|o| o.score))
    }

    /// 게이트 점수: held-out(val) 태스크가 있으면 그 평균, 없으면 전체 평균.
    /// 옵티마이저의 수락/거절 게이트가 비교하는 값 — 옵티마이저가 한 번도 본 적 없는
    /// 태스크에서도 개선이 일반화될 때만 점수로 인정된다.
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
