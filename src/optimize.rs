//! The optimization loop: baseline -> (propose -> candidate eval -> gate)*.
//! This is the composition root: it builds the configured provider + judge
//! adapters and drives the generic eval. The gate keeps a proposal only if it
//! strictly beats the best score; nothing live is overwritten (staged only).

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
            continue; // never leak held-out tasks to the optimizer
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

/// Soft guard for auto-authored configs: a held-out test must not also be shown to
/// the agent. If a task's `verify_cmd` names one of its `context_files`, that file is
/// almost certainly the answer leaking in. Returns one message per suspected leak
/// (callers warn; never block — the user may know better). Pure, so it is unit-tested.
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

/// Strip a single wrapping ```...``` fence if the model added one.
fn strip_fences(s: &str) -> String {
    let t = s.trim();
    if let Some(rest) = t.strip_prefix("```") {
        let body = rest.split_once('\n').map(|x| x.1).unwrap_or("");
        return body.trim_end_matches("```").trim().to_string();
    }
    t.to_string()
}

/// CLI `skillsmith run`: optimize once, write artifacts, discard the returned data.
pub async fn run(home: &Path, project_name: &str, plain: bool) -> Result<()> {
    run_once(home, project_name, plain).await.map(|_| ())
}

/// One optimization run. Returns the machine-readable [`results::Results`] so `bench`
/// can aggregate it across seeds (the CLI `run` wrapper discards it). Writes
/// `results.json` / `report.md` / `skill.staged.md` as a side effect, same as before.
pub async fn run_once(home: &Path, project_name: &str, plain: bool) -> Result<results::Results> {
    let project = Project::load(home, project_name)?;
    for warn in answer_leak_warnings(&project) {
        eprintln!("warning: {warn}");
    }
    // Per-stage providers: the cheap agent (eval) stage and the optimizer (propose)
    // stage can use different CLI commands (e.g. a smaller model on the agent stage).
    // With no overrides both resolve to the base provider — unchanged behaviour.
    let agent_llm = build_stage_provider(&project.cfg, &project.cfg.agent_provider_cmd)?;
    let optimizer_llm = build_stage_provider(&project.cfg, &project.cfg.optimizer_provider_cmd)?;
    let judge = ExecJudge;
    let emitter = Emitter::new(plain);
    // Split: `test` tasks are held out of optimization entirely — evaluated ONCE at the
    // end on the best skill for an unbiased number. `train`+`val` drive the loop; the
    // gate scores on `val` (the `holdout` set below).
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
    // All train+val tasks are `val` = no training signal (a misconfig). Degrade to
    // train+gate-on-all instead of wasting rounds.
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
    // Per-task scores of the current best, for showing what moved each round.
    let mut best_by_id: HashMap<String, f64> = scores_by_id(&baseline);
    // Machine-readable accumulators for results.json (baseline rows + best rows + gate log).
    let baseline_tasks = task_results(&baseline, &holdout);
    let mut best_tasks = baseline_tasks.clone();
    let mut round_results: Vec<results::RoundResult> = Vec::new();
    let mut last_report = baseline;
    let mut rounds_log: Vec<(u32, f64, bool)> = Vec::new();

    for round in 1..=project.cfg.rounds {
        emitter.emit(&Event::Stage { label: format!("round {round} · propose") });
        let user = propose_user(&skill, &last_report, &holdout);
        // A transient provider error (e.g. `claude` exits 1) must NOT abort the run and
        // discard improvements already won — stop the loop and keep the best so far.
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
            best_score, // the score the candidate was gated against (pre-accept)
            accepted,
        });
        last_report = cand;
        if accepted {
            best_skill = proposal.clone();
            best_score = cand_score;
            best_by_id = scores_by_id(&last_report);
            best_tasks = task_results(&last_report, &holdout);
            skill = proposal; // continue refining from the improved skill
            // Persist immediately: an accepted improvement is durable from this point,
            // even if a later round hits a transient provider error and we break out.
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
    // Record what these inputs looked like so `skillsmith check` can later detect drift.
    let config_fp = stamp_drift(&project);

    // Held-out test: evaluate the best skill ONCE on the `test` split (never seen during
    // optimization) for an unbiased final number. Skipped when there is no test split.
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

    // Machine-readable results (the benchmark / sweep / CI seam) — alongside report.md.
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

/// `skillsmith bench`: run the optimization `seeds` times and aggregate into a
/// variance-aware scorecard + a `sweep.jsonl` ledger under `<project>/bench/`. The
/// agent LLM isn't seedable, so each seed is a fresh independent sample. Spends
/// `seeds ×` a normal run's tokens — explicit and opt-in.
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

/// Snapshot of each task's graded score, keyed by task id (for round deltas).
fn scores_by_id(report: &EvalReport) -> HashMap<String, f64> {
    report
        .outcomes
        .iter()
        .map(|o| (o.id.clone(), o.score))
        .collect()
}

/// Per-task outcomes as serializable rows for `results.json` (marks held-out tasks).
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

/// Per-task score movements from the prior best to this candidate (changed tasks
/// only). A task absent from the prior snapshot is treated as unchanged baseline.
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

/// Persist the current best skill (`skill.staged.md`) + `report.md`. Called after every
/// accepted round so a later transient provider failure can't discard an improvement,
/// and once more at the end. Returns the staged path and the rendered report.
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

/// Adopt the staged proposal: copy `skill.staged.md` over the live skill file. This is
/// the ONE place the live skill is overwritten, invoked explicitly by the user (or the
/// host agent after a HITL confirm) — never by the optimization loop.
pub fn adopt(home: &Path, project_name: &str) -> Result<()> {
    let project = Project::load(home, project_name)?;
    let (staged, live) = adopt_project(&project)?;
    println!("adopted: {} -> {}", staged.display(), live.display());
    Ok(())
}

/// Copy a project's staged proposal over its live skill file; returns (staged, live).
/// Errors if there is no staged proposal. The staged file is removed after adoption so a
/// stale proposal can't be silently re-adopted.
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

/// `--watch`: re-run the full optimize loop whenever an input file changes — a
/// FOREGROUND dev loop for iterating on the skill seed / config. Each pass is a real
/// run and spends tokens, so this is for active authoring with you at the keyboard,
/// NOT a background auto-optimizer. For a cheaper feedback loop use `eval --watch`
/// (re-eval only, no propose rounds). Runs until interrupted (Ctrl-C).
pub async fn run_watch(home: &Path, project_name: &str, plain: bool) -> Result<()> {
    watch_loop(home, project_name, plain, false).await
}

/// `eval --watch`: re-evaluate the current skill on every input change — the cheap
/// "did my hand-edit to the skill help?" loop (no optimization rounds).
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

/// Files whose change matters. `config.toml` + each task's context/target files in
/// the repo, plus the live skill **only when `include_skill`** (watch wants a skill
/// hand-edit to re-run; drift detection does NOT — adopting rewrites skill.md on
/// purpose, so counting it would flag every adopt as drift). Always EXCLUDES the
/// generated `skill.staged.md` / `report.md` so a run's own writes aren't inputs.
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

/// Hash `(path, len, mtime)` over a set of files. A missing file hashes as just its
/// path, so creating it later still flips the fingerprint.
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

/// Watch fingerprint — includes the live skill (a hand-edit should re-run).
fn input_fingerprint(home: &Path, project_name: &str) -> u64 {
    match Project::load(home, project_name) {
        Ok(p) => hash_paths(&fingerprint_paths(&p, true)),
        Err(_) => 0,
    }
}

/// Drift fingerprint — the repo inputs (config + target/context files) the skill was
/// optimized against, EXCLUDING skill.md. Used by `skillsmith check`.
fn drift_fingerprint(project: &Project) -> u64 {
    hash_paths(&fingerprint_paths(project, false))
}

/// Scratch file (per project) holding the drift fingerprint of the last `run`.
const LAST_RUN: &str = ".last-run";

/// Record the drift fingerprint of the inputs this run was evaluated against, so a
/// later `skillsmith check` can tell whether they've changed (token-0, no LLM).
/// Returns the fingerprint so the caller can reuse it (e.g. in `results.json`).
fn stamp_drift(project: &Project) -> u64 {
    let fp = drift_fingerprint(project);
    let _ = std::fs::write(project.dir.join(LAST_RUN), fp.to_string());
    fp
}

/// `skillsmith check`: token-0 **drift detection** — has the repo (config + target/
/// context files) changed since the last `run`, leaving the optimized skill possibly
/// stale? Compares the current inputs against the fingerprint stamped at the last run.
/// Exits non-zero on drift so a git pre-commit hook / CI can branch; it NEVER runs the
/// optimizer or touches the skill (detection only — re-running + adopting stay the
/// user's explicit call). This is the safe, frugal half of "auto-strengthen on change".
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

/// Block until an input file changes (1.5s polling — zero-dependency, robust across
/// editors/platforms, and responsive enough for a hand-edit loop).
async fn wait_for_input_change(home: &Path, project_name: &str) {
    let start = input_fingerprint(home, project_name);
    loop {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        if input_fingerprint(home, project_name) != start {
            return;
        }
    }
}

/// `skillsmith run --project <name> --dry-run`: validate the config and run every
/// task's verify_cmd in a worktree with NO agent edits and NO LLM — a free preflight
/// that catches a bad repo_path / verify_cmd / worktree setup before a real run.
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

/// Core of the dry run (generic over the judge so tests can drive it without an
/// LLM). Runs each verify_cmd on the UNMODIFIED repo (no edits); returns true if
/// every command executed without an internal error (a failing test is fine).
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

/// Scaffold a new project adapter (`skillsmith new <name>`). In `local` mode the
/// project lives in a repo-local `.skillsmith/` and `repo_path` is left blank.
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

/// Print discovered projects (`skillsmith list`).
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
        // Stable on a re-read with no change.
        assert_eq!(fp1, input_fingerprint(home.path(), "p"));
        // Editing the live skill (length changes) flips the fingerprint -> a re-run.
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

        // Editing the skill (the optimizer's OUTPUT): watch re-runs, drift must NOT
        // flag it — else every `adopt` (which rewrites skill.md) would read as drift.
        std::fs::write(project.skill_path(), "EDITED\n").unwrap();
        assert_ne!(watch0, input_fingerprint(home.path(), "p"), "watch tracks skill edits");
        assert_eq!(drift0, drift_fingerprint(&project), "drift ignores skill edits");

        // Editing config (an INPUT the skill was optimized against): drift notices.
        let cfg = project.dir.join("config.toml");
        let touched = std::fs::read_to_string(&cfg).unwrap() + "\n# touched\n";
        std::fs::write(&cfg, touched).unwrap();
        assert_ne!(drift0, drift_fingerprint(&project), "drift tracks config edits");
    }
}
