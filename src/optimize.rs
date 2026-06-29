//! 최적화 루프: baseline -> (propose -> candidate eval -> gate)*.
//! 컴포지션 루트: 설정된 프로바이더 + judge 어댑터를 생성하고
//! 제네릭 eval을 구동한다. 게이트는 제안을 최선 점수를 엄격히 초과할 때만 수락;
//! 라이브 파일은 덮어쓰지 않음 (staged만 기록).

use crate::config::{Project, Task, TaskSplit};
use crate::eval::{self, EvalReport};
use crate::judge::{ExecJudge, Judge};
use crate::llm::{LlmProvider, build_stage_provider};
use crate::obs::{Delta, Emitter, Event, line_diff};
use crate::report;
use crate::results;
use anyhow::{Result, bail};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

fn propose_system() -> &'static str {
    "You optimize a coding agent's SKILL (a markdown instruction sheet). You are given the \
current skill and the tasks that FAILED, with their verification output. Rewrite the skill so \
the agent succeeds next time by adding the missing project-specific rules the failures reveal. \
Keep it concise and general — encode durable conventions, not one-off answers. Output ONLY the \
new skill markdown: no code fences, no commentary."
}

fn propose_user(skill: &str, report: &EvalReport, holdout: &HashSet<String>) -> String {
    let mut s = String::new();
    s.push_str("=== CURRENT SKILL ===\n");
    s.push_str(skill);
    s.push_str("\n\n=== FAILED TRAINING TASKS (with verify output) ===\n");
    let mut any = false;
    for o in &report.outcomes {
        if holdout.contains(&o.id) {
            continue; // held-out 태스크를 옵티마이저에게 절대 노출하지 않음
        }
        if o.score < 1.0 {
            any = true;
            s.push_str(&format!(
                "- task `{}` (score {:.2}):\n{}\n\n",
                o.id, o.score, o.detail
            ));
        }
    }
    if !any {
        s.push_str("(no training task failed — propose a small clarity/robustness improvement)\n");
    }
    s
}

/// 자동 작성 설정의 소프트 가드: held-out 테스트가 에이전트에게 보여지면 안 됨.
/// 태스크의 `verify_cmd`가 `context_files` 중 하나를 참조하면 답이 노출될 가능성이 높음.
/// 의심되는 누출당 메시지 하나를 반환 (호출자가 경고; 블로킹 없음 — 사용자가 더 잘 알 수도 있음).
/// 순수 함수이므로 단위 테스트 가능.
pub fn answer_leak_warnings(project: &Project) -> Vec<String> {
    let mut out = Vec::new();
    for task in &project.cfg.tasks {
        for cf in &task.context_files {
            if !cf.is_empty() && task.verify_cmd.contains(cf.as_str()) {
                out.push(format!(
                    "task `{}`: context_file `{}` is referenced by verify_cmd — the held-out \
test may be leaking into the agent's context (remove it from context_files)",
                    task.id, cf
                ));
            }
        }
    }
    out
}

/// 모델이 추가했을 경우 단일 래핑 ```...``` 펜스를 제거.
fn strip_fences(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let body = rest.split_once('\n').map(|x| x.1).unwrap_or("");
        return body.trim_end_matches("```").trim().to_string();
    }
    t.to_string()
}

/// CLI `skillsmith run`: 한 번 최적화, 아티팩트 기록, 반환 데이터 버림.
pub async fn run(home: &Path, project_name: &str, plain: bool) -> Result<()> {
    run_once(home, project_name, plain).await.map(|_| ())
}

