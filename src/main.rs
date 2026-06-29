//! skillsmith CLI — `skillsmith` 라이브러리 크레이트의 얇은 컴포지션 루트.

use anyhow::Result;
use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "skillsmith", version, about = "Eval-gated skill optimizer")]
struct Cli {
    /// skillsmith 홈 (`projects/`가 위치하는 곳). 기본값: $SKILLSMITH_HOME, 없으면 ~/.skillsmith.
    #[arg(long, global = true)]
    home: Option<String>,
    /// 출력에서 color/ANSI 비활성화 (stdout이 TTY가 아닐 때도 자동 비활성,
    /// 예: 파이프 또는 `/skillsmith`로 릴레이될 때).
    #[arg(long, global = true)]
    plain: bool,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 전체 루프: baseline -> propose -> candidate eval -> gate -> stage.
    Run {
        /// <home>/projects/<name>/ 아래의 프로젝트 이름
        #[arg(long)]
        project: String,
        /// config를 검증하고 각 verify_cmd를 worktree에서 LLM 없이 실행 (토큰 없음) —
        /// 실제 실행 전에 repo_path/verify_cmd가 올바른지 확인.
        #[arg(long)]
        dry_run: bool,
        /// skill.md / config.toml / 대상 파일이 변경될 때마다 재실행 —
        /// 포그라운드 개발 루프 (각 패스마다 토큰 소비). --dry-run과 함께 사용 시 무시됨.
        #[arg(long)]
        watch: bool,
    },
    /// 현재 스킬을 한 번 평가 (최적화 없음).
    Eval {
        #[arg(long)]
        project: String,
        /// 입력 변경 시마다 재평가 — 저렴한 루프 (최적화 라운드 없음).
        #[arg(long)]
        watch: bool,
    },
    /// 드리프트 감지 (LLM 없음, 토큰 없음): 마지막 `run` 이후 레포 입력이 변경되어
    /// 최적화된 스킬이 오래된 상태가 됐는가? git 훅 / CI가 분기할 수 있도록 드리프트 시 non-zero 종료.
    /// 감지만 함 — 절대 재실행하거나 채택하지 않음.
    Check {
        #[arg(long)]
        project: String,
    },
    /// 벤치마크: 최적화를 N회 실행(k-시드)하고 분산 인식 스코어카드 +
    /// sweep.jsonl을 <project>/bench/에 기록. N× 일반 실행 토큰 소비.
    Bench {
        #[arg(long)]
        project: String,
        /// 집계할 독립 실행 수 (분산에는 ≥2 필요). 기본값 3.
        #[arg(long, default_value_t = 3)]
        seeds: u32,
    },
    /// 스테이징된 제안 채택: skill.staged.md를 라이브 스킬 파일 위에 복사.
    Adopt {
        /// <home>/projects/<name>/ 아래의 프로젝트 이름
        #[arg(long)]
        project: String,
    },
    /// <home>/projects/ 아래의 발견된 프로젝트 목록 출력.
    List,
    /// 번들 데모 프로젝트를 <home>에 시드 (idempotent).
    Init,
    /// 새 프로젝트 생성. git 레포 안에서는 bare `skillsmith new`로 충분:
    /// 레포 로컬로 생성되고 레포 디렉토리 이름을 따름.
    New {
        /// 프로젝트 이름. 생략 시 레포(또는 --repo) 디렉토리 이름을 기본값으로 사용.
        name: Option<String>,
        /// 대상 git 레포 경로 (중앙 모드 선택; 이후 config.toml 편집).
        #[arg(long)]
        repo: Option<String>,
        /// git 레포 밖이거나 --repo와 함께 사용해도 레포 로컬 강제. (git 레포 안에서
        /// --repo 없이 사용하면 이미 레포 로컬이 기본값.)
        #[arg(long)]
        local: bool,
    },
    /// 채택된 스킬을 코딩 에이전트가 읽는 위치에 배치 — Claude 스킬 파일
    /// (`--as skill`) 또는 항상 켜진 컨텍스트 파일(`--as context`). 순수 파일 작업, LLM 없음.
    Deploy {
        /// <home>/projects/<name>/ 아래의 프로젝트 이름
        #[arg(long)]
        project: String,
        /// "skill" -> .claude/skills/<name>/SKILL.md ; "context" -> 컨텍스트 파일에 삽입.
        #[arg(long = "as", default_value = "skill")]
        as_kind: String,
        /// context: 대상 파일 (기본값 CLAUDE.md).
        #[arg(long)]
        to: Option<String>,
        /// skill: frontmatter `description` / 트리거 문구.
        #[arg(long)]
        desc: Option<String>,
        /// skill/블록 이름 (기본값: 프로젝트 이름).
        #[arg(long)]
        name: Option<String>,
        /// deploy 루트 (기본값: 프로젝트 .skillsmith/를 포함하는 git 레포).
        #[arg(long)]
        root: Option<String>,
        /// context: 에이전트 -> 파일 csv (claude=CLAUDE.md, codex=AGENTS.md, gemini=GEMINI.md).
        #[arg(long)]
        agents: Option<String>,
    },
}

/// 결정된 skillsmith 홈 + 번들 데모를 자동 시드할 수 있는지 여부.
struct Home {
    path: PathBuf,
    /// 전역 기본값(~/.skillsmith) 또는 명시적 홈만 데모를 자동 시드;
    /// 발견된 레포 로컬 `.skillsmith/`는 절대 시드하지 않음 (사용자 레포에
    /// 데모 fixture가 들어가는 것을 방지).
    auto_seed: bool,
}

