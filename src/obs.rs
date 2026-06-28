//! Run observability: a small structured event stream with a human renderer.
//!
//! The optimization loop ([`crate::optimize`]) and eval pass ([`crate::eval`])
//! emit semantic [`Event`]s through an [`Emitter`] instead of ad-hoc `println!`.
//! [`render`] is a PURE function (event + color -> string) so the output is
//! unit-tested without an LLM. Color is auto-disabled off a TTY (pipes, and the
//! host-agent relay when driven via `/skillsmith`), so the same events render as
//! clean plain text a host can narrate.
//!
//! A JSON/NDJSON renderer can plug in behind the same [`Event`] seam later
//! (would add `serde_json`); v1 ships the human renderer only (zero new deps).

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

/// A per-task score movement between the prior best and the current candidate.
pub struct Delta {
    pub id: String,
    pub from: f64,
    pub to: f64,
}

/// A semantic milestone in a run. Owns its strings — events are built and
/// emitted immediately, and a few allocs are nothing next to multi-second LLM
/// calls, while owning keeps the loop free of borrow gymnastics.
pub enum Event {
    /// Banner: what is about to run, with which provider/models (the model
    /// fields apply to the `genai` provider; CLI providers use their own).
    RunStart {
        project: String,
        provider: String,
        agent_model: String,
        optimizer_model: String,
        tasks: usize,
        rounds: u32,
        holdout: usize,
    },
    /// A stage header, e.g. `baseline eval` or `round 1 · propose`.
    Stage { label: String },
    /// One task's graded outcome plus how long the agent call + judge took.
    Task {
        id: String,
        passed: bool,
        score: f64,
        ms: u128,
    },
    /// Baseline gate score (end of the baseline stage).
    Baseline { score: f64, ms: u128 },
    /// A round's gate decision, with per-task deltas and the skill's line churn.
    Round {
        round: u32,
        cand: f64,
        best: f64,
        accepted: bool,
        deltas: Vec<Delta>,
        skill_add: usize,
        skill_del: usize,
    },
    /// The staged best skill was persisted (after an accepted round, or at the end).
    Staged { path: String, round: Option<u32> },
    /// Final summary: baseline -> best, total cost, and the adopt hint (None when
    /// there was no improvement to adopt).
    RunEnd {
        baseline: f64,
        best: f64,
        /// Mean on the held-out test split (unbiased), if a test split exists.
        test_score: Option<f64>,
        staged: String,
        adopt_cmd: Option<String>,
        calls: u32,
        ms: u128,
    },
}

/// Emits events to stdout (human renderer), tracks the LLM-call count, and holds
/// the run's start instant. Cheap to construct; `silent()` is for tests.
pub struct Emitter {
    enabled: bool,
    color: bool,
    t0: Instant,
    calls: AtomicU32,
}

impl Emitter {
    /// Live emitter. `plain` forces color off even on a TTY (for clean piping).
    pub fn new(plain: bool) -> Self {
        Emitter {
            enabled: true,
            color: !plain && std::io::stdout().is_terminal(),
            t0: Instant::now(),
            calls: AtomicU32::new(0),
        }
    }

    /// A no-op emitter for tests / library callers that want no output.
    pub fn silent() -> Self {
        Emitter {
            enabled: false,
            color: false,
            t0: Instant::now(),
            calls: AtomicU32::new(0),
        }
    }

    /// Record that one LLM call was made (drives the final cost summary).
    pub fn note_call(&self) {
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn calls(&self) -> u32 {
        self.calls.load(Ordering::Relaxed)
    }

    /// Wall-clock since the emitter was created, in millis (for the final summary).
    pub fn elapsed_ms(&self) -> u128 {
        self.t0.elapsed().as_millis()
    }

    /// Render an event to stdout and flush so live progress shows during the long
    /// LLM awaits (un-flushed stdout would batch until exit — the "is it hung?" bug).
    pub fn emit(&self, ev: &Event) {
        if !self.enabled {
            return;
        }
        let s = render(ev, self.color);
        print!("{s}");
        let _ = std::io::stdout().flush();
    }
}

// ---- pure rendering (unit-tested) ----------------------------------------

/// ANSI paint helper; a no-op when `color` is false (non-TTY / `--plain`).
struct Paint {
    color: bool,
}
impl Paint {
    fn w(&self, code: &str, s: &str) -> String {
        if self.color {
            format!("\x1b[{code}m{s}\x1b[0m")
        } else {
            s.to_string()
        }
    }
    fn green(&self, s: &str) -> String {
        self.w("32", s)
    }
    fn red(&self, s: &str) -> String {
        self.w("31", s)
    }
    fn cyan(&self, s: &str) -> String {
        self.w("36", s)
    }
    fn dim(&self, s: &str) -> String {
        self.w("2", s)
    }
    fn bold(&self, s: &str) -> String {
        self.w("1", s)
    }
}

/// Human-readable duration: `320ms`, `1.2s`, `1m04s`.
pub fn dur(ms: u128) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        let s = ms / 1000;
        format!("{}m{:02}s", s / 60, s % 60)
    }
}