/// 최적화 한 번 실행. `bench`가 시드 간 집계할 수 있도록 머신 리더블 [`results::Results`] 반환.
/// CLI `run` 래퍼는 이를 버림. `results.json` / `report.md` / `skill.staged.md`를 부작용으로 기록.
pub async fn run_once(home: &Path, project_name: &str, plain: bool) -> Result<results::Results> {
    let project = Project::load(home, project_name)?;
    for warn in answer_leak_warnings(&project) {
        eprintln!("warning: {warn}");
    }
    // 단계별 프로바이더: 저렴한 에이전트(eval) 단계와 옵티마이저(propose) 단계가
    // 서로 다른 CLI 커맨드 사용 가능 (예: 에이전트 단계에 더 작은 모델).
    // 오버라이드 없으면 둘 다 기본 프로바이더로 해석 — 기존 동작 유지.
    let agent_llm = build_stage_provider(&project.cfg, &project.cfg.agent_provider_cmd)?;
    let optimizer_llm = build_stage_provider(&project.cfg, &project.cfg.optimizer_provider_cmd)?;
    let judge = ExecJudge;
    let emitter = Emitter::new(plain);
    // 스플릿: `test` 태스크는 최적화에서 완전히 제외 — 편향 없는 최종 수치를 위해
    // 마지막에 최선 스킬로 단 한 번 평가. `train`+`val`이 루프를 구동;
    // 게이트는 `val`(아래 `holdout` 집합)로 점수를 냄.
    let test_tasks: Vec<Task> = project
        .cfg
        .tasks
        .iter()
        .filter(|t| t.split() == TaskSplit::Test)
        .cloned()
        .collect();
    let train_val: Vec<Task> = project
        .cfg
        .tasks
        .iter()
        .filter(|t| t.split() != TaskSplit::Test)
        .cloned()
        .collect();
    let mut holdout: HashSet<String> = train_val
        .iter()
        .filter(|t| t.split() == TaskSplit::Val)
        .map(|t| t.id.clone())
        .collect();
    // 모든 train+val 태스크가 `val` = 학습 신호 없음 (잘못된 설정). 라운드를 낭비하는 대신
    // train+gate-on-all로 대체.
    if !holdout.is_empty() && holdout.len() == train_val.len() {
        eprintln!(
            "warning: every train/val task is `val` — no training signal; treating all as train+val"
        );
        holdout.clear();
    }
    emitter.emit(&Event::RunStart {
        project: project.cfg.name.clone(),
        provider: project.cfg.provider.clone(),
        agent_model: project.cfg.agent_model.clone(),
        optimizer_model: project.cfg.optimizer_model.clone(),
        tasks: project.cfg.tasks.len(),
        rounds: project.cfg.rounds,
        holdout: holdout.len(),
    });

    let mut skill = std::fs::read_to_string(project.skill_path())?;
    emitter.emit(&Event::Stage { label: "baseline eval".into() });
    let t_base = Instant::now();
    let baseline = eval::eval_skill(&agent_llm, &judge, &project, &skill, &train_val, &emitter).await?;
    let baseline_score = baseline.gate_score(&holdout);
    emitter.emit(&Event::Baseline {
        score: baseline_score,
        ms: t_base.elapsed().as_millis(),
    });

    let mut best_skill = skill.clone();
    let mut best_score = baseline_score;
    // 현재 최선의 태스크별 점수 (각 라운드에서 무엇이 변했는지 표시용).
    let mut best_by_id: HashMap<String, f64> = scores_by_id(&baseline);
    // results.json용 머신 리더블 누산기 (baseline/best 행 + 게이트 로그).
    let baseline_tasks = task_results(&baseline, &holdout);
    let mut best_tasks = baseline_tasks.clone();
    let mut round_results: Vec<results::RoundResult> = Vec::new();
    let mut last_report = baseline;
    let mut rounds_log: Vec<(u32, f64, bool)> = Vec::new();

    for round in 1..=project.cfg.rounds {
        emitter.emit(&Event::Stage { label: format!("round {round} · propose") });
        let user = propose_user(&skill, &last_report, &holdout);
        // 일시적 프로바이더 오류(예: `claude` exit 1)가 실행을 중단하고
        // 이미 얻은 개선을 버리면 안 됨 — 루프를 중단하고 지금까지의 최선을 유지.
        let raw = match optimizer_llm
            .complete(&project.cfg.optimizer_model, propose_system(), &user)
            .await
        {
            Ok(r) => r,
            Err(e) => {
                eprintln!("round {round}: propose failed ({e}) — stopping with best so far");
                break;
            }
        };
        emitter.note_call();
        let proposal = strip_fences(&raw);

        emitter.emit(&Event::Stage { label: format!("round {round} · candidate eval") });
        let cand = match eval::eval_skill(&agent_llm, &judge, &project, &proposal, &train_val, &emitter).await {
            Ok(c) => c,
            Err(e) => {
                eprintln!("round {round}: candidate eval failed ({e}) — stopping with best so far");
                break;
            }
        };
        let cand_score = cand.gate_score(&holdout);
        let accepted = cand_score > best_score;
        let (skill_add, skill_del) = line_diff(&best_skill, &proposal);
        emitter.emit(&Event::Round {
            round,
            cand: cand_score,
            best: best_score,
            accepted,
            deltas: deltas_vs(&best_by_id, &cand),
            skill_add,
            skill_del,
        });

        rounds_log.push((round, cand_score, accepted));
        round_results.push(results::RoundResult {
            round,
            candidate_score: cand_score,
            best_score, // 후보가 게이트된 점수 (수락 전)
            accepted,
        });
        last_report = cand;
        if accepted {
            best_skill = proposal.clone();
            best_score = cand_score;
            best_by_id = scores_by_id(&last_report);
            best_tasks = task_results(&last_report, &holdout);
            skill = proposal; // 개선된 스킬에서 계속 정제
            // 즉시 저장: 수락된 개선은 이 시점부터 영구적,
            // 이후 라운드에서 일시적 프로바이더 오류로 break해도 사라지지 않음.
            let (staged, _) = stage_best(
                &project,
                project_name,
                &best_skill,
                baseline_score,
                best_score,
                &rounds_log,
                holdout.len(),
            )?;
            emitter.emit(&Event::Staged {
                path: staged.display().to_string(),
                round: Some(round),
            });
        }
    }

    let (staged, _rep) = stage_best(
        &project,
        project_name,
        &best_skill,
        baseline_score,
        best_score,
        &rounds_log,
        holdout.len(),
    )?;
    // 이 실행이 평가된 입력의 드리프트 지문 기록 — 이후 `skillsmith check`가 변경 여부 감지 가능 (토큰 0).
    let config_fp = stamp_drift(&project);

    // Held-out 테스트: 최적화 중 한 번도 보지 않은 `test` 스플릿에 대해 최선 스킬을
    // 단 한 번 평가 (편향 없는 최종 수치). test 스플릿이 없으면 건너뜀.
    let mut test_results: Vec<results::TaskResult> = Vec::new();
    let test_score = if test_tasks.is_empty() {
        None
    } else {
        emitter.emit(&Event::Stage { label: "held-out test eval".into() });
        match eval::eval_skill(&agent_llm, &judge, &project, &best_skill, &test_tasks, &emitter).await {
            Ok(rep) => {
                test_results = task_results(&rep, &holdout);
                Some(rep.score())
            }
            Err(e) => {
                eprintln!("test eval failed ({e}) — skipping the test metric");
                None
            }
        }
    };

    // 머신 리더블 결과 (벤치마크 / 스윕 / CI 심) — report.md와 함께 저장.
    let timestamp_unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let results = results::Results {
        schema_version: 1,
        project: project.cfg.name.clone(),
        timestamp_unix,
        provider: project.cfg.provider.clone(),
        agent_model: project.cfg.agent_model.clone(),
        optimizer_model: project.cfg.optimizer_model.clone(),
        rounds_configured: project.cfg.rounds,
        holdout_count: holdout.len(),
        config_fingerprint: config_fp.to_string(),
        llm_calls: emitter.calls(),
        elapsed_ms: emitter.elapsed_ms() as u64,
        baseline_score,
        best_score,
        lift: best_score - baseline_score,
        test_score,
        baseline: baseline_tasks,
        best: best_tasks,
        test: test_results,
        rounds: round_results,
        staged_path: staged.display().to_string(),
    };
    if let Ok(json) = results::to_json(&results) {
        let _ = std::fs::write(project.dir.join("results.json"), json);
    }
    let adopt_cmd = (best_score > baseline_score)
        .then(|| format!("skillsmith adopt --project {project_name}"));
    emitter.emit(&Event::RunEnd {
        baseline: baseline_score,
        best: best_score,
        test_score,
        staged: staged.display().to_string(),
        adopt_cmd,
        calls: emitter.calls(),
        ms: emitter.elapsed_ms(),
    });
    Ok(results)
}

