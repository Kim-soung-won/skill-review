//! 프로젝트 설정 + eval 태스크 모델. "프로젝트"는 `projects/<name>/config.toml`에 위치한
//! 레포별 어댑터 — 옵티마이저 코어는 프로젝트에 무관하고, 이 파일들만 프로젝트 고유하다.

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Deserialize)]
pub struct ProjectConfig {
    pub name: String,
    /// 실행 judge용 git worktree 레포. 생략(레포 로컬 모드)하면
    /// 프로젝트 `.skillsmith/` 홈을 포함하는 git 레포를 기본값으로 사용.
    #[serde(default)]
    pub repo_path: String,
    /// 최적화 대상 스킬 파일 (프로젝트 디렉토리 기준 상대 경로).
    pub skill_file: String,
    #[serde(default = "default_agent_model")]
    pub agent_model: String,
    #[serde(default = "default_optimizer_model")]
    pub optimizer_model: String,
    /// LLM 백엔드: "claude" | "codex" | "gemini" (설치된 CLI, API 키 불필요) |
    /// "genai" (ANTHROPIC_API_KEY로 직접 API 호출) | "cli" (커스텀 `provider_cmd`).
    #[serde(default = "default_provider")]
    pub provider: String,
    /// 커스텀 CLI 기본 커맨드 (프롬프트가 마지막 인자로 붙음). `provider = "cli"`이거나
    /// 프리셋 커맨드를 오버라이드할 때 사용.
    #[serde(default)]
    pub provider_cmd: Vec<String>,
    /// 저렴한 에이전트(eval) 단계용 CLI 커맨드 오버라이드. 작은 모델로 티어 다운 가능,
    /// 예: `["claude","-p","--model","claude-haiku-4-5"]` 또는 `["codex","exec","-m","gpt-5-mini"]`.
    /// 비어 있으면 `provider_cmd` 사용. `genai`는 `agent_model`로 티어링하므로 무시됨.
    #[serde(default)]
    pub agent_provider_cmd: Vec<String>,
    /// 옵티마이저(propose) 단계용 CLI 커맨드 오버라이드. 비어 있으면
    /// `provider_cmd` 사용. `genai`는 `optimizer_model`로 티어링하므로 무시됨.
    #[serde(default)]
    pub optimizer_provider_cmd: Vec<String>,
    #[serde(default = "default_rounds")]
    pub rounds: u32,
    #[serde(default, rename = "task")]
    pub tasks: Vec<Task>,
    /// `skillsmith deploy`용 선택적 `[deploy]` 기본값 (스킬 이름 + frontmatter `description`
    /// 트리거 문구). 둘 다 선택적이며 CLI 플래그가 우선함.
    #[serde(default)]
    pub deploy: DeployConfig,
}

/// `[deploy]` 기본값 — 프로젝트가 스킬 이름 + 트리거 문구를 고정해두면
/// `skillsmith deploy`에 플래그 없이 실행 가능 (CI/헤드리스 환경에서도 동작).
#[derive(Debug, Clone, Deserialize, Default)]
pub struct DeployConfig {
    /// 스킬/블록 이름 오버라이드 (기본값: 프로젝트 `name`).
    #[serde(default)]
    pub name: String,
    /// `--as skill`용 `description:` 트리거 문구 (기본값: `--desc`, 없으면 플레이스홀더).
    #[serde(default)]
    pub description: String,
}

/// 태스크가 속하는 스플릿. **train** — 옵티마이저가 실패를 보고 편집을 제안.
/// **val** — 옵티마이저에게 숨겨짐; 게이트가 이것으로 수락/거절 결정.
/// **test** — 최적화 중 절대 실행되지 않음; 마지막에 최적 스킬에 대해 단 한 번 평가해 편향 없는 최종 수치 산출.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TaskSplit {
    Train,
    Val,
    Test,
}

#[derive(Debug, Clone, Deserialize)]
pub struct Task {
    pub id: String,
    /// 에이전트가 수행해야 할 내용 (테스트 파일은 숨겨져 있어 보이지 않음).
    pub intent: String,
    /// 에이전트에게 컨텍스트로 보여주는 파일들 (repo_path 기준 상대 경로).
    #[serde(default)]
    pub context_files: Vec<String>,
    /// 에이전트가 작성할 것으로 기대되는 파일들 (프롬프트의 힌트용).
    #[serde(default)]
    pub target_files: Vec<String>,
    /// 학습에서 숨김: 옵티마이저가 이 태스크의 실패를 절대 보지 않으며,
    /// 게이트는 held-out 태스크가 있을 때 그것만으로 점수를 매김. 기본값 false.
    /// `split = "val"`의 하위 호환 별칭 — 둘 다 설정되면 `split`이 우선.
    #[serde(default)]
    pub holdout: bool,
    /// 명시적 train/val/test 스플릿. 생략 시 `holdout = true`면 `val`, 아니면 `train`.
    #[serde(default)]
    pub split: Option<TaskSplit>,
    /// 편집 적용 전 worktree에서 실행되는 선택적 셸 커맨드 (초기 상태 설정).
    #[serde(default)]
    pub setup_cmd: String,
    /// 편집 후 worktree에서 실행; exit code 0 == 통과.
    pub verify_cmd: String,
}

