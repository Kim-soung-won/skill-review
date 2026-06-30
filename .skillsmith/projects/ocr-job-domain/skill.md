---
name: ocr-job_domain
description: >-
  계층 무관하게 OcrDocument·ProcessingResult·MaskingResult가 무엇인지,
  1:N 관계와 불변조건이 무엇인지 알아야 할 때 읽는다.
  `src/features/`와 `src/entities/` 모두에서 타입을 참조할 때 기준 문서가 된다.
  API 명세는 [[ocr-job_api]], UI 흐름은 [[ocr-job_scenario]]를 읽는다.
metadata:
  type: domain-skill
  confidence:
    inferred:
      - topic: "KvItem.type 전체 리터럴 값"
        note: "실제 응답에서 'string' | 'date' | 'datetime' | 'number' | 'time' | 'boolean' 확인. 추가 값 존재 여부는 미확인. ocr-job_api 응답 샘플 기준 추론."
      - topic: "MaskingResult.maskedKv pending 시 빈 객체"
        note: "ocr-job_api 응답 샘플에서 status=pending이면 maskedKv: {} 빈 객체로 반환됨 확인. 런타임 파싱 시 KvResult 구조 guard 필요."
---

## 한 줄 정의

`/v1/documents` 기반 문서 분석 Job 도메인의 엔티티 구조, TypeScript 타입, 핵심 의사 결정.

> API 엔드포인트 명세는 [[ocr-job_api]] 참고.
> 처리 흐름·시나리오는 [[ocr-job_scenario]] 참고.
> 프로파일 도메인은 [[ocr-profile_domain]] 참고.

---

## 처리 파이프라인

문서를 업로드하면 내부적으로 다음 순서로 처리된다.

```
1. 파일 업로드
      ↓
2. 분류 (Classification)
      — 업로드된 문서가 어떤 OcrProfile에 속하는지 자동 판별
      — 매칭 프로파일이 있으면 → 해당 프로파일의 기본 스키마(defaultSchema)로 few-shot 실행
      — 매칭 프로파일 없음(unknown) → zero-shot 실행
      ↓
3. 분석 실행
      few-shot : 분류된 프로파일의 기본 스키마를 schema로 사용해 정밀 추출
      zero-shot: 스키마 없이 범용 분석
```

- `process=true` 쿼리 파라미터로 업로드와 동시에 파이프라인 자동 실행
- `process=false`(기본)이면 업로드만 수행 → 이후 별도 분석 요청 필요
- 전체 처리는 **비동기**. `processing.overallStatus` 폴링으로 완료 여부 확인

---

## 도메인 관계

OcrDocument는 OcrProfile에 **종속(N:1)**된다.

```
OcrProfile (1)
└── OcrDocument[] (N)   — profileName 필드로 연결 (없으면 null = unknown)
      └── ProcessingResult[] (1:N)
            └── MaskingResult (1:1)
```

- 분류 단계에서 프로파일이 결정되며, 이 값이 `Document.profileName`에 기록됨
- 프로파일이 결정되지 않은 문서(unknown)는 `profileName: null`

---

## 도메인 구조

```
OcrProfile (부모, [[ocr-profile_domain]] 참고)
│
└── Document (문서)
      ├── id            — UUID
      ├── fileName      — 원본 파일명
      ├── contentType   — MIME 타입 (e.g. image/png)
      ├── sizeBytes     — 파일 크기 (bytes)
      ├── pageCount     — 페이지 수
      ├── ownerUsername — 문서 소유자 (권한·공유는 userId 기준으로 동작)
      ├── profileName   — 분류 단계에서 결정된 OcrProfile명. 없으면 null (unknown)
      ├── processingType — "auto" | "zeroShot" | "fewShot"
      ├── processing    — 현재 처리 진행 상태. 실제 응답은 Processing 인터페이스 기준
      │     ├── overallStatus   — "completed" | "failed" | "pending"
      │     ├── startedAt / updatedAt
      │     ├── steps[]         — 내부 실행 단계 목록 (name, status, index, durationMs 등)
      │     └── progressSteps[] — 화면 표출용 그룹화 진행도 (label이 사용자 노출명)
      │
      └── ProcessingResult[] (1:N) — 분석 실행 결과
            ├── processingType — 'zeroShot' | 'fewShot' | 'auto'
            ├── status         — 'completed' | 'failed' | 'in_progress' | 'pending'
            ├── htmlContents   — 페이지별 HTML 배열 (처리 중이면 null)
            ├── kvResult       — { pages: KvPage[]; merged: { keyValues: KvItem[] } }
            │                    pages[n].keyValues: 페이지별 KV
            │                    merged.keyValues: 전체 통합 KV
            │                    (처리 중이면 null)
            ├── schemaUsed     — fewShot 시 사용된 스키마 {id, name} 단건
            │                    (zeroShot이면 null. 배열 아님)
            └── MaskingResult  — 마스킹 수행 후 조회 가능
                                 maskedKv 구조는 kvResult와 동일(KvResult)
```