/// `skillsmith bench`: 최적화를 `seeds`회 실행하고 분산 인식 스코어카드 +
/// `sweep.jsonl` 원장을 `<project>/bench/`에 집계. 에이전트 LLM은 시드 고정 불가,
/// 각 시드는 독립된 새 샘플. `seeds ×` 일반 실행 토큰 소비 — 명시적이고 옵트인.
pub async fn bench(home: &Path, project_name: &str, seeds: u32, plain: bool) -> Result<()> {
    let project = Project::load(home, project_name)?;
    let seeds = seeds.max(1);
    let mut runs: Vec<results::Results> = Vec::new();
    for seed in 1..=seeds {
        eprintln!("\n══ bench seed {seed}/{seeds} ══");
        match run_once(home, project_name, plain).await {
            Ok(r) => runs.push(r),
            Err(e) => eprintln!("seed {seed} failed: {e:#}"),
        }
    }
    if runs.is_empty() {
        bail!("no successful bench runs for `{project_name}`");
    }
    let dir = project.dir.join("bench");
    std::fs::create_dir_all(&dir)?;
    if let Ok(jsonl) = crate::bench::sweep_jsonl(&runs) {
        let _ = std::fs::write(dir.join("sweep.jsonl"), jsonl);
    }
    let card = crate::bench::scorecard(project_name, &runs);
    std::fs::write(dir.join("scorecard.md"), &card)?;
    println!("\n{card}\nbench artifacts -> {}", dir.display());
    Ok(())
}

