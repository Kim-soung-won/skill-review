//! skillsmith — 프로젝트에 무관한 옵티마이저 코어 (라이브러리 크레이트).
//!
//! 포트(트레이트)는 확장 포인트:
//! - [`llm::LlmProvider`] — 채팅 백엔드. 어댑터: [`llm::GenaiProvider`]
//!   (직접 API), [`llm::CliProvider`] (설치된 에이전트 CLI: claude / codex
//!   / gemini — CLI 자체 인증 사용, API 키 불필요).
//! - [`judge::Judge`] — 에이전트의 편집을 채점. 어댑터: [`judge::ExecJudge`]
//!   (worktree에 적용 후 레포 테스트 실행).
//!
//! CLI 바이너리(`main.rs`)는 이 API의 얇은 컴포지션 루트다.

pub mod agent;
pub mod bench;
pub mod config;
pub mod deploy;
pub mod eval;
pub mod judge;
pub mod llm;
pub mod obs;
pub mod optimize;
pub mod report;
pub mod results;
pub mod seed;
pub mod worktree;

pub use agent::{Edit, parse_edits};
pub use config::{Project, ProjectConfig, ProjectSummary, Task, list_projects};
pub use eval::{EvalReport, eval_skill};
pub use judge::{ExecJudge, Judge, Outcome};
pub use llm::{AnyProvider, CliProvider, GenaiProvider, LlmProvider, build_provider};