연관 도메인:

- `OcrProfile` : 문서 분류의 기준이 되는 프로파일. `defaultSchemaName`이 few-shot 시 사용됨
- `/v1/schemas` : Few-shot 분석 시 참조하는 스키마 관리 API

---

## TypeScript 타입

```typescript
// processing 필드 타입 — 실제 목록 조회 응답 기준

type ProcessingType = "auto" | "zeroShot" | "fewShot";
type StepStatus = "pending" | "completed" | "failed";
type OverallStatus = "pending" | "completed" | "failed";

interface ProcessingStep {
  name: string;
  status: StepStatus;
  index: number;
  startedAt: string | null;
  endAt: string | null;
  changedAt: string | null;
  durationMs: number;
}

interface ProgressStep {
  name: string;
  label: string;    // 사용자에게 노출되는 단계명 (e.g. "문서 다운로드", "텍스트 분석")
  steps: string[]; // 해당 progressStep에 속한 내부 step name 목록
  status: StepStatus;
  startedAt: string | null;
  endAt: string | null;
  changedAt: string | null;
  durationMs: number;
}

interface Processing {
  startedAt: string | null;
  updatedAt: string | null;
  overallStatus: OverallStatus;
  steps: ProcessingStep[];
  progressSteps: ProgressStep[];
}

// OcrDocument (ocr-job.contracts.ts) — 목록 조회 응답: data.documents[] + data.pagination
interface OcrDocument {
  id: string;
  pipelineId: string | null;
  runId: string | null;
  fileName: string;
  contentType: string;
  sizeBytes: number;
  pageCount: number;
  profileId: string | null;
  profileName: string | null; // 분류 단계에서 결정. 미매칭이면 null (unknown)
  processingType: ProcessingType;
  processing: Processing;
  createdAt: string;
  ownerUsername: string;
}

// 목록 조회 응답 래퍼 구조
// { data: { documents: OcrDocument[]; pagination: Pagination } }

// ⚠️ 하위 호환: Zod 스키마(KnowledgeStorageStatusDtoSchema) 재사용 코드가 남아 있을 수 있음.
// 실제 응답 구조는 위 Processing 인터페이스 기준. 스키마 마이그레이션 필요 여부 확인 필요.

// 분석 결과 요약 (목록용)
type ProcessingResultSummary = {
  id: string;
  documentId: string;
  processingType: ProcessingType;
  createdAt: string;
};

// KV 구조 — ProcessingResult.kvResult 와 MaskingResult.maskedKv 가 동일 구조를 공유
// Key, Value 는 PascalCase (실제 응답 기준). type 은 소문자.
interface KvItem {
  Key: string;
  type: string; // e.g. "string" | "date" | "datetime" | "number" | "time" | "boolean"
  Value: string;
}

interface KvPage {
  pageNumber: number;
  keyValues: KvItem[];
}

interface KvResult {
  pages: KvPage[];
  merged: {
    keyValues: KvItem[];
  };
}

// 분석 결과 상세
// status 가 "in_progress" | "pending" 이면 htmlContents / kvResult 는 null
interface ProcessingResult {
  id: string;
  documentId: string;
  processingType: ProcessingType; // "auto" | "zeroShot" | "fewShot"
  status: string;                 // "completed" | "failed" | "in_progress" | "pending"
  htmlContents: string[] | null;  // 페이지별 HTML 배열. 처리 중이면 null
  kvResult: KvResult | null;      // 처리 중이면 null
  schemaUsed: { id: string; name: string } | null; // zeroShot: null / fewShot: 사용된 스키마 단건
  error: string | null;
  createdAt: string;
  updatedAt: string;
}

// 마스킹 결과 — maskedKv 는 KvResult 와 동일 구조
interface MaskingResult {
  id: string;
  documentId: string;
  extractionResultId: string; // 원본 ProcessingResult ID
  status: string;
  maskedKv: KvResult | {};    // pending 상태이면 빈 객체 {}
  sensitiveCount: number;     // 마스킹된 항목 수
  pageCount: number;
  error: string | null;
  createdAt: string;
}
```

