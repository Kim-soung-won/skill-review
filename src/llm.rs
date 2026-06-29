//! LLM 포트 + 어댑터.
//!
//! [`LlmProvider`]가 포트. 두 가지 어댑터가 기본 제공:
//! - [`CliProvider`] — 설치된 에이전트 CLI(claude / codex / gemini)를 셸 호출.
//!   CLI 자체 인증 사용 — **API 키 불필요**. 기본값.
//! - [`GenaiProvider`] — `genai` 크레이트를 통한 직접 API (`ANTHROPIC_API_KEY` 필요).
//!
//! [`build_provider`]는 런타임에 프로젝트 설정으로 [`AnyProvider`] 열거형을 통해
//! 하나를 선택 (열거형 디스패치 — `dyn` async 트레이트 없이 eval 루프를 제네릭하게 유지).

use anyhow::{Result, anyhow, bail};
use genai::Client;
use genai::chat::{ChatMessage, ChatRequest};
use tokio::process::Command;

use crate::config::ProjectConfig;

/// 포트: 채팅 백엔드. 구현체는 교체 가능 (CLI, 직접 API, 또는 목(mock)).
#[allow(async_fn_in_trait)]
pub trait LlmProvider {
    async fn complete(&self, model: &str, system: &str, user: &str) -> Result<String>;
}

/// 어댑터: `genai`를 통한 직접 API (Claude는 `claude-` 모델 접두사로 인식;
/// `ANTHROPIC_API_KEY`에서 키 읽음).
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

/// 어댑터: 설치된 에이전트 CLI(claude / codex / gemini / …)를 셸 호출.
/// CLI의 기존 인증 사용 — **API 키 불필요**. `system` + `user`를 하나의 프롬프트로
/// 결합해 마지막 인자로 전달; stdout이 응답.
pub struct CliProvider {
    cmd: Vec<String>,
}

impl CliProvider {
    pub fn new(cmd: Vec<String>) -> Self {
        Self { cmd }
    }

    /// 주요 에이전트 CLI의 내장 커맨드 프리셋 (프롬프트가 마지막 인자로 붙음).
    /// `codex`/`gemini`는 최선 노력; `provider_cmd`로 오버라이드 가능.
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
            .current_dir(std::env::temp_dir()) // 도구 부작용 격리
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

/// 런타임 선택 프로바이더. 열거형 디스패치로 `dyn` async-trait 없이
/// eval 루프를 [`LlmProvider`]에 대해 제네릭하게 유지.
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

/// 프로젝트 설정으로 프로바이더 생성 (기본 `provider_cmd`, 단계 오버라이드 없음).
/// 기본값(`"claude"`)은 설치된 Claude CLI 사용 — API 키 불필요.
pub fn build_provider(cfg: &ProjectConfig) -> Result<AnyProvider> {
    build_provider_with(cfg, &cfg.provider_cmd)
}

/// 한 단계용 프로바이더 생성, 단계별 `provider_cmd` 오버라이드 적용
/// (CLI 프로바이더 전용 — `genai`는 모델로 티어링하므로 커맨드 무관,
/// 두 단계 모두 동일한 직접 API 클라이언트로 해석됨). 비어 있는 `stage_cmd`는
/// 기본 `provider_cmd`로 폴백.
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