/// 태스크별 채점 점수 스냅샷, 태스크 id 키 (라운드 델타용).
fn scores_by_id(report: &EvalReport) -> HashMap<String, f64> {
    report
        .outcomes
        .iter()
        .map(|o| (o.id.clone(), o.score))
        .collect()
}

/// `results.json`용 직렬화 가능 행으로 태스크별 결과 반환 (held-out 태스크 표시).
fn task_results(report: &EvalReport, holdout: &HashSet<String>) -> Vec<results::TaskResult> {
    report
        .outcomes
        .iter()
        .map(|o| results::TaskResult {
            id: o.id.clone(),
            passed: o.passed,
            score: o.score,
            holdout: holdout.contains(&o.id),
        })
        .collect()
}

/// 이전 최선에서 현재 후보로의 태스크별 점수 변화 (변경된 태스크만).
/// 이전 스냅샷에 없는 태스크는 변경 없음으로 처리.
fn deltas_vs(prev: &HashMap<String, f64>, cand: &EvalReport) -> Vec<Delta> {
    cand.outcomes
        .iter()
        .filter_map(|o| {
            let from = *prev.get(&o.id).unwrap_or(&o.score);
            ((from - o.score).abs() > 1e-9).then(|| Delta {
                id: o.id.clone(),
                from,
                to: o.score,
            })
        })
        .collect()
}

/// 현재 최선 스킬(`skill.staged.md`) + `report.md` 저장. 수락된 라운드마다 호출해
/// 이후 일시적 프로바이더 오류로 개선이 사라지지 않도록 하고, 종료 시 한 번 더 호출.
/// staged 경로와 렌더링된 리포트를 반환.
fn stage_best(
    project: &Project,
    project_name: &str,
    best_skill: &str,
    baseline_score: f64,
    best_score: f64,
    rounds_log: &[(u32, f64, bool)],
    holdout_len: usize,
) -> Result<(PathBuf, String)> {
    let staged = project.dir.join("skill.staged.md");
    std::fs::write(&staged, best_skill)?;
    let rep = report::render(project_name, baseline_score, best_score, rounds_log, holdout_len);
    std::fs::write(project.dir.join("report.md"), &rep)?;
    Ok((staged, rep))
}

/// 스테이징된 제안 채택: `skill.staged.md`를 라이브 스킬 파일 위에 복사한다.
/// 라이브 스킬이 덮어써지는 유일한 지점이며, 사용자(또는 HITL 확인 후 호스트 에이전트)가
/// 명시적으로 호출 — 최적화 루프가 절대 호출하지 않음.
pub fn adopt(home: &Path, project_name: &str) -> Result<()> {
    let project = Project::load(home, project_name)?;
    let (staged, live) = adopt_project(&project)?;
    println!("adopted: {} -> {}", staged.display(), live.display());
    Ok(())
}

/// 프로젝트의 스테이징된 제안을 라이브 스킬 파일 위에 복사; (staged, live) 반환.
/// 스테이징된 제안이 없으면 오류. 채택 후 staged 파일을 삭제해 오래된 제안이
/// 묵묵히 재채택되지 않도록 함.
pub fn adopt_project(project: &Project) -> Result<(PathBuf, PathBuf)> {
    let staged = project.dir.join("skill.staged.md");
    if !staged.exists() {
        bail!(
            "no staged proposal at {} — run `skillsmith run --project {}` first",
            staged.display(),
            project.cfg.name
        );
    }
    let live = project.skill_path();
    let content = std::fs::read_to_string(&staged)?;
    std::fs::write(&live, &content)?;
    std::fs::remove_file(&staged).ok();
    Ok((staged, live))
}

