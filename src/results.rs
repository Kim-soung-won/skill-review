//! Machine-readable run results (`results.json`) — the benchmark / automation seam.
//!
//! Prose `report.md` stays for humans; this is the parseable artifact a sweep,
//! a scorecard, a CI gate, or a variance study consumes. Field order is the
//! struct declaration order (serde serializes structs in declared order), so the
//! JSON is stable and diffable across runs. `schema_version` lets consumers
//! evolve safely. Written alongside `report.md` at the end of a `run`.

use serde::Serialize;

/// One run's complete, machine-readable outcome.
#[derive(Serialize)]
pub struct Results {
    /// Bumped on any breaking change to this shape.
    pub schema_version: u32,
    pub project: String,
    /// Unix seconds at write time (audit / ordering; no wall-clock formatting dep).
    pub timestamp_unix: u64,
    pub provider: String,
    pub agent_model: String,
    pub optimizer_model: String,
    pub rounds_configured: u32,
    pub holdout_count: usize,
    /// Hash of the inputs (config + target/context files) this run was graded
    /// against — ties a result to the repo state that produced it (same as `.last-run`).
    pub config_fingerprint: String,
    /// LLM calls made (skillsmith's cost unit; token counts aren't exposed by the
    /// CLI providers, so this is calls, not tokens).
    pub llm_calls: u32,
    pub elapsed_ms: u64,
    pub baseline_score: f64,
    pub best_score: f64,
    pub lift: f64,
    /// Mean score of the best skill on the held-out `test` split, evaluated ONCE at the
    /// end (never seen during optimization) — the unbiased number. `None` if no test split.
    pub test_score: Option<f64>,
    /// Per-task outcomes of the baseline (initial) skill.
    pub baseline: Vec<TaskResult>,
    /// Per-task outcomes of the accepted best skill (== baseline if no round was accepted).
    pub best: Vec<TaskResult>,
    /// Per-task outcomes on the held-out `test` split (empty if no test split).
    pub test: Vec<TaskResult>,
    /// Per-round gate decisions.
    pub rounds: Vec<RoundResult>,
    pub staged_path: String,
}

/// One task's graded outcome (gate score is the mean over the held-out subset).
#[derive(Serialize, Clone)]
pub struct TaskResult {
    pub id: String,
    pub passed: bool,
    /// Continuous in [0,1]: fraction of the task's test cases that passed.
    pub score: f64,
    /// Held out from training (gates on these when any exist).
    pub holdout: bool,
}

/// One optimization round's accept/reject decision.
#[derive(Serialize, Clone)]
pub struct RoundResult {
    pub round: u32,
    pub candidate_score: f64,
    /// The best score the candidate was gated against (strict `>` to accept).
    pub best_score: f64,
    pub accepted: bool,
}

/// Serialize to pretty JSON. Stable key order (struct field order).
pub fn to_json(r: &Results) -> serde_json::Result<String> {
    serde_json::to_string_pretty(r)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Results {
        Results {
            schema_version: 1,
            project: "demo".into(),
            timestamp_unix: 1_700_000_000,
            provider: "claude".into(),
            agent_model: "claude-sonnet-4-6".into(),
            optimizer_model: "claude-opus-4-8".into(),
            rounds_configured: 3,
            holdout_count: 1,
            config_fingerprint: "12345".into(),
            llm_calls: 7,
            elapsed_ms: 64_000,
            baseline_score: 0.5,
            best_score: 0.7,
            lift: 0.2,
            test_score: Some(0.65),
            baseline: vec![
                TaskResult { id: "a".into(), passed: true, score: 1.0, holdout: false },
                TaskResult { id: "b".into(), passed: false, score: 0.0, holdout: true },
            ],
            best: vec![
                TaskResult { id: "a".into(), passed: true, score: 1.0, holdout: false },
                TaskResult { id: "b".into(), passed: false, score: 0.4, holdout: true },
            ],
            test: vec![
                TaskResult { id: "c".into(), passed: true, score: 0.65, holdout: false },
            ],
            rounds: vec![RoundResult {
                round: 1,
                candidate_score: 0.7,
                best_score: 0.5,
                accepted: true,
            }],
            staged_path: "/p/skill.staged.md".into(),
        }
    }

    #[test]
    fn to_json_is_valid_and_carries_key_fields() {
        let s = to_json(&sample()).unwrap();
        // Round-trips as valid JSON.
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["project"], "demo");
        assert_eq!(v["lift"], 0.2);
        // Per-task breakdown survives (the benchmark-enabling part).
        assert_eq!(v["baseline"][1]["id"], "b");
        assert_eq!(v["best"][1]["score"], 0.4);
        assert_eq!(v["best"][1]["holdout"], true);
        assert_eq!(v["rounds"][0]["accepted"], true);
        assert_eq!(v["llm_calls"], 7);
    }

    #[test]
    fn field_order_is_stable() {
        // schema_version must be the first key (consumers can sniff version cheaply).
        let s = to_json(&sample()).unwrap();
        let first_key = s.find('"').map(|i| &s[i..]).unwrap();
        assert!(first_key.starts_with("\"schema_version\""), "schema_version leads");
    }
}
