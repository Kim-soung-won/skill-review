---
name: ocr-job_scenario
description: >-
  `src/features/ocr/` 또는 `src/pages/ocr/`에서 문서 업로드→분석→결과→마스킹
  UI 흐름, 폴링 전략, 미리보기·KV 연동을 구현·수정할 때 읽는다.
  타입·불변조건은 [[ocr-job_domain]], API 명세는 [[ocr-job_api]]를 읽는다.
metadata:
  type: domain-skill
---

## 한 줄 정의

OCR 문서 분석 Job의 업로드→분석→결과→마스킹 전체 처리 흐름과 제약사항.

> API 엔드포인트 명세는 [[ocr-job_api]] 참고.
> 도메인 구조·타입은 [[ocr-job_domain]] 참고.

---

## UI 통합 뷰 시나리오 (미리보기 + 분석 결과)

문서를 선택하면 미리보기와 분석 결과가 하나의 화면에 함께 표시된다.

```
┌──────────────┬───────────────────────┬──────────────────────────┐
│ 미리보기 목록 │   선택 페이지 원본 이미지  │  분석 결과 (탭 전환)        │
│  (썸네일)    │   (중앙 전체 표출)       │  ├─ HTML 렌더링           │
│  page 1 ◀   │                       │  ├─ KV 추출 결과           │
│  page 2     │                       │  └─ 마스킹 처리 결과        │
│  page 3     │                       │                          │
└──────────────┴───────────────────────┴──────────────────────────┘
```

- 미리보기 썸네일 선택 → `GET /v1/documents/{id}/pages/{page}/preview` 로 원본 이미지 조회
- 선택된 페이지 번호(1-based)를 상태로 유지하여 KV 결과 연동에 사용
- KV 추출 결과: `kvResult.pages`에서 `pageNumber === selectedPage`인 항목의 `keyValues` 표출
- 전체 KV 탭: `kvResult.merged.keyValues` 표출
- HTML 결과: `htmlContents` 배열을 페이지 순서대로 렌더링

---

## 문서 분석 전체 흐름 (비동기 방식)

```
1. 문서 업로드 (부모 문서 등록)
   ├─ POST /v1/documents                   (process=false 기본 — 업로드만)
   └─ POST /v1/documents?process=true      (업로드 즉시 자동 파이프라인 실행)
        ├─ 분류 → 기본 스키마 매칭 시 → few-shot 비동기 실행
        ├─ 매칭 없음(unknown)              → zero-shot 비동기 실행
        └─ 응답 data.resultId에 처리 결과 ID 반환

2. 분석 실행 → 즉시 응답 반환 (비동기, process=false로 업로드한 경우 별도 요청)
   ├─ Zero-shot: POST /v1/documents/{id}/processing/zero-shot
   └─ Few-shot:  POST /v1/documents/{id}/processing/few-shot

3. 문서 목록 폴링 (Knowledge Storage와 동일한 패턴)
   └─ GET /v1/documents?...
      ├─ processing.progressSteps[].status 로 UI 진행도 표시
      │   (label 필드가 사용자에게 노출되는 단계명)
      ├─ overallStatus === "completed" → 폴링 중단, 결과 조회 가능
      └─ overallStatus === "failed"    → 폴링 중단, 에러 표시

4. 결과 목록 조회
   └─ GET /v1/documents/{id}/processing/results

5. 결과 상세 조회
   └─ GET /v1/documents/{id}/processing/results/{result_id}
      ├─ htmlContents: HTML 렌더링
      ├─ kvResult.pages[n].keyValues: 선택된 페이지(pageNumber 일치)의 KV 렌더링
      └─ kvResult.merged.keyValues: 전체 페이지 통합 KV 렌더링

6. 마스킹 (선택, 분석 결과 존재 시에만 가능)
   ├─ POST /v1/documents/{id}/processing/results/{result_id}/mask
   └─ GET  /v1/documents/{id}/processing/results/{result_id}/masking
```

---

## Zero-shot vs Few-shot 비교

| 구분 | Zero-shot | Few-shot |
|------|-----------|----------|
| 추가 입력 | 없음 | schema-id 지정 |
| 생성 결과 | 문서명, 세부사항, html, kv | 문서명, 세부사항, html, kv, kv-schema |
| `schemaUsed` | `null` | `{id, name}` 단일 객체 |
| 용도 | 범용 분석 | 스키마 기반 정밀 추출 |

---

## 핵심 제약사항

- 권한·공유는 `userId` (string) 기준으로 동작한다
- **분석은 비동기 처리**: 분석 요청 후 즉시 응답이 반환되므로 `GET /v1/documents` 폴링으로 `processing.overallStatus`가 `"completed"` 또는 `"failed"`가 될 때까지 진행도를 추적해야 한다
- UI 진행도 표시는 `processing.progressSteps[].status`를 기준으로 하며, 사용자에게는 `label` 필드 값을 노출한다 (e.g. "문서 준비", "텍스트 분석", "정보 추출", "결과 저장")
- `schemaUsed`는 `zeroShot`이면 `null`, `fewShot`이면 `{ id: string; name: string }` 단일 객체다; null 여부로 스키마 사용 여부를 판단한다
- `kvResult.pages[n].keyValues`의 `type` 필드(`'string' | 'date' | 'datetime' | 'number' | 'time' | 'boolean'`)로 값 포맷을 구분해 렌더링한다; `boolean` 타입은 `Value`가 `boolean` 원시값으로 올 수 있으므로 `String(value)` 변환 필요
- 선택된 미리보기 페이지에 해당하는 KV는 `kvResult.pages.find(p => p.pageNumber === selectedPage)?.keyValues`로 조회한다; 전체 통합 KV는 `kvResult.merged.keyValues`를 사용한다
- 마스킹은 분석 결과(`ProcessingResult`)가 존재할 때만 수행 가능하다
- 문서 목록·분석 결과 목록은 페이지네이션(`page`, `size`) 파라미터를 사용한다
- 미리보기 목록은 `multipart/mixed` 형식이므로 `responseType: 'arraybuffer'` + `parseMultipartMixed` 유틸 필요 (`src/shared/lib/multipart/index.ts`)
- 각 파트의 `X-Page` 헤더가 1-based 페이지 번호를 나타내며, 응답 순서와 일치함
- 문서 삭제 시 연결된 `ProcessingResult`도 함께 삭제되므로 결과 목록 캐시 무효화 필요