pub async fn eval_only(home: &Path, project_name: &str, plain: bool) -> Result<()> {
    let project = Project::load(home, project_name)?;
    let llm = build_stage_provider(&project.cfg, &project.cfg.agent_provider_cmd)?;
    let judge = ExecJudge;
    let emitter = Emitter::new(plain);
    let skill = std::fs::read_to_string(project.skill_path())?;
    emitter.emit(&Event::Stage {
        label: format!("eval · provider {}", project.cfg.provider),
    });
    let r = eval::eval_skill(&llm, &judge, &project, &skill, &project.cfg.tasks, &emitter).await?;
    println!("score: {:.3}", r.score());
    Ok(())
}

/// `--watch`: 입력 파일이 변경될 때마다 전체 최적화 루프 재실행 —
/// 스킬 시드 / 설정 반복 작업을 위한 포그라운드 개발 루프.
/// 각 패스가 실제 실행이고 토큰을 소비하므로 키보드 앞에서 능동적으로 작성할 때 사용,
/// 백그라운드 자동 옵티마이저가 아님. 저렴한 피드백 루프는 `eval --watch` 사용
/// (재eval만, propose 라운드 없음). Ctrl-C로 중단할 때까지 실행.
pub async fn run_watch(home: &Path, project_name: &str, plain: bool) -> Result<()> {
    watch_loop(home, project_name, plain, false).await
}

/// `eval --watch`: 입력 변경 시마다 현재 스킬을 재평가 —
/// "내 스킬 수정이 도움이 됐나?" 저렴한 루프 (최적화 라운드 없음).
pub async fn eval_watch(home: &Path, project_name: &str, plain: bool) -> Result<()> {
    watch_loop(home, project_name, plain, true).await
}

async fn watch_loop(home: &Path, project_name: &str, plain: bool, eval_mode: bool) -> Result<()> {
    loop {
        let pass = if eval_mode {
            eval_only(home, project_name, plain).await
        } else {
            run(home, project_name, plain).await
        };
        if let Err(e) = pass {
            eprintln!("error: {e:#}");
        }
        eprintln!(
            "\n⌚ watching skill.md · config.toml · target files — edit to re-run (Ctrl-C to stop)"
        );
        wait_for_input_change(home, project_name).await;
    }
}

/// 감시 대상 파일들. `config.toml` + 레포 내 각 태스크의 context/target 파일,
/// `include_skill`일 때만 라이브 스킬 포함 (watch는 스킬 수정 시 재실행;
/// 드리프트 감지는 포함하지 않음 — `adopt`가 skill.md를 의도적으로 재작성하므로
/// 포함하면 모든 adopt가 드리프트로 잘못 감지됨).
/// 실행 자체가 생성하는 `skill.staged.md` / `report.md`는 항상 제외.
fn fingerprint_paths(project: &Project, include_skill: bool) -> Vec<PathBuf> {
    let mut paths = vec![project.dir.join("config.toml")];
    if include_skill {
        paths.push(project.skill_path());
    }
    if let Ok(repo) = project.repo() {
        for t in &project.cfg.tasks {
            for f in t.context_files.iter().chain(t.target_files.iter()) {
                paths.push(repo.join(f));
            }
        }
    }
    paths
}

/// 파일 집합에 대해 `(경로, 크기, mtime)` 해시. 없는 파일은 경로만 해시하므로
/// 나중에 생성돼도 지문이 바뀜.
fn hash_paths(paths: &[PathBuf]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    for p in paths {
        p.hash(&mut h);
        if let Ok(meta) = std::fs::metadata(p) {
            meta.len().hash(&mut h);
            if let Ok(mtime) = meta.modified()
                && let Ok(d) = mtime.duration_since(std::time::UNIX_EPOCH)
            {
                d.as_nanos().hash(&mut h);
            }
        }
    }
    h.finish()
}