impl Task {
    /// 결정된 스플릿: 명시적 `split`이 우선; 없으면 `holdout`이면 `val`, 아니면 `train`.
    pub fn split(&self) -> TaskSplit {
        self.split.unwrap_or(if self.holdout {
            TaskSplit::Val
        } else {
            TaskSplit::Train
        })
    }
}

fn default_agent_model() -> String {
    "claude-sonnet-4-6".to_string()
}
fn default_optimizer_model() -> String {
    "claude-opus-4-8".to_string()
}
fn default_provider() -> String {
    "claude".to_string()
}
fn default_rounds() -> u32 {
    3
}

/// 로드된 프로젝트: 디렉토리 + 파싱된 설정.
pub struct Project {
    pub dir: PathBuf,
    pub cfg: ProjectConfig,
}

impl Project {
    pub fn load(home: &Path, name: &str) -> Result<Self> {
        let dir = home.join("projects").join(name);
        let cfg_path = dir.join("config.toml");
        let text = std::fs::read_to_string(&cfg_path)
            .with_context(|| format!("reading {}", cfg_path.display()))?;
        let cfg: ProjectConfig =
            toml::from_str(&text).with_context(|| format!("parsing {}", cfg_path.display()))?;
        Ok(Self { dir, cfg })
    }

    /// worktree할 레포의 절대 경로.
    /// - 절대 `repo_path`: 그대로 사용.
    /// - 상대 `repo_path`: 프로젝트 디렉토리 기준으로 해석 (cwd 무관), 이후 정규화.
    /// - 비어 있음(레포 로컬 모드): 프로젝트 디렉토리를 포함하는 git 레포
    ///   (`.skillsmith/` 홈의 부모) — 커밋된 레포 내 프로젝트는 경로 불필요.
    pub fn repo(&self) -> Result<PathBuf> {
        let raw = self.cfg.repo_path.trim();
        if raw.is_empty() {
            return enclosing_git_root(&self.dir).with_context(|| {
                format!(
                    "repo_path is empty and no enclosing git repo was found above {} — \
set repo_path in config.toml",
                    self.dir.display()
                )
            });
        }
        let p = Path::new(raw);
        if p.is_absolute() {
            return Ok(p.to_path_buf());
        }
        let joined = self.dir.join(p);
        Ok(std::fs::canonicalize(&joined).unwrap_or(joined))
    }

    pub fn skill_path(&self) -> PathBuf {
        self.dir.join(&self.cfg.skill_file)
    }
}

/// `start`에서 위로 올라가며 `.skillsmith/` 디렉토리를 포함하는 가장 가까운 조상을 찾는다;
/// 해당 `.skillsmith` 경로(레포 로컬 홈)를 반환. 가장 가까운 것이 우선,
/// git이 루트를 찾는 방식과 동일. 순수 함수(env/cwd 없음)이므로 단위 테스트 가능.
pub fn discover_dot_skillsmith(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        let candidate = dir.join(".skillsmith");
        if candidate.is_dir() {
            return Some(candidate);
        }
    }
    None
}

/// `start`에서 위로 올라가며 `.git`이 있는 가장 가까운 git 레포 루트를 찾는다.
pub fn enclosing_git_root(start: &Path) -> Option<PathBuf> {
    for dir in start.ancestors() {
        if dir.join(".git").exists() {
            return Some(dir.to_path_buf());
        }
    }
    None
}

/// 경로의 마지막 컴포넌트에서 프로젝트 이름 슬러그를 생성: ASCII 소문자 영숫자만 유지,
/// 나머지 연속 문자는 단일 `-`로 축소, 양끝 트리밍.
/// `skillsmith new`가 레포 디렉토리 이름으로 프로젝트를 자동 명명하는 데 사용.
pub fn slug_from_path(p: &Path) -> Option<String> {
    let name = p.file_name()?.to_string_lossy();
    let mut out = String::new();
    for ch in name.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if !out.ends_with('-') {
            out.push('-');
        }
    }
    let slug = out.trim_matches('-').to_string();
    (!slug.is_empty()).then_some(slug)
}

