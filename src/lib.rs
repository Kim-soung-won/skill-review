//! skillsmith — the project-agnostic optimizer core (library crate).
//!
//! Ports (traits) are the extension seams:
//! - [`llm::LlmProvider`] — a chat backend. Adapters: [`llm::GenaiProvider`]
//!   (raw API) and [`llm::CliProvider`] (an installed agent CLI: claude / codex
//!   / gemini — uses the CLI's own auth, no API key).
//! - [`judge::Judge`] — grades an agent's edits. Adapter: [`judge::ExecJudge`]
//!   (apply in a worktree, run the repo's tests).
//!
//! The CLI binary (`main.rs`) is a thin composition root over this API.

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