/// Watch 지문 — 라이브 스킬 포함 (직접 수정 시 재실행).
fn input_fingerprint(home: &Path, project_name: &str) -> u64 {
    match Project::load(home, project_name) {
        Ok(p) => hash_paths(&fingerprint_paths(&p, true)),
        Err(_) => 0,
    }
}

/// 드리프트 지문 — 스킬이 최적화된 레포 입력(config + target/context 파일),
/// skill.md 제외. `skillsmith check`에서 사용.
fn drift_fingerprint(project: &Project) -> u64 {
    hash_paths(&fingerprint_paths(project, false))
}

/// 드리프트 지문을 저장하는 프로젝트별 스크래치 파일.
const LAST_RUN: &str = ".last-run";

/// 이 실행이 평가된 입력의 드리프트 지문 기록 — 이후 `skillsmith check`가 변경 여부
/// 감지 가능 (토큰 0). 호출자가 재사용할 수 있도록 지문 반환 (예: `results.json`).
fn stamp_drift(project: &Project) -> u64 {
    let fp = drift_fingerprint(project);
    let _ = std::fs::write(project.dir.join(LAST_RUN), fp.to_string());
    fp
}

/// `skillsmith check`: 토큰 0 **드리프트 감지** — 마지막 `run` 이후 레포
/// (config + target/context 파일)가 변경되어 최적화된 스킬이 오래됐는가?
/// 마지막 실행 시 기록된 지문과 현재 입력을 비교.
/// git pre-commit 훅 / CI가 분기할 수 있도록 드리프트 시 non-zero 종료;
/// 절대 옵티마이저를 실행하거나 스킬을 수정하지 않음 (감지만 — 재실행+채택은 사용자의 명시적 결정).
pub fn check(home: &Path, project_name: &str) -> Result<()> {
    let project = Project::load(home, project_name)?;
    let current = drift_fingerprint(&project);
    let stored = std::fs::read_to_string(project.dir.join(LAST_RUN))
        .ok()
        .and_then(|s| s.trim().parse::<u64>().ok());
    match stored {
        None => {
            println!(
                "skillsmith: no prior run for `{project_name}` — run `skillsmith run --project {project_name}` first"
            );
            Ok(())
        }
        Some(s) if s == current => {
            println!("skillsmith: `{project_name}` is current (inputs unchanged since last run)");
            Ok(())
        }
        Some(_) => {
            eprintln!(
                "skillsmith: ⚠ `{project_name}` inputs changed since last run — the optimized \
skill may be stale; consider `skillsmith run --project {project_name}`"
            );
            std::process::exit(1);
        }
    }
}

/// 입력 파일이 변경될 때까지 블로킹 (1.5초 폴링 — 의존성 없음, 에디터/플랫폼 무관,
/// 직접 수정 루프에 충분히 빠름).
async fn wait_for_input_change(home: &Path, project_name: &str) {
    let start = input_fingerprint(home, project_name);
    loop {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        if input_fingerprint(home, project_name) != start {
            return;
        }
    }
}

/// `skillsmith run --project <name> --dry-run`: 설정을 검증하고 에이전트 편집 없이,
/// LLM 없이 모든 태스크의 verify_cmd를 worktree에서 실행 — 실제 실행 전에
/// repo_path / verify_cmd / worktree 설정이 올바른지 확인하는 무료 프리플라이트.
pub async fn dry_run(home: &Path, project_name: &str) -> Result<()> {
    let project = Project::load(home, project_name)?;
    for warn in answer_leak_warnings(&project) {
        eprintln!("warning: {warn}");
    }
    let repo = project.repo()?;
    println!(
        "dry-run: {} — {} task(s), repo {} (no LLM, no staging)\n",
        project.cfg.name,
        project.cfg.tasks.len(),
        repo.display()
    );
    let ok = dry_run_project(&ExecJudge, &project).await;
    println!();
    if ok {
        println!(
            "dry-run OK — config loads, repo worktrees, every verify_cmd runs. Safe to `run`."
        );
    } else {
        println!("dry-run FAILED — fix the config / verify_cmd above before `run`.");
    }
    Ok(())
}

