//! Benchmark aggregation — turn k independent runs into a variance-aware scorecard.
//!
//! `skillsmith bench` runs the optimization k times and feeds the per-run
//! [`Results`] here. The agent LLM isn't seedable, so each "seed" is a fresh
//! independent sample; the variance across them is the signal — a strict `>` gate
//! on a single sample can't tell noise from a real improvement, but k samples plus
//! a standard deviation can. Pure functions (stats + rendering), so they're tested.

use crate::results::Results;

/// Summary statistics over one metric across runs. Sample stddev (divides by n-1),
/// so a single run reports 0 spread rather than a misleading population value.
pub struct Stat {
    pub mean: f64,
    pub stddev: f64,
    pub min: f64,
    pub max: f64,
    pub n: usize,
}

/// Mean / sample-stddev / min / max over the values (empty -> all zero, n = 0).
pub fn stat(values: &[f64]) -> Stat {
    let n = values.len();
    if n == 0 {
        return Stat { mean: 0.0, stddev: 0.0, min: 0.0, max: 0.0, n: 0 };
    }
    let mean = values.iter().sum::<f64>() / n as f64;
    let stddev = if n > 1 {
        (values.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / (n as f64 - 1.0)).sqrt()
    } else {
        0.0
    };
    Stat {
        mean,
        stddev,
        min: values.iter().cloned().fold(f64::INFINITY, f64::min),
        max: values.iter().cloned().fold(f64::NEG_INFINITY, f64::max),
        n,
    }
}

/// One JSON object per run, newline-delimited — a sweep ledger for downstream tools.
pub fn sweep_jsonl(runs: &[Results]) -> serde_json::Result<String> {
    let mut s = String::new();
    for r in runs {
        s.push_str(&serde_json::to_string(r)?);
        s.push('\n');
    }
    Ok(s)
}

/// Human-readable markdown scorecard: per-seed rows + aggregate (mean ± stddev).
pub fn scorecard(project: &str, runs: &[Results]) -> String {
    let baseline = stat(&runs.iter().map(|r| r.baseline_score).collect::<Vec<_>>());
    let best = stat(&runs.iter().map(|r| r.best_score).collect::<Vec<_>>());
    let lift = stat(&runs.iter().map(|r| r.lift).collect::<Vec<_>>());
    let test = stat(&runs.iter().filter_map(|r| r.test_score).collect::<Vec<_>>());

    let mut s = String::new();
    s.push_str(&format!("# skillsmith bench — {project}\n\n"));
    s.push_str(&format!(
        "{} seed(s) — independent samples (the agent LLM isn't seedable; each is a fresh run).\n\n",
        runs.len()
    ));
    s.push_str("## Per-seed\n\n");
    s.push_str("| seed | baseline | best | lift | test | calls |\n");
    s.push_str("|---|---|---|---|---|---|\n");
    for (i, r) in runs.iter().enumerate() {
        let test_cell = r
            .test_score
            .map(|v| format!("{v:.3}"))
            .unwrap_or_else(|| "—".into());
        s.push_str(&format!(
            "| {} | {:.3} | {:.3} | {:+.3} | {} | {} |\n",
            i + 1,
            r.baseline_score,
            r.best_score,
            r.lift,
            test_cell,
            r.llm_calls,
        ));
    }
    s.push_str("\n## Aggregate (mean ± sample stddev)\n\n");
    s.push_str("| metric | mean | stddev | min | max |\n");
    s.push_str("|---|---|---|---|---|\n");
    let row = |name: &str, st: &Stat| {
        if st.n == 0 {
            format!("| {name} | — | — | — | — |\n")
        } else {
            format!(
                "| {name} | {:.3} | {:.3} | {:.3} | {:.3} |\n",
                st.mean, st.stddev, st.min, st.max
            )
        }
    };
    s.push_str(&row("baseline", &baseline));
    s.push_str(&row("best", &best));
    s.push_str(&row("lift", &lift));
    s.push_str(&row("test", &test));
    s
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::results::Results;

    fn res(baseline: f64, best: f64, test: Option<f64>, calls: u32) -> Results {
        Results {
            schema_version: 1,
            project: "demo".into(),
            timestamp_unix: 0,
            provider: "claude".into(),
            agent_model: "a".into(),
            optimizer_model: "o".into(),
            rounds_configured: 3,
            holdout_count: 1,
            config_fingerprint: "0".into(),
            llm_calls: calls,
            elapsed_ms: 1000,
            baseline_score: baseline,
            best_score: best,
            lift: best - baseline,
            test_score: test,
            baseline: vec![],
            best: vec![],
            test: vec![],
            rounds: vec![],
            staged_path: "/p".into(),
        }
    }

    #[test]
    fn stat_mean_and_sample_stddev() {
        let s = stat(&[0.6, 0.7, 0.8]);
        assert!((s.mean - 0.7).abs() < 1e-9);
        // sample stddev = sqrt((0.01 + 0.0 + 0.01) / (3-1)) = 0.1
        assert!((s.stddev - 0.1).abs() < 1e-9);
        assert_eq!(s.n, 3);
        assert!((s.min - 0.6).abs() < 1e-9 && (s.max - 0.8).abs() < 1e-9);
    }

    #[test]
    fn stat_single_sample_has_zero_stddev() {
        let s = stat(&[0.5]);
        assert_eq!(s.stddev, 0.0);
        assert_eq!(s.n, 1);
    }

    #[test]
    fn stat_empty_is_zeroed() {
        let s = stat(&[]);
        assert_eq!(s.n, 0);
    }

    #[test]
    fn scorecard_and_jsonl_render() {
        let runs = vec![res(0.5, 0.6, Some(0.6), 8), res(0.5, 0.8, Some(0.7), 9)];
        let card = scorecard("demo", &runs);
        assert!(card.contains("# skillsmith bench — demo"));
        assert!(card.contains("2 seed(s)"));
        assert!(card.contains("## Aggregate"));
        assert!(card.contains("| best | 0.700 |"), "best mean across seeds");
        // sweep ledger has one line per run.
        let jsonl = sweep_jsonl(&runs).unwrap();
        assert_eq!(jsonl.lines().count(), 2);
        assert!(jsonl.lines().all(|l| l.contains("\"best_score\"")));
    }

    #[test]
    fn scorecard_handles_missing_test_split() {
        let runs = vec![res(0.5, 0.7, None, 5)];
        let card = scorecard("demo", &runs);
        assert!(card.contains("| test | — | — | — | — |"), "no test split -> dashes");
    }
}
