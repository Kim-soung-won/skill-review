# Skillsmith 도입 보류

## 배경

Skillsmith는 마크다운 스킬 문서를 LLM 기반으로 평가·개선하는 도구다.
React 공통 Design Component 스킬의 품질 검증 및 유지보수 자동화를 위해 검토했다.

## 보류 이유

스킬 유효성 검증(`eval`)과 최적화(`run`)는 LLM 호출이 필수다.

| 실행 환경 | 인증 방식 | 가능 여부 |
|---|---|---|
| 로컬 | Claude CLI 세션 | ✅ |
| Jenkins CI | `ANTHROPIC_API_KEY` | ❌ 미보유 |

팀 단위 자동화를 위해서는 Jenkins Credentials에 등록할 API Key가 필요하나 현재 확보되지 않았다.

## 현재 구조와의 충돌

기존 Jenkins 파이프라인은 RTL 테스트로 컴포넌트 정합성을 검증한다.
Skillsmith가 추가하려는 "스킬 문서가 에이전트를 올바르게 안내하는가"라는 검증 레이어는
LLM 비용이 수반되므로 CI 통합이 현실적으로 불가능하다.

## 재개 조건

- 팀 `ANTHROPIC_API_KEY` 확보
- Jenkins Credentials 등록 및 `provider = "genai"` 설정

## 보존 자산

검토 과정에서 생성한 프로젝트 구조와 픽스처는 `.skillsmith/projects/` 아래에 유지한다.
재개 시 `just eval <project>` 로 즉시 실행 가능한 상태다.