/// dry run의 코어 (judge를 제네릭으로 받아 LLM 없이 테스트 가능).
/// 수정 없이 원본 레포에서 각 verify_cmd를 실행; 모든 커맨드가 내부 오류 없이
/// 실행되면 true 반환 (테스트 실패는 괜찮음).
pub async fn dry_run_project<J: Judge>(judge: &J, project: &Project) -> bool {
    let repo = match project.repo() {
        Ok(r) => r,
        Err(e) => {
            println!("  [ERR] repo_path — {e:#}");
            return false;
        }
    };
    let mut all_ran = true;
    for task in &project.cfg.tasks {
        match judge.run(&repo, task, &[]).await {
            Ok(o) => println!(
                "  [ok]  {} — verify_cmd ran (score {:.2} with no edits)",
                task.id, o.score
            ),
            Err(e) => {
                all_ran = false;
                println!("  [ERR] {} — {e:#}", task.id);
            }
        }
    }
    all_ran
}

/// 새 프로젝트 어댑터 생성 (`skillsmith new <name>`). `local` 모드에서는
/// 프로젝트가 레포 로컬 `.skillsmith/`에 위치하고 `repo_path`는 비워둠.
pub fn new_project(home: &Path, name: &str, repo: Option<&str>, local: bool) -> Result<()> {
    let dir = crate::config::scaffold_project(home, name, repo, local)?;
    println!("created {}", dir.display());
    let step1 = if local && repo.is_none() {
        "(add eval tasks — repo_path already defaults to this repo)"
    } else {
        "(set repo_path + add eval tasks)"
    };
    println!("  1. edit {}/config.toml   {step1}", dir.display());
    println!(
        "  2. edit {}/skill.md       (seed conventions)",
        dir.display()
    );
    println!("  3. skillsmith run --project {name}");
    if local {
        println!("     (run it from anywhere inside the repo — .skillsmith/ is auto-discovered)");
    }
    Ok(())
}

/// 발견된 프로젝트 출력 (`skillsmith list`).
pub fn list(home: &Path) -> Result<()> {
    let projects = crate::config::list_projects(home)?;
    let base = home.join("projects");
    if projects.is_empty() {
        println!("no projects found under {}", base.display());
        return Ok(());
    }
    println!("{} project(s) under {}:", projects.len(), base.display());
    for p in projects {
        println!("  {:<22} {} task(s)  -> {}", p.name, p.tasks, p.repo);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn input_fingerprint_changes_when_skill_edited() {
        let home = tempfile::tempdir().unwrap();
        crate::config::scaffold_project(home.path(), "p", None, true).unwrap();
        let fp1 = input_fingerprint(home.path(), "p");
        // 변경 없이 재읽으면 안정적.
        assert_eq!(fp1, input_fingerprint(home.path(), "p"));
        // 라이브 스킬 편집(길이 변경)이 지문을 바꿈 -> 재실행.
        let skill = home.path().join("projects").join("p").join("skill.md");
        let mut body = std::fs::read_to_string(&skill).unwrap();
        body.push_str("\nNEW RULE\n");
        std::fs::write(&skill, body).unwrap();
        assert_ne!(fp1, input_fingerprint(home.path(), "p"));
    }

    #[test]
    fn input_fingerprint_unknown_project_is_zero() {
        let home = tempfile::tempdir().unwrap();
        assert_eq!(input_fingerprint(home.path(), "nope"), 0);
    }

    #[test]
    fn drift_fingerprint_excludes_skill_unlike_watch() {
        let home = tempfile::tempdir().unwrap();
        crate::config::scaffold_project(home.path(), "p", None, true).unwrap();
        let project = Project::load(home.path(), "p").unwrap();
        let drift0 = drift_fingerprint(&project);
        let watch0 = input_fingerprint(home.path(), "p");

        // 스킬 편집(옵티마이저의 출력): watch는 재실행, drift는 감지하면 안 됨 —
        // 그렇지 않으면 skill.md를 재작성하는 모든 `adopt`가 드리프트로 읽힘.
        std::fs::write(project.skill_path(), "EDITED\n").unwrap();
        assert_ne!(watch0, input_fingerprint(home.path(), "p"), "watch tracks skill edits");
        assert_eq!(drift0, drift_fingerprint(&project), "drift ignores skill edits");

        // config 편집(스킬이 최적화된 입력): drift가 감지함.
        let cfg = project.dir.join("config.toml");
        let touched = std::fs::read_to_string(&cfg).unwrap() + "\n# touched\n";
        std::fs::write(&cfg, touched).unwrap();
        assert_ne!(drift0, drift_fingerprint(&project), "drift tracks config edits");
    }
}
