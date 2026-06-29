//! 머신 리더블 실행 결과 (`results.json`) — 벤치마크 / 자동화 심(seam).
//!
//! 산문 `report.md`는 사람용으로 유지; 이것은 스윕, 스코어카드, CI 게이트,
//! 분산 연구가 소비하는 파싱 가능한 아티팩트. 필드 순서는 구조체 선언 순서
//! (serde는 선언 순서로 직렬화)이므로 JSON이 실행 간 안정적이고 diff 가능.
//! `schema_version`은 소비자가 안전하게 진화할 수 있게 함.
//! `run` 종료 시 `report.md`와 함께 저장.

use serde::Serialize;

/// 한 번 실행의 완전한 머신 리더블 결과.
#[derive(Serialize)]
pub struct Results {
    /// 이 구조체 형태의 파괴적 변경 시 증가.
    pub schema_version: u32,
    pub project: String,
    /// 저장 시점의 Unix 초 (감사 / 정렬용; 벽시계 포맷팅 의존성 없음).
    pub timestamp_unix: u64,
    pub provider: String,
    pub agent_model: String,
    pub optimizer_model: String,
    pub rounds_configured: u32,
    pub holdout_count: usize,
    /// 이 실행이 채점된 입력(config + target/context 파일)의 해시 —
    /// 결과를 생성한 레포 상태에 묶음 (`.last-run`과 동일).
    pub config_fingerprint: String,
    /// LLM 호출 횟수 (skillsmith의 비용 단위; CLI 프로바이더는 토큰 수를 노출하지
    /// 않으므로 토큰이 아닌 호출 횟수).
    pub llm_calls: u32,
    pub elapsed_ms: u64,
    pub baseline_score: f64,
    pub best_score: f64,
    pub lift: f64,
    /// held-out `test` 스플릿에서 최선 스킬의 평균 점수, 마지막에 단 한 번 평가
    /// (최적화 중 한 번도 보지 않음) — 편향 없는 수치. test 스플릿 없으면 `None`.
    pub test_score: Option<f64>,
    /// 베이스라인(초기) 스킬의 태스크별 결과.
    pub baseline: Vec<TaskResult>,
    /// 수락된 최선 스킬의 태스크별 결과 (라운드 수락 없으면 == baseline).
    pub best: Vec<TaskResult>,
    /// held-out `test` 스플릿의 태스크별 결과 (test 스플릿 없으면 비어 있음).
    pub test: Vec<TaskResult>,
    /// 라운드별 게이트 결정.
    pub rounds: Vec<RoundResult>,
    pub staged_path: String,
}

/// 한 태스크의 채점 결과 (게이트 점수는 held-out 부분집합의 평균).
#[derive(Serialize, Clone)]
pub struct TaskResult {
    pub id: String,
    pub passed: bool,
    /// 연속값 [0,1]: 태스크 테스트 케이스 중 통과한 비율.
    pub score: f64,
    /// 학습에서 숨겨짐 (있을 때 이것으로 게이트 점수를 냄).
    pub holdout: bool,
}

/// 한 최적화 라운드의 수락/거절 결정.
#[derive(Serialize, Clone)]
pub struct RoundResult {
    pub round: u32,
    pub candidate_score: f64,
    /// 후보가 게이트된 최선 점수 (수락에는 엄격한 `>` 필요).
    pub best_score: f64,
    pub accepted: bool,
}

/// 예쁜 JSON으로 직렬화. 안정적인 키 순서 (구조체 필드 순서).
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
        // 유효한 JSON으로 라운드트립.
        let v: serde_json::Value = serde_json::from_str(&s).unwrap();
        assert_eq!(v["schema_version"], 1);
        assert_eq!(v["project"], "demo");
        assert_eq!(v["lift"], 0.2);
        // 태스크별 분석 유지 (벤치마크 가능 부분).
        assert_eq!(v["baseline"][1]["id"], "b");
        assert_eq!(v["best"][1]["score"], 0.4);
        assert_eq!(v["best"][1]["holdout"], true);
        assert_eq!(v["rounds"][0]["accepted"], true);
        assert_eq!(v["llm_calls"], 7);
    }

    #[test]
    fn field_order_is_stable() {
        // schema_version이 첫 번째 키여야 함 (소비자가 버전을 빠르게 감지 가능).
        let s = to_json(&sample()).unwrap();
        let first_key = s.find('"').map(|i| &s[i..]).unwrap();
        assert!(first_key.starts_with("\"schema_version\""), "schema_version leads");
    }
}