/// `<home>/projects/<name>/`에 새 프로젝트 어댑터(config + skill)를 생성한다.
/// `skillsmith new`가 호출 — 디렉토리를 직접 만들 필요 없음. `local`
/// (레포 로컬) 모드에서는 `repo_path`를 생략(포함하는 git 레포를 기본값)하고
/// `skill.md`/`config.toml`은 커밋되고 생성된 `skill.staged.md`/`report.md`는
/// 무시되도록 스크래치 `.gitignore`를 작성한다.
pub fn scaffold_project(home: &Path, name: &str, repo: Option<&str>, local: bool) -> Result<PathBuf> {
    let dir = home.join("projects").join(name);
    if dir.exists() {
        bail!("project already exists: {}", dir.display());
    }
    std::fs::create_dir_all(&dir)?;
    let repo_line = match repo {
        Some(r) => format!("repo_path = \"{r}\"\n"),
        None if local => "# repo_path omitted -> the git repo enclosing this .skillsmith/ dir\n\
repo_path = \"\"\n"
            .to_string(),
        None => "repo_path = \"/path/to/your/git/repo\"\n".to_string(),
    };
    let config = format!(
        "name = \"{name}\"\n\
{repo_line}\
skill_file = \"skill.md\"\n\
provider = \"claude\"        # claude | codex | gemini (installed CLI, no key) | genai\n\
# Tier the cheap agent stage to a smaller model (CLI providers; genai tiers by agent_model):\n\
# agent_provider_cmd = [\"claude\", \"-p\", \"--model\", \"claude-haiku-4-5\"]\n\
rounds = 3\n\
\n\
# Add eval tasks. Each runs in an isolated git worktree of repo_path (cwd = worktree root).\n\
# Keep the test file OUT of context_files (held out) so the SKILL carries the knowledge.\n\
[[task]]\n\
id = \"example\"\n\
intent = \"Describe what the agent must do (do not leak the answer).\"\n\
context_files = []\n\
target_files = []\n\
verify_cmd = \"echo replace-me; false\"   # exit 0 = pass\n"
    );
    std::fs::write(dir.join("config.toml"), config)?;
    let skill = format!(
        "# {name} — agent skill\n\n\
Seed conventions (the optimizer refines this against the eval tasks).\n\
Put durable, project-specific rules here.\n"
    );
    std::fs::write(dir.join("skill.md"), skill)?;
    if local {
        ensure_local_gitignore(home)?;
    }
    Ok(dir)
}

/// `<home>/.gitignore`를 한 번 작성 — 레포 로컬 `.skillsmith/`가 영구 아티팩트
/// (`skill.md`, `config.toml`)는 커밋하고 생성된 스크래치는 무시하도록.
fn ensure_local_gitignore(home: &Path) -> Result<()> {
    let gi = home.join(".gitignore");
    if gi.exists() {
        return Ok(());
    }
    std::fs::write(
        &gi,
        "# skillsmith scratch — commit skill.md + config.toml; ignore generated artifacts\n\
**/skill.staged.md\n\
**/report.md\n\
**/results.json\n\
**/bench/\n\
**/.last-run\n",
    )
    .with_context(|| format!("writing {}", gi.display()))?;
    Ok(())
}

/// `skillsmith list`의 한 행.
pub struct ProjectSummary {
    pub name: String,
    pub tasks: usize,
    pub repo: String,
}

/// `<home>/projects/*/config.toml` 아래의 모든 프로젝트를 탐색한다.
/// 새 프로젝트 폴더는 자동으로 인식 — 코드나 통합 변경 불필요.
pub fn list_projects(home: &Path) -> Result<Vec<ProjectSummary>> {
    let base = home.join("projects");
    let mut out = Vec::new();
    let entries = match std::fs::read_dir(&base) {
        Ok(e) => e,
        Err(_) => return Ok(out),
    };
    for entry in entries.flatten() {
        let cfg_path = entry.path().join("config.toml");
        if !cfg_path.is_file() {
            continue;
        }
        if let Ok(text) = std::fs::read_to_string(&cfg_path)
            && let Ok(cfg) = toml::from_str::<ProjectConfig>(&text)
        {
            out.push(ProjectSummary {
                name: cfg.name,
                tasks: cfg.tasks.len(),
                repo: if cfg.repo_path.trim().is_empty() {
                    "(enclosing repo)".to_string()
                } else {
                    cfg.repo_path
                },
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}
