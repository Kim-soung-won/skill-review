//! 실행 관측성: 작은 구조화된 이벤트 스트림과 사람이 읽을 수 있는 렌더러.
//!
//! 최적화 루프([`crate::optimize`])와 eval 패스([`crate::eval`])는
//! 즉석 `println!` 대신 [`Emitter`]를 통해 의미 있는 [`Event`]를 발행한다.
//! [`render`]는 순수 함수(이벤트 + 컬러 -> 문자열)이므로 LLM 없이 단위 테스트 가능.
//! TTY가 아니면(파이프, `/skillsmith`로 릴레이될 때) 컬러가 자동 비활성화되어
//! 동일 이벤트가 호스트가 나레이션할 수 있는 깔끔한 일반 텍스트로 렌더링된다.
//!
//! JSON/NDJSON 렌더러는 나중에 동일한 [`Event`] 심(seam)에 추가 가능
//! (`serde_json` 추가 필요); v1은 사람용 렌더러만 제공 (의존성 없음).

use std::io::{IsTerminal, Write};
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

/// 이전 최선과 현재 후보 간의 태스크별 점수 변화.
pub struct Delta {
    pub id: String,
    pub from: f64,
    pub to: f64,
}

/// 실행의 의미 있는 마일스톤. 문자열을 소유 — 이벤트는 즉시 생성하고 발행하며,
/// 몇 번의 할당은 수 초짜리 LLM 호출에 비하면 무시할 수준이고,
/// 소유하면 루프에서 빌림 관련 복잡함을 피할 수 있다.
pub enum Event {
    /// 배너: 실행할 내용, 사용할 프로바이더/모델 (`genai` 프로바이더에 적용;
    /// CLI 프로바이더는 자체 모델 사용).
    RunStart {
        project: String,
        provider: String,
        agent_model: String,
        optimizer_model: String,
        tasks: usize,
        rounds: u32,
        holdout: usize,
    },
    /// 단계 헤더, 예: `baseline eval` 또는 `round 1 · propose`.
    Stage { label: String },
    /// 한 태스크의 채점 결과 + 에이전트 호출 및 judge 소요 시간.
    Task {
        id: String,
        passed: bool,
        score: f64,
        ms: u128,
    },
    /// 베이스라인 게이트 점수 (베이스라인 단계 종료).
    Baseline { score: f64, ms: u128 },
    /// 한 라운드의 게이트 결정, 태스크별 델타와 스킬 줄 변화량.
    Round {
        round: u32,
        cand: f64,
        best: f64,
        accepted: bool,
        deltas: Vec<Delta>,
        skill_add: usize,
        skill_del: usize,
    },
    /// 스테이징된 최선 스킬이 저장됨 (수락된 라운드 후 또는 종료 시).
    Staged { path: String, round: Option<u32> },
    /// 최종 요약: baseline -> best, 총 비용, 채택 힌트 (개선 없으면 None).
    RunEnd {
        baseline: f64,
        best: f64,
        /// held-out test 스플릿의 최선 스킬 평균 점수 (편향 없음), test 스플릿 없으면 None.
        test_score: Option<f64>,
        staged: String,
        adopt_cmd: Option<String>,
        calls: u32,
        ms: u128,
    },
}

/// 이벤트를 stdout에 발행(사람용 렌더러), LLM 호출 횟수 추적, 실행 시작 시각 보유.
/// 생성 비용 낮음; `silent()`는 테스트용.
pub struct Emitter {
    enabled: bool,
    color: bool,
    t0: Instant,
    calls: AtomicU32,
}

impl Emitter {
    /// 라이브 발행자. `plain`은 TTY에서도 컬러 강제 비활성 (깔끔한 파이프용).
    pub fn new(plain: bool) -> Self {
        Emitter {
            enabled: true,
            color: !plain && std::io::stdout().is_terminal(),
            t0: Instant::now(),
            calls: AtomicU32::new(0),
        }
    }

    /// 테스트 / 라이브러리 호출자가 출력 없이 사용하는 no-op 발행자.
    pub fn silent() -> Self {
        Emitter {
            enabled: false,
            color: false,
            t0: Instant::now(),
            calls: AtomicU32::new(0),
        }
    }

    /// LLM 호출 한 번 기록 (최종 비용 요약에 사용).
    pub fn note_call(&self) {
        self.calls.fetch_add(1, Ordering::Relaxed);
    }

    pub fn calls(&self) -> u32 {
        self.calls.load(Ordering::Relaxed)
    }

    /// 발행자 생성 이후 경과 시간 (밀리초) (최종 요약용).
    pub fn elapsed_ms(&self) -> u128 {
        self.t0.elapsed().as_millis()
    }

    /// 이벤트를 stdout에 렌더링하고 플러시 — LLM 대기 중에도 라이브 진행 상황이 보이도록
    /// (플러시 안 하면 종료 시까지 버퍼링 — "멈춘 건가?" 버그).
    pub fn emit(&self, ev: &Event) {
        if !self.enabled {
            return;
        }
        let s = render(ev, self.color);
        print!("{s}");
        let _ = std::io::stdout().flush();
    }
}

// ---- 순수 렌더링 (단위 테스트) ----------------------------------------

/// ANSI 색상 헬퍼; `color`가 false면 no-op (비-TTY / `--plain`).
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

/// 사람이 읽을 수 있는 시간: `320ms`, `1.2s`, `1m04s`.
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

/// 멀티셋 줄 diff: 두 스킬 간 추가/삭제된 줄 수 반환.
/// 새 스킬에 N번 더 나타나는 줄은 N줄 추가로 계산 (반대도 마찬가지).
/// 저렴하고 의존성 없으며 "+12/-3 줄" 수준의 한눈에 보기에 충분.
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

/// 이벤트를 문자열로 렌더링. 순수 함수: 동일 입력 -> 동일 출력 (모든 타이밍/카운트는
/// 이벤트에 포함됨)이므로 사람용 포맷을 단위 테스트 가능.
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
        // "b" 한 줄 삭제, "d" 한 줄 추가; "a"/"c"는 변경 없음.
        assert_eq!(line_diff("a\nb\nc", "a\nc\nd"), (1, 1));
        // 순수 증가.
        assert_eq!(line_diff("a", "a\nb\nc"), (2, 0));
        // 동일 -> 변화 없음.
        assert_eq!(line_diff("x\ny", "x\ny"), (0, 0));
    }

    #[test]
    fn render_is_plain_without_color() {
        // 컬러 off일 때 ANSI 이스케이프 없음 (호스트 릴레이 / 파이프 경로).
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
