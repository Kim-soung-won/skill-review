//! LLM ports + adapters.
//!
//! [`LlmProvider`] is the port. Two adapters ship:
//! - [`CliProvider`] — shells out to an installed agent CLI (claude / codex /
//!   gemini). Uses the CLI's **own auth — no API key**. This is the default.
//! - [`GenaiProvider`] — the raw API via `genai` (needs `ANTHROPIC_API_KEY`).
//!
//! [`build_provider`] selects one from project config at runtime via the
//! [`AnyProvider`] enum (enum dispatch — keeps the eval loop generic without
//! `dyn` async traits).

use anyhow::{Result, anyhow, bail};
use genai::Client;
use genai::chat::{ChatMessage, ChatRequest};
use tokio::process::Command;

use crate::config::ProjectConfig;

/// Port: a chat backend. Implementors are swappable (CLI, raw API, or a mock).
#[allow(async_fn_in_trait)]
pub trait LlmProvider {
    async fn complete(&self, model: &str, system: &str, user: &str) -> Result<String>;
}

/// Adapter: raw API via `genai` (Claude resolved by the `claude-` model prefix;
/// key from `ANTHROPIC_API_KEY`).
pub struct GenaiProvider {
    client: Client,
}

impl GenaiProvider {
    pub fn new() -> Self {
        Self {
            client: Client::default(),
        }
    }
}

impl Default for GenaiProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl LlmProvider for GenaiProvider {
    async fn complete(&self, model: &str, system: &str, user: &str) -> Result<String> {
        let req = ChatRequest::new(vec![ChatMessage::system(system), ChatMessage::user(user)]);
        let res = self.client.exec_chat(model, req, None).await?;
        Ok(res.first_text().map(|s| s.to_string()).unwrap_or_default())
    }
}

/// Adapter: shell out to an installed agent CLI (claude / codex / gemini / …).
/// Uses the CLI's existing auth — **no API key**. `system` + `user` are combined
/// into one prompt appended as the final argument; stdout is the response.
pub struct CliProvider {
    cmd: Vec<String>,
}

impl CliProvider {
    pub fn new(cmd: Vec<String>) -> Self {
        Self { cmd }
    }

    /// Built-in command presets for common agent CLIs (prompt appended as the
    /// final arg). `codex`/`gemini` are best-effort; override via `provider_cmd`.
    pub fn preset(name: &str) -> Option<Vec<String>> {
        let v = match name {
            "claude" => vec!["claude", "-p"],
            "codex" => vec!["codex", "exec"],
            "gemini" => vec!["gemini", "-p"],
            _ => return None,
        };
        Some(v.into_iter().map(String::from).collect())
    }
}

impl LlmProvider for CliProvider {
    async fn complete(&self, _model: &str, system: &str, user: &str) -> Result<String> {
        let (bin, args) = self
            .cmd
            .split_first()
            .ok_or_else(|| anyhow!("empty provider command"))?;
        let prompt = format!("{system}\n\n{user}");
        let out = Command::new(bin)
            .args(args)
            .arg(&prompt)
            .current_dir(std::env::temp_dir()) // isolate any stray tool side-effects
            .output()
            .await
            .map_err(|e| anyhow!("running `{bin}`: {e} (installed and on PATH?)"))?;
        if !out.status.success() {
            bail!(
                "`{bin}` exited {}: {}",
                out.status.code().unwrap_or(-1),
                String::from_utf8_lossy(&out.stderr)
            );
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }
}

/// Runtime-selected provider. Enum dispatch avoids `dyn` async-trait while
/// keeping the eval loop generic over [`LlmProvider`].
pub enum AnyProvider {
    Genai(GenaiProvider),
    Cli(CliProvider),
}

impl LlmProvider for AnyProvider {
    async fn complete(&self, model: &str, system: &str, user: &str) -> Result<String> {
        match self {
            AnyProvider::Genai(p) => p.complete(model, system, user).await,
            AnyProvider::Cli(p) => p.complete(model, system, user).await,
        }
    }
}

/// Build a provider from project config (base `provider_cmd`, no stage override).
/// The default (`"claude"`) uses the installed Claude CLI — no API key required.
pub fn build_provider(cfg: &ProjectConfig) -> Result<AnyProvider> {
    build_provider_with(cfg, &cfg.provider_cmd)
}

/// Build a provider for one stage, honoring a per-stage `provider_cmd` override
/// (CLI providers only — `genai` tiers by model, so the command is irrelevant and
/// both stages resolve to the same raw-API client). An empty `stage_cmd` falls
/// back to the base `provider_cmd`.
pub fn build_stage_provider(cfg: &ProjectConfig, stage_cmd: &[String]) -> Result<AnyProvider> {
    let cmd = if stage_cmd.is_empty() {
        &cfg.provider_cmd
    } else {
        stage_cmd
    };
    build_provider_with(cfg, cmd)
}

fn build_provider_with(cfg: &ProjectConfig, provider_cmd: &[String]) -> Result<AnyProvider> {
    match cfg.provider.as_str() {
        "genai" => Ok(AnyProvider::Genai(GenaiProvider::new())),
        "cli" => {
            if provider_cmd.is_empty() {
                bail!("provider = \"cli\" requires a non-empty provider_cmd");
            }
            Ok(AnyProvider::Cli(CliProvider::new(provider_cmd.to_vec())))
        }
        name => {
            let cmd = if !provider_cmd.is_empty() {
                provider_cmd.to_vec()
            } else {
                CliProvider::preset(name).ok_or_else(|| {
                    anyhow!("unknown provider '{name}' (use claude|codex|gemini|genai|cli)")
                })?
            };
            Ok(AnyProvider::Cli(CliProvider::new(cmd)))
        }
    }
}
