//! `skillsmith deploy` — 마지막 단계: 채택된 스킬을 코딩 에이전트가 실제로 읽는 위치에 배치한다.
//! 옵티마이저 + `adopt`는 프로젝트 로컬 `skill.md`를 생성하지만, 아무것도 그 경로를 자동으로
//! 읽지는 않는다. `deploy`는 순수 파일 작업 — **LLM 없음, 토큰 없음**:
//!
//! - `--as skill`   → `name`/`description` frontmatter로 감싸서
//!   `<repo>/.claude/skills/<name>/SKILL.md`에 기록 (description이 일치하면 Claude가 자동 로드).
//! - `--as context` → body를 idempotent 마커 사이에 삽입해서 항상 켜져 있는
//!   컨텍스트 파일(`CLAUDE.md` / `AGENTS.md` / `GEMINI.md`)에 주입 — 세 에이전트 모두 읽는 경로.
//!
//! deploy 루트는 프로젝트 `.skillsmith/` 홈을 포함하는 **실제 레포** —
//! `repo_path`가 아님(fixture 기반 프로젝트의 경우 합성 eval 샌드박스임).

use crate::config::{Project, enclosing_git_root};
use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

/// `deploy` 옵션 (CLI 플래그에서 파싱).
pub struct DeployOpts {
    /// "skill" (Claude `.claude/skills` 파일) | "context" (항상 켜져 있는 컨텍스트 파일).
    pub as_kind: String,
    /// context: 대상 파일 (기본값 `CLAUDE.md`).
    pub to: Option<String>,
    /// skill: frontmatter `description` / 트리거 문구.
    pub desc: Option<String>,
    /// skill/블록 이름 (기본값: 프로젝트 이름).
    pub name: Option<String>,
    /// deploy 루트 오버라이드 (기본값: 프로젝트 `.skillsmith/`를 포함하는 git 레포).
    pub root: Option<String>,
    /// context: 에이전트 → 파일 csv (`claude`=CLAUDE.md, `codex`=AGENTS.md, `gemini`=GEMINI.md).
    pub agents: Option<String>,
}

/// 한 줄 스칼라를 YAML 이중 인용 (description은 한 줄에 들어가야 함).
fn yaml_quote(s: &str) -> String {
    let esc = s
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
        .replace(['\n', '\r'], " ");
    format!("\"{esc}\"")
}

/// Claude `SKILL.md` 생성: `name`/`description` frontmatter + 스킬 body. 순수 함수.
pub fn wrap_skill(name: &str, description: &str, body: &str) -> String {
    format!(
        "---\nname: {name}\ndescription: {desc}\n---\n\n{body}\n",
        desc = yaml_quote(description),
        body = body.trim()
    )
}

/// `name`의 idempotent 마커 사이에 `body`를 `existing`에 삽입한다. 마커가 이미 존재하면
/// 그 사이 블록을 교체 (재배포 시 제자리 업데이트); 없으면 끝에 추가. 순수 함수.
pub fn inject_context(existing: &str, name: &str, body: &str) -> String {
    let start = format!("<!-- skillsmith:{name} START -->");
    let end = format!("<!-- skillsmith:{name} END -->");
    let block = format!("{start}\n{}\n{end}", body.trim());
    let (s, e) = match (existing.find(&start), existing.find(&end)) {
        (Some(s), Some(e)) if e > s => (s, e),
        _ => {
            let mut out = existing.trim_end().to_string();
            if !out.is_empty() {
                out.push_str("\n\n");
            }
            out.push_str(&block);
            out.push('\n');
            return out;
        }
    };
    format!("{}{}{}", &existing[..s], block, &existing[e + end.len()..])
}

/// 배포할 실제 레포: `--root` > 프로젝트 `.skillsmith/`를 포함하는 git 레포 > cwd.
/// `repo_path`가 아님 (fixture 샌드박스일 수 있음).
fn deploy_root(project: &Project, override_root: Option<&str>) -> Result<PathBuf> {
    if let Some(r) = override_root {
        return Ok(PathBuf::from(r));
    }
    if let Some(git) = enclosing_git_root(&project.dir) {
        return Ok(git);
    }
    std::env::current_dir().context("no --root and no enclosing git repo found; pass --root <path>")
}

fn resolve_name(opts: &DeployOpts, project: &Project) -> String {
    if let Some(n) = &opts.name {
        return n.clone();
    }
    if !project.cfg.deploy.name.is_empty() {
        return project.cfg.deploy.name.clone();
    }
    project.cfg.name.clone()
}

fn resolve_desc(opts: &DeployOpts, project: &Project, name: &str) -> String {
    if let Some(d) = &opts.desc {
        return d.clone();
    }
    if !project.cfg.deploy.description.is_empty() {
        return project.cfg.deploy.description.clone();
    }
    eprintln!(
        "warning: no --desc and no [deploy] description in config.toml — wrote a placeholder. \
Edit the SKILL.md `description:` with real trigger phrases (what the user would say) so the agent \
auto-loads it."
    );
    format!("Use when working on {name} in this repo. Replace with concrete trigger phrases.")
}

/// context 대상: `--agents` csv → 파일, 없으면 `--to`, 없으면 `CLAUDE.md`.
fn context_targets(opts: &DeployOpts) -> Vec<String> {
    if let Some(a) = &opts.agents {
        return a
            .split(',')
            .filter_map(|s| match s.trim() {
                "" => None,
                "claude" => Some("CLAUDE.md".to_string()),
                "codex" => Some("AGENTS.md".to_string()),
                "gemini" => Some("GEMINI.md".to_string()),
                other => Some(other.to_string()),
            })
            .collect();
    }
    vec![opts.to.clone().unwrap_or_else(|| "CLAUDE.md".to_string())]
}

fn write_file(path: &Path, content: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, content).with_context(|| format!("writing {}", path.display()))
}

/// `skillsmith deploy` — 프로젝트의 채택된 스킬을 에이전트가 읽는 위치에 배치한다.
pub fn deploy(home: &Path, project_name: &str, opts: &DeployOpts) -> Result<()> {
    let project = Project::load(home, project_name)?;
    let staged = project.dir.join("skill.staged.md");
    if staged.exists() {
        eprintln!(
            "warning: an unadopted staged proposal exists ({}). deploy uses the LIVE skill — run \
`skillsmith adopt --project {}` first to deploy the optimized one.",
            staged.display(),
            project_name
        );
    }
    let body = std::fs::read_to_string(project.skill_path())
        .with_context(|| format!("reading live skill {}", project.skill_path().display()))?;
    let root = deploy_root(&project, opts.root.as_deref())?;
    let name = resolve_name(opts, &project);

    match opts.as_kind.as_str() {
        "skill" => {
            let desc = resolve_desc(opts, &project, &name);
            let dest = root.join(".claude/skills").join(&name).join("SKILL.md");
            write_file(&dest, &wrap_skill(&name, &desc, &body))?;
            println!("deployed skill -> {}", dest.display());
            println!(
                "  Claude auto-loads it when the description matches; edit the trigger phrases if needed."
            );
        }
        "context" => {
            for f in context_targets(opts) {
                let dest = root.join(&f);
                let existing = std::fs::read_to_string(&dest).unwrap_or_default();
                write_file(&dest, &inject_context(&existing, &name, &body))?;
                println!(
                    "deployed context -> {} (block `skillsmith:{}`)",
                    dest.display(),
                    name
                );
            }
        }
        other => bail!("unknown --as '{other}' (use: skill | context)"),
    }
    Ok(())
}