/// 홈 디렉토리 결정: 명시적 `--home` > `$SKILLSMITH_HOME` > cwd 위로 탐색한
/// 레포 로컬 `.skillsmith/` > 사용자별 기본값 `~/.skillsmith`.
/// 레포 로컬 분기가 커밋된 레포 내 프로젝트를 bare `skillsmith run`으로 동작하게 함
/// (`.git`처럼); 사용자별 기본값은 env var 없이 어떤 cwd에서도 동작하게 함
/// (데모가 첫 실행 시 거기에 자동 시드됨).
fn resolve_home(opt: Option<String>) -> Home {
    if let Some(h) = opt {
        return Home { path: PathBuf::from(h), auto_seed: true };
    }
    if let Ok(h) = std::env::var("SKILLSMITH_HOME")
        && !h.is_empty()
    {
        return Home { path: PathBuf::from(h), auto_seed: true };
    }
    let global = std::env::var("HOME")
        .ok()
        .filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(h).join(".skillsmith"));
    // 전역 홈이 아닌 레포 로컬 `.skillsmith/`가 우선 (가장 가까운 것 우선).
    // 발견된 것이 ~/.skillsmith뿐이면 자동 시드를 위해 통과.
    if let Ok(cwd) = std::env::current_dir()
        && let Some(local) = skillsmith::config::discover_dot_skillsmith(&cwd)
        && Some(&local) != global.as_ref()
    {
        return Home { path: local, auto_seed: false };
    }
    if let Some(g) = global {
        return Home { path: g, auto_seed: true };
    }
    Home { path: PathBuf::from(".skillsmith"), auto_seed: true }
}

/// 최적의 DX로 `skillsmith new` 실행: 레포 로컬 vs 중앙을 자동 선택, 프로젝트 이름 자동 유도.
/// git 레포 안에서 bare `new` => 레포 이름을 따른 레포 로컬 프로젝트.
/// `--repo <path>`는 중앙 선택; 명시적 이름 또는 `--local`이 항상 우선.
fn cmd_new(home: &Home, name: Option<String>, repo: Option<String>, local_flag: bool) -> Result<()> {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    let git_root = skillsmith::config::enclosing_git_root(&cwd);
    // git 레포 안에서는 기본적으로 레포 로컬, --repo가 다른 곳을 가리키면 중앙.
    let local = local_flag || (repo.is_none() && git_root.is_some());
    let target_home = if local {
        git_root.clone().unwrap_or_else(|| cwd.clone()).join(".skillsmith")
    } else {
        home.path.clone()
    };
    // 이름 생략 시 유도: --repo의 디렉토리, 없으면 포함하는 git 레포 디렉토리.
    let name = match name {
        Some(n) => n,
        None => {
            let basis = repo
                .as_deref()
                .map(PathBuf::from)
                .or_else(|| if local { git_root.clone().or_else(|| Some(cwd.clone())) } else { None });
            basis
                .as_deref()
                .and_then(skillsmith::config::slug_from_path)
                .ok_or_else(|| {
                    anyhow::anyhow!(
                        "couldn't derive a project name — run inside a git repo, or pass one: \
`skillsmith new <name> [--repo <path>]`"
                    )
                })?
        }
    };
    skillsmith::optimize::new_project(&target_home, &name, repo.as_deref(), local)
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let plain = cli.plain;
    let home = resolve_home(cli.home);
    let path = home.path.as_path();
    // 전역/명시적 홈에서만 데모를 자동 시드, 발견된 레포 로컬 `.skillsmith/`에서는 절대 안 함
    // (데모 fixture가 레포에 들어가면 안 됨).
    let maybe_seed = || -> Result<()> {
        if home.auto_seed {
            skillsmith::seed::ensure_seeded(path)?;
        }
        Ok(())
    };
    match cli.cmd {
        Cmd::Init => skillsmith::seed::init(path),
        Cmd::Run { project, dry_run, watch } => {
            maybe_seed()?;
            if dry_run {
                skillsmith::optimize::dry_run(path, &project).await
            } else if watch {
                skillsmith::optimize::run_watch(path, &project, plain).await
            } else {
                skillsmith::optimize::run(path, &project, plain).await
            }
        }
        Cmd::Eval { project, watch } => {
            maybe_seed()?;
            if watch {
                skillsmith::optimize::eval_watch(path, &project, plain).await
            } else {
                skillsmith::optimize::eval_only(path, &project, plain).await
            }
        }
        Cmd::Check { project } => {
            maybe_seed()?;
            skillsmith::optimize::check(path, &project)
        }
        Cmd::Bench { project, seeds } => {
            maybe_seed()?;
            skillsmith::optimize::bench(path, &project, seeds, plain).await
        }
        Cmd::Adopt { project } => {
            maybe_seed()?;
            skillsmith::optimize::adopt(path, &project)
        }
        Cmd::List => {
            maybe_seed()?;
            skillsmith::optimize::list(path)
        }
        Cmd::New { name, repo, local } => cmd_new(&home, name, repo, local),
        Cmd::Deploy { project, as_kind, to, desc, name, root, agents } => {
            maybe_seed()?;
            skillsmith::deploy::deploy(
                path,
                &project,
                &skillsmith::deploy::DeployOpts { as_kind, to, desc, name, root, agents },
            )
        }
    }
}
