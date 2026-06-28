# CLAUDE.md

이 파일은 이 저장소에서 작업하는 Claude Code(claude.ai/code)에게 가이드를 제공합니다.

## 명령어

**빌드 / 실행** ([`just`](https://just.systems)와 Rust 1.85+, Edition 2024 필요):

```sh
just build          # 릴리즈 빌드
just run demo       # 번들 데모 전체 최적화 루프 실행
just eval demo      # 최적화 없이 평가 1회만 실행
just install        # 엔진 + 모든 에이전트 통합 설치
just install-bin    # 바이너리를 ~/.cargo/bin/skillsmith 에 설치
just init           # 번들 데모 재시드
just validate       # Claude Code + 플러그인 매니페스트 검증
just list           # 발견된 프로젝트 목록 출력
```

**CLI 서브커맨드** (`just install-bin` 이후 사용):

```sh
skillsmith run   --project <name> [--dry-run] [--watch]
skillsmith eval  --project <name> [--watch]
skillsmith check --project <name>          # 드리프트 감지
skillsmith bench --project <name> [--seeds N]
skillsmith adopt --project <name>          # staged → live 복사 (라이브 파일을 건드리는 유일한 단계)
skillsmith deploy --project <name> [--as skill|context]
skillsmith new   [name] [--repo <path>]
skillsmith list
```

**테스트:**

```sh
cargo test                        # 전체 통합 테스트
cargo test <test_name>            # 단일 테스트 (tests/core.rs)
```

## 아키텍처

Skillsmith는 **평가 게이트 기반 스킬 옵티마이저**다. 마크다운 지침 문서("스킬")를 개선하기 위해, 제안된 편집이 코딩 에이전트의 출력을 더 많은 테스트에 통과시키는지를 측정한다. 대상 저장소의 실제 테스트 스위트를 오라클로 사용하며, 격리된 git worktree 안에서 실행된다.

### 핵심 루프 (`src/optimize.rs`)

```
baseline eval (현재 스킬, 전체 train/val 태스크)
  → 각 라운드마다:
      propose  — 옵티마이저 LLM이 후보 스킬 편집 제안
      eval     — 에이전트 LLM이 파일 편집; ExecJudge가 worktree에서 verify_cmd 실행
      gate     — candidate_score > best_score (엄격한 초과)인 경우에만 수락
      stage    — 수락할 때마다 skill.staged.md에 증분 기록
  → (선택적) test 스플릿을 루프 종료 후 1회만 평가 (편향 없음)
```

개선 사항은 매 수락 직후 기록되므로 중간에 오류가 발생해도 이전 성과가 사라지지 않는다. `adopt`는 라이브 스킬 파일을 실제로 변경하는 유일한 단계다.

### 포트 & 어댑터

`src/lib.rs`에 두 개의 확장 포트가 있다:

| 포트 | 어댑터 |
|------|--------|
| **`LlmProvider`** (`src/llm.rs`) | `CliProvider` (claude/codex/gemini CLI를 셸 호출 — API 키 불필요), `GenaiProvider` (genai 크레이트 — 환경변수 키 필요), 커스텀 `provider_cmd` |
| **`Judge`** (`src/judge.rs`) | `ExecJudge` (git worktree에서 편집 적용 후 `verify_cmd` 실행, 통과 테스트 비율로 점수화) |

프로바이더 티어링: 빈번한 저비용 eval 호출엔 `agent_model` / `agent_provider_cmd`, 소수의 고비용 propose 호출엔 `optimizer_model` / `optimizer_provider_cmd`.

### 모듈 맵

| 모듈 | 역할 |
|------|------|
| `src/optimize.rs` | 메인 루프; 프로바이더/judge 연결; baseline → 라운드 → 리포트/스테이징 |
| `src/eval.rs` | 태스크 목록에 대해 eval 1회 실행; 태스크별 결과 반환 |
| `src/agent.rs` | 에이전트 프롬프트 생성; `<<<FILE: path>>> … <<<END>>>` 편집 블록 파싱 |
| `src/judge.rs` | worktree에서 편집 적용; verify_cmd 실행; 채점 (pytest/unittest 출력 파싱, 폴백은 exit code) |
| `src/llm.rs` | `LlmProvider` 트레이트 + CliProvider / GenaiProvider 어댑터 |
| `src/config.rs` | `ProjectConfig` TOML 파싱; 태스크/스플릿 모델; 프로젝트 탐색 |
| `src/worktree.rs` | 태스크별 `git worktree add --detach HEAD` (격리된 병렬 샌드박스) |
| `src/deploy.rs` | adopt 이후 배치: SKILL.md로 래핑하거나 CLAUDE.md/AGENTS.md/GEMINI.md에 주입 |
| `src/bench.rs` | k-시드 분산: 최적화를 N회 실행해 평균 ± 표준편차 스코어카드 집계 |
| `src/obs.rs` | 구조화된 `Event` 스트림; 사람이 읽을 수 있는 렌더러; TTY vs 파이프 색상 처리 |
| `src/seed.rs` | 번들 데모 픽스처를 실제 git 레포로 구체화 |
| `src/main.rs` | CLI 진입점; 홈 경로 결정 (`--home` > `$SKILLSMITH_HOME` > 레포 로컬 `.skillsmith/` > `~/.skillsmith`) |

### 프로젝트 레이아웃 (프로젝트별)

```
projects/<name>/
  config.toml        # 태스크, 프로바이더, 라운드 수, verify_cmd, deploy 기본값
  skill.md           # 시드 / 라이브 스킬 (커밋됨, 버전 관리)
  skill.staged.md    # 제안된 개선안 (gitignored, adopt 전)
  report.md          # 사람이 읽을 수 있는 실행 리포트 (gitignored)
  results.json       # 머신 리더블 결과 (gitignored)
  .last-run          # 드리프트 감지용 설정 지문
  fixture/           # (선택적) 합성 eval 샌드박스 — 실제 git 레포여야 함
```

### 에이전트 편집 프로토콜

에이전트는 파일 전체를 교체하는 방식으로 편집을 작성한다:
```
<<<FILE: relative/path.ext>>>
<전체 파일 내용>
<<<END>>>
```
한 응답에 여러 블록이 있으면 순서대로 파싱된다. 주변 산문은 무시되며, 퍼지 diff 없이 결정론적으로 채점된다.

### 채점

연속 점수 ∈ [0,1]:
1. pytest/unittest 요약 파싱 — `score = passed / (passed + failed + errors)`
2. 폴백: 바이너리 exit code (0 → 1.0, 그 외 → 0.0)

게이트는 **엄격한 초과(strict greater-than)**: 동점은 거부되어 장황함으로의 드리프트를 방지한다.

### 에이전트 통합 (Claude Code 플러그인, Codex, Gemini)

플러그인 매니페스트와 스킬은 `plugins/` 아래에 있다. `plugins/skillsmith/skills/skillsmith/SKILL.md`는 Claude와 Codex 양쪽에서 사용된다. Gemini는 TOML 커스텀 커맨드(`plugins/gemini/skillsmith.toml`)를 사용한다. 플러그인을 호출하기 전에 반드시 엔진 바이너리를 별도로 설치해야 한다.