---

## 핵심 제약사항

- `ProcessingResult.kvResult`와 `MaskingResult.maskedKv`는 `KvResult` 구조를 공유한다. `maskedKv`는 pending 상태이면 빈 객체 `{}`로 반환되므로 접근 전 구조 guard 필요.
- `KvItem.Key`, `KvItem.Value`는 PascalCase다. 소문자 `key` / `value`로 접근하면 `undefined` 반환.
- `ProcessingResult.htmlContents`와 `kvResult`는 `status`가 `"in_progress"` 또는 `"pending"`이면 `null`이다.
- `schemaUsed`는 단건 객체 또는 `null`이다. 배열 처리 금지.
- 동일 타입(`KvItem`, `KvPage`, `KvResult`)을 `_api`·`_scenario` 스킬에 재정의하지 않는다. 이 스킬(`ocr-job_domain`)이 타입 권위 소스다.

---

## 핵심 의사 결정

| 관심사 | 결정 | 이유 |
|--------|------|------|
| OcrDocument ↔ OcrProfile 관계 | N:1 (Document가 Profile에 종속) | 분류 단계에서 프로파일이 결정되며, 한 문서는 하나의 프로파일에만 속함. 분류 실패 시 `profileName: null` |
| 분석 방식 결정 기준 | 분류 결과에 따라 자동 분기 | 프로파일 매칭 → few-shot(프로파일 기본 스키마 사용); 미매칭 → zero-shot |
| 분석 방식 선택 | Zero-shot vs Few-shot | Zero-shot은 추가 입력 없이 범용 분석; Few-shot은 분류된 프로파일의 기본 스키마로 정밀 추출 수행 |
| 업로드 시 자동 분석 (`process`) | `process=true` 쿼리 파라미터 | 업로드 즉시 자동 파이프라인(분류 → few-shot 또는 zero-shot) 실행. `false`(기본)이면 업로드만 수행 후 별도 분석 요청 필요 |
| 권한 기준 | userId (string) | 문서 소유·공유 모두 userId 단위로 동작 |
| `schemaUsed` 필드 | `{id, name} \| null` (단건) | zeroShot이면 null, fewShot이면 사용된 스키마 단건 객체. 배열 아님 |
| `kvResult` 구조 | `KvResult` (pages + merged 객체) | pages는 페이지별 KV 배열, merged는 전체 통합 KV. MaskingResult.maskedKv도 동일 구조 공유 |
| `KvItem` 필드 케이싱 | `Key`, `Value`는 PascalCase; `type`은 소문자 | 실제 응답 기준. 렌더링 시 `item.Key` / `item.Value` 로 접근 |
| `ProcessingResult.status` | `string` (`"completed" \| "failed" \| "in_progress" \| "pending"`) | in_progress / pending 상태이면 htmlContents, kvResult 모두 null |
| `ProcessingResult.updatedAt` | `string` (ISO 8601) | 실제 응답에 포함 확인 |
| `processing` 타입 | `Processing` 인터페이스 (실제 응답 기준) | 실제 응답에서 steps[].endAt, progressSteps[].endAt 필드 포함 확인. 기존 KnowledgeStorageStatusDtoSchema 재사용 코드는 마이그레이션 검토 필요 |
| 분석 응답 방식 | 비동기 즉시 반환 + 폴링 | 분석 소요 시간(2~30초)이 HTTP 타임아웃 리스크를 유발하므로 비동기로 전환 |
| 폴링 기준 필드 | `processing.overallStatus` | 전체 완료 여부 판단에 사용; UI 단계 표시는 `progressSteps[].status` + `label` 사용 |