/// Multiset line diff: returns `(added, removed)` line counts between two skills.
/// A line present N more times in `new` than `old` counts as N added (and vice
/// versa). Cheap, dependency-free, and enough for an at-a-glance "+12/-3 lines".
pub fn line_diff(old: &str, new: &str) -> (usize, usize) {
    use std::collections::HashMap;
    let mut counts: HashMap<&str, i64> = HashMap::new();
    for l in old.lines() {
        *counts.entry(l).or_default() += 1;
    }
    for l in new.lines() {
        *counts.entry(l).or_default() -= 1;
    }
    let removed = counts.values().filter(|&&v| v > 0).map(|&v| v as usize).sum();
    let added = counts
        .values()
        .filter(|&&v| v < 0)
        .map(|&v| (-v) as usize)
        .sum();
    (added, removed)
}

/// Render one event to a string. PURE: same inputs -> same output (every
/// timing/count is carried in the event), so the human format is unit-tested.
pub fn render(ev: &Event, color: bool) -> String {
    let p = Paint { color };
    match ev {
        Event::RunStart {
            project,
            provider,
            agent_model,
            optimizer_model,
            tasks,
            rounds,
            holdout,
        } => {
            let ho = if *holdout > 0 {
                format!(" ({holdout} held-out)")
            } else {
                String::new()
            };
            format!(
                "{} {}\n  {} {} · {} {} · {} {}\n  {tasks} task(s){ho} · {rounds} round(s)\n\n",
                p.bold("skillsmith"),
                p.bold(project),
                p.dim("provider"),
                provider,
                p.dim("agent"),
                agent_model,
                p.dim("optimizer"),
                optimizer_model,
            )
        }
        Event::Stage { label } => format!("{} {}\n", p.cyan("▸"), p.bold(label)),
        Event::Task {
            id,
            passed,
            score,
            ms,
        } => {
            let mark = if *passed { p.green("✓") } else { p.red("✗") };
            format!(
                "  {mark} {:<22} {:>5.2}   {}\n",
                id,
                score,
                p.dim(&dur(*ms))
            )
        }
        Event::Baseline { score, ms } => format!(
            "  baseline gate score {}   {}\n\n",
            p.bold(&format!("{score:.3}")),
            p.dim(&format!("({})", dur(*ms)))
        ),
        Event::Round {
            round,
            cand,
            best,
            accepted,
            deltas,
            skill_add,
            skill_del,
        } => {
            let verdict = if *accepted {
                p.green("ACCEPT")
            } else {
                p.dim("reject")
            };
            let lift = cand - best;
            let lift_s = format!("{lift:+.3}");
            let lift_c = if *accepted { p.green(&lift_s) } else { p.dim(&lift_s) };
            let mut s = format!(
                "  round {round} → {}  (best {best:.3})  {verdict}  {lift_c}\n",
                p.bold(&format!("{cand:.3}")),
            );
            for d in deltas {
                s.push_str(&format!(
                    "    {} {:<20} {:.2} → {:.2}\n",
                    p.cyan("Δ"),
                    d.id,
                    d.from,
                    d.to
                ));
            }
            if *skill_add > 0 || *skill_del > 0 {
                s.push_str(&format!(
                    "    {} skill {}{}\n",
                    p.dim("·"),
                    p.green(&format!("+{skill_add}")),
                    p.red(&format!("/-{skill_del} lines")),
                ));
            }
            s.push('\n');
            s
        }
        Event::Staged { path, round } => {
            let what = match round {
                Some(r) => format!("staged round {r} best"),
                None => "staged best skill".to_string(),
            };
            format!("    {}\n", p.dim(&format!("{what} → {path}")))
        }
        Event::RunEnd {
            baseline,
            best,
            test_score,
            staged,
            adopt_cmd,
            calls,
            ms,
        } => {
            let lift = best - baseline;
            let mut s = String::new();
            s.push_str(&p.dim("─────────────────────────────────────────────\n"));
            s.push_str(&format!(
                "  baseline {baseline:.3} → best {}   lift {}\n",
                p.bold(&format!("{best:.3}")),
                if lift > 0.0 {
                    p.green(&format!("{lift:+.3}"))
                } else {
                    p.dim(&format!("{lift:+.3}"))
                },
            ));
            if let Some(t) = test_score {
                s.push_str(&format!(
                    "  held-out test {}   {}\n",
                    p.bold(&format!("{t:.3}")),
                    p.dim("(unbiased — never seen in training)")
                ));
            }
            s.push_str(&format!(
                "  {}\n",
                p.dim(&format!("{calls} call(s) · {}", dur(*ms)))
            ));
            s.push_str(&format!("  {}\n", p.dim(&format!("staged → {staged}"))));
            match adopt_cmd {
                Some(cmd) => s.push_str(&format!("  {}  {}\n", p.bold("adopt:"), cmd)),
                None => s.push_str(&format!(
                    "  {}\n",
                    p.dim("(no improvement over baseline — nothing to adopt)")
                )),
            }
            s
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dur_formats_ms_seconds_minutes() {
        assert_eq!(dur(320), "320ms");
        assert_eq!(dur(1200), "1.2s");
        assert_eq!(dur(64_000), "1m04s");
    }

    #[test]
    fn line_diff_counts_added_and_removed() {
        // one line removed ("b"), one added ("d"); "a"/"c" unchanged.
        assert_eq!(line_diff("a\nb\nc", "a\nc\nd"), (1, 1));
        // pure growth.
        assert_eq!(line_diff("a", "a\nb\nc"), (2, 0));
        // identical -> no churn.
        assert_eq!(line_diff("x\ny", "x\ny"), (0, 0));
    }

    #[test]
    fn render_is_plain_without_color() {
        // No ANSI escapes when color is off (the host-relay / pipe path).
        let ev = Event::Task {
            id: "magic".into(),
            passed: true,
            score: 0.75,
            ms: 1200,
        };
        let out = render(&ev, false);
        assert!(!out.contains('\x1b'), "no escapes in plain mode");
        assert!(out.contains("magic"));
        assert!(out.contains("0.75"));
        assert!(out.contains("1.2s"));
        assert!(out.contains('✓'));
    }

    #[test]
    fn render_round_shows_delta_and_skill_churn() {
        let ev = Event::Round {
            round: 1,
            cand: 0.700,
            best: 0.500,
            accepted: true,
            deltas: vec![Delta {
                id: "magic".into(),
                from: 0.50,
                to: 1.00,
            }],
            skill_add: 12,
            skill_del: 3,
        };
        let out = render(&ev, false);
        assert!(out.contains("round 1"));
        assert!(out.contains("0.700"));
        assert!(out.contains("ACCEPT"));
        assert!(out.contains("+0.200"), "lift is shown");
        assert!(out.contains("magic"));
        assert!(out.contains("0.50 → 1.00"), "per-task delta");
        assert!(out.contains("+12") && out.contains("-3 lines"), "skill churn");
    }

    #[test]
    fn render_color_wraps_with_ansi() {
        let ev = Event::Task {
            id: "x".into(),
            passed: false,
            score: 0.0,
            ms: 800,
        };
        let out = render(&ev, true);
        assert!(out.contains('\x1b'), "color mode emits ANSI escapes");
    }

    #[test]
    fn render_runend_without_improvement_omits_adopt() {
        let ev = Event::RunEnd {
            baseline: 0.5,
            best: 0.5,
            test_score: None,
            staged: "/p/skill.staged.md".into(),
            adopt_cmd: None,
            calls: 7,
            ms: 64_000,
        };
        let out = render(&ev, false);
        assert!(out.contains("nothing to adopt"));
        assert!(out.contains("7 call(s)"));
        assert!(out.contains("1m04s"));
    }
}
