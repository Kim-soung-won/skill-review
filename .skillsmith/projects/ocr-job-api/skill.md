---
name: ocr-job_api
description: >-
  `src/entities/ocr/ocr-job/` 또는 `src/shared/api/`에서 `/v1/documents`
  엔드포인트(업로드·분석·마스킹·다운로드)를 구현·수정할 때 읽는다.
  API 경로·파라미터·응답 구조가 필요할 때 트리거한다.
  도메인 타입은 [[ocr-job_domain]], UI 흐름은 [[ocr-job_scenario]]를 읽는다.
metadata:
  type: domain-skill
---

## 한 줄 정의

`/v1/documents` 기반의 문서 업로드·분석·마스킹 처리 Job API 엔드포인트 명세.

> 도메인 구조·타입은 [[ocr-job_domain]] 참고.
> 처리 흐름·시나리오는 [[ocr-job_scenario]] 참고.

---

## 공통 규칙

모든 요청(Request Body · Query Parameters)과 응답(Response) 데이터 필드명은 **camelCase**를 사용한다.  
단, `processing.steps[].name` · `processing.progressSteps[].name`의 **값**(식별자 문자열, e.g. `download-document`)은 document-ai 공유 도메인 식별자이므로 camelCase 규칙에서 제외한다.

---

## API 명세

### 문서 업로드

```
POST /v1/documents?process={boolean}
Content-Type: multipart/form-data
Authorization: Bearer {token}
```

Query Parameters:

| 파라미터 | 타입 | 필수 | 기본값 | 설명 |
|----------|------|------|--------|------|
| `process` | boolean | 선택 | `false` | `true`이면 업로드 직후 자동 처리 파이프라인을 비동기로 실행 |

Request Body (`multipart/form-data`):

| 필드 | 타입 | 설명 |
|------|------|------|
| `files` | `File[]` | 업로드할 문서 파일 배열. `FormData.append('files', file)`로 복수 추가 (빈 파일 시 `400`) |

- 파일은 MinIO에 `users/{userId}/documents/...` prefix로 저장되어 사용자별로 분리됨
- 업로드한 사용자가 문서 소유자가 되며, 소유자만 삭제·권한 관리 가능
- 응답 `data`에 생성된 문서 `id`와 `fileName` 반환
- `process=true`이면 응답 `data.resultId`에 처리 결과 ID 추가 반환

**`process=true` 자동 파이프라인 동작:**
- 문서 분류 후 매칭되는 기본 스키마가 있으면 → few-shot 처리
- 매칭 스키마 없음(unknown) → zero-shot 처리
- 비동기 실행이므로 `GET /v1/documents` 폴링으로 진행도 추적

---

### 문서 목록 조회

```
GET /v1/documents?page={number}&size={number}&keyword={string}&sortBy={string}&order={string}
```

Query Parameters:

| 파라미터 | 타입 | 필수 | 설명 |
|----------|------|------|------|
| `page` | number | 필수 | 페이지 번호 (1-based) |
| `size` | number | 필수 | 페이지당 항목 수 |
| `keyword` | string | 선택 | 파일명 부분일치 검색 (대소문자 무시). `name`과 동일 |
| `name` | string | 선택 | 파일명 부분일치 검색 (대소문자 무시). `keyword`와 동일 |
| `sortBy` | string | 선택 | 정렬 기준 필드. 현재 허용값: `createdAt`. 허용되지 않은 값은 기본값으로 대체 |
| `order` | string | 선택 | 정렬 방향. `ASC` 또는 `DESC`. 동일 정렬값은 `id` 역순으로 안정 정렬 |

Response:

```json
{
  "success": true,
  "data": {
    "documents": [
      {
        "id": "e207532c-bccc-41d1-ab8c-49487820a44d",
        "pipelineId": null,
        "runId": null,
        "fileName": "[별지 11] 위임장(개인정보 처리 방법에 관한 고시).png",
        "contentType": "image/png",
        "sizeBytes": 53411,
        "pageCount": 1,
        "profileId": null,
        "profileName": null,
        "processingType": "zeroShot",
        "processing": {
          "startedAt": "2026-06-16T11:23:28.638159+09:00",
          "updatedAt": "2026-06-16T11:23:40.797376+09:00",
          "overallStatus": "completed",
          "steps": [
            {
              "name": "download-document",
              "status": "completed",
              "index": 0,
              "startedAt": "2026-06-16T11:23:28.641732+09:00",
              "changedAt": "2026-06-16T11:23:28.646702+09:00",
              "durationMs": 4
            }
          ],
          "progressSteps": [
            {
              "name": "document-loader",
              "label": "문서 준비",
              "steps": ["download-document"],
              "status": "completed",
              "changedAt": "2026-06-16T11:23:28.646702+09:00",
              "durationMs": 4
            }
          ]
        },
        "createdAt": "2026-06-16T02:23:26.584963+00:00",
        "ownerUsername": "dev"
      }
    ],
    "pagination": {
      "length": 1,
      "size": 10,
      "page": 1,
      "lastPage": 1,
      "startIndex": 1,
      "endIndex": 1
    }
  },
  "message": null
}
```

주요 필드 설명:

| 필드 | 타입 | 설명 |
|------|------|------|
| `pipelineId` | `string \| null` | 연결된 파이프라인 ID. 없으면 `null` |
| `runId` | `string \| null` | 파이프라인 실행 ID. 없으면 `null` |
| `profileId` | `string \| null` | 연결된 OCR 프로파일 ID. 없으면 `null`. Few-shot 스키마 목록 조회 시 사용 |
| `profileName` | `string \| null` | 연결된 OCR 프로파일명. 없으면 `null` |
| `processingType` | `"zeroShot" \| "fewShot" \| "auto"` | 분석 유형 |
| `processing` | `object \| null` | 처리 진행 상태. 분석 미실행 시 `null` |
| `processing.overallStatus` | `"completed" \| "failed" \| "in_progress" \| "pending"` | 전체 처리 상태 |
| `processing.progressSteps[].label` | `string` | **화면 표출용** 단계명 (e.g. "문서 준비", "텍스트 분석", "정보 추출", "결과 저장") |

---

### 문서 삭제

```
DELETE /v1/documents/{document_id}
```

> 삭제 시 연결된 `ProcessingResult`도 함께 삭제 → 결과 목록 캐시 무효화 필요.

---

### 문서 분석 실행

분석 결과로 kv, html을 생성. 요청 즉시 응답 반환(비동기), 폴링으로 진행도 추적.

#### Zero-shot

```
POST /v1/documents/{document_id}/processing/zeroShot
```

#### Few-shot

```
POST /v1/documents/{document_id}/processing/fewShot
```

---

### 분석 결과 목록 조회

```
GET /v1/documents/{document_id}/processing/results?page={page}&size={size}
```

Response:

```json
{
  "success": true,
  "data": {
    "results": [
      {
        "id": "a038a3a5-ff8e-4a23-8aef-606dcaa4c538",
        "documentId": "549ef677-4ad8-41e6-8673-8b2f7778f2db",
        "processingType": "auto",
        "status": "in_progress",
        "error": null,
        "createdAt": "2026-06-16T08:07:13.417399+00:00",
        "updatedAt": "2026-06-16T08:07:13.868203+00:00"
      }
    ],
    "pagination": {
      "length": 1,
      "size": 100,
      "page": 1,
      "lastPage": 1,
      "startIndex": 1,
      "endIndex": 1
    }
  },
  "message": null
}
```

주요 필드 설명:

| 필드 | 타입 | 설명 |
|------|------|------|
| `processingType` | `"zeroShot" \| "fewShot" \| "auto"` | 분석 유형 |
| `status` | `"completed" \| "failed" \| "in_progress" \| "pending"` | 처리 상태 |
| `error` | `string \| null` | 처리 실패 시 오류 메시지 |
| `updatedAt` | `string` | 마지막 업데이트 시각 (ISO 8601) |

---

### 분석 결과 단건 조회

```
GET /v1/documents/{documentId}/processing/results/{resultId}
```

Path Parameters:

| 파라미터 | 타입 | 설명 |
|----------|------|------|
| `documentId` | `string` | 문서 ID |
| `resultId` | `string` | 분석 결과 ID |

Response:

```json
{
  "success": true,
  "data": {
    "id": "b97dbf1d-...",
    "documentId": "ee484b2d-...",
    "processingType": "fewShot",
    "status": "completed",
    "htmlContents": ["<html>...</html>"],
    "kvResult": {
      "pages": [
        { "pageNumber": 1, "keyValues": [] }
      ],
      "merged": {
        "keyValues": []
      }
    },
    "schemaUsed": {
      "id": "995ce28b-...",
      "name": "E2E 추출 스키마"
    },
    "error": null,
    "createdAt": "2026-06-22T08:52:17.131179+00:00",
    "updatedAt": "2026-06-22T08:52:31.996820+00:00"
  },
  "message": null
}
```

주요 필드 설명:

| 필드 | 타입 | 설명 |
|------|------|------|
| `status` | `"completed" \| "failed" \| "in_progress" \| "pending"` | 처리 상태. `in_progress`/`pending` 중이면 `htmlContents`/`kvResult` 모두 `null` |
| `htmlContents` | `string[]` | 페이지별 HTML 변환 결과 배열 (index = 페이지 순서) |
| `kvResult.pages` | `{ pageNumber: number; keyValues: KvItem[] }[]` | 페이지별 KV. `pageNumber`로 미리보기 페이지와 연동 |
| `kvResult.pages[].keyValues[]` | `{ Key: string; type: string; Value: string }` | Key/Value는 PascalCase |
| `kvResult.merged` | `{ keyValues: KvItem[] }` | 전체 페이지 통합 KV. 전체 결과 탭 표출 시 사용 |
| `schemaUsed` | `{ id: string; name: string } \| null` | 단건 객체. zeroShot이면 `null`, fewShot이면 사용된 스키마. 배열 아님. 타입 정의는 [[ocr-job_domain]] 참고 |
| `error` | `string \| null` | 처리 실패 시 오류 메시지 |
| `updatedAt` | `string` | 마지막 업데이트 시각 (ISO 8601) |

---

### 마스킹 실행

```
POST /v1/documents/{document_id}/processing/results/{result_id}/mask
```

### 마스킹 결과 조회

```
GET /v1/documents/{document_id}/processing/results/{result_id}/masking
```

Response:

**pending (마스킹 진행 중) — `maskedKv`가 빈 객체**

```json
{
  "success": true,
  "data": {
    "results": [
      {
        "id": "b0d0b33a-baad-465c-bbd7-30ea1370a8ba",
        "documentId": "47388b06-7d7e-434e-9e86-43da3bb991a9",
        "extractionResultId": "1d7071d4-7e50-4f09-9c9e-97df8ec4eeec",
        "status": "pending",
        "maskedKv": {},
        "sensitiveCount": 0,
        "pageCount": 0,
        "error": null,
        "createdAt": "2026-06-17T00:52:54.001477+00:00"
      }
    ]
  },
  "message": null
}
```

**completed (마스킹 완료) — `maskedKv`에 pages/merged 포함**

```json
{
  "success": true,
  "data": {
    "results": [
      {
        "id": "d5a20d06-0a31-45cd-88fa-030dad3cd838",
        "documentId": "47388b06-7d7e-434e-9e86-43da3bb991a9",
        "extractionResultId": "1d7071d4-7e50-4f09-9c9e-97df8ec4eeec",
        "status": "completed",
        "maskedKv": {
          "pages": [
            {
              "pageNumber": 1,
              "keyValues": [
                { "Key": "문서 제목", "type": "string", "Value": "위임장" },
                { "Key": "위임자 서명", "type": "string", "Value": "" },
                { "Key": "작성일", "type": "date", "Value": "년 월 일" }
              ]
            }
          ],
          "merged": {
            "keyValues": [
              { "Key": "문서 제목", "type": "string", "Value": "위임장" },
              { "Key": "위임자 서명", "type": "string", "Value": "" },
              { "Key": "작성일", "type": "date", "Value": "년 월 일" }
            ]
          }
        },
        "sensitiveCount": 3,
        "pageCount": 1,
        "error": null,
        "createdAt": "2026-06-17T00:52:10.443001+00:00"
      }
    ]
  },
  "message": null
}
```

주요 필드 설명:

| 필드 | 타입 | 설명 |
|------|------|------|
| `status` | `"pending" \| "completed" \| "failed" \| "in_progress"` | 마스킹 처리 상태 |
| `maskedKv` | `{ pages?, merged? } \| {}` | 진행 중(`pending`)이면 빈 객체 `{}`. 완료 시 `pages`·`merged` 포함 |
| `sensitiveCount` | `number` | 마스킹된 민감 항목 수. 진행 중이면 `0` |
| `pageCount` | `number` | 처리된 페이지 수. 진행 중이면 `0` |
| `error` | `string \| null` | 실패 시 오류 메시지 |

- `results[]` 배열 형태지만 추출 결과 1건당 마스킹 결과는 1건
- 민감 항목 판별: `Value === "OOO"`
- `status === "pending"` 일 때 `maskedKv.pages`/`maskedKv.merged`는 존재하지 않음 — UI에서 `status` 체크 후 접근할 것

---

### 마스킹 결과 삭제

```
DELETE /v1/documents/{documentId}/processing/results/{resultId}/masking/{maskingId}
```

Path Parameters:

| 파라미터 | 타입 | 설명 |
|----------|------|------|
| `documentId` | `string` | 문서 ID |
| `resultId` | `string` | 분석 결과 ID |
| `maskingId` | `string` | 마스킹 결과 ID (`results[].id`) |

Response:

```json
{
  "success": true,
  "data": null,
  "message": null
}
```

- 삭제 후 복구 불가
- 삭제 성공 시 `data: null`인 `StandardResponse` 반환

---

### 분석 결과 ZIP 다운로드

```
GET /v1/documents/{document_id}/processing/results/{result_id}/download
```

Query Parameters:

| 파라미터 | 타입 | 기본값 | 설명 |
|----------|------|--------|------|
| `includeHtml` | `boolean` | `true` | HTML 파싱 결과 포함 (`html/page_001.html`) |
| `includeKv` | `boolean` | `true` | KV 추출 결과 포함 (`kv.json`) |
| `includeMasking` | `boolean` | `true` | 최신 마스킹 결과 포함 (`masking/`) |

- Response: `application/zip` blob
- 문서 접근 권한이 있는 사용자만 호출 가능

ZIP 구조:

```text
{document_id}/
  html/page_001.html   ← includeHtml=true
  kv.json              ← includeKv=true
  masking/
    masked_kv.json     ← includeMasking=true
    pages/page_001.png ← 마스킹 completed 시에만 포함
```

마스킹 결과 선택 규칙:

| 최신 마스킹 상태 | ZIP 동작 |
|---|---|
| 없음 | 빈 `masked_kv.json` |
| `pending` / `in_progress` / `failed` | 빈 `masked_kv.json` |
| `completed` | `masked_kv.json` + `pages/*.png` 포함 |

> 마스킹 결과가 없거나 completed가 아니어도 다운로드는 실패하지 않는다.

UI 케이스별 파라미터:

| 버튼 | `includeHtml` | `includeKv` | `includeMasking` |
|------|:---:|:---:|:---:|
| 전체 다운로드 | `true` | `true` | `true` |
| HTML 다운로드 | `true` | `false` | `false` |
| KV 다운로드 | `false` | `true` | `false` |
| 마스킹 결과 다운로드 | `false` | `false` | `true` |

TypeScript 타입:

```typescript
type OcrDownloadQuery = {
  includeHtml?: boolean;
  includeKv?: boolean;
  includeMasking?: boolean;
};

type OcrResultDownloadQuery = {
  documentId: string;
  resultId: string;
  includeHtml?: boolean;
  includeKv?: boolean;
  includeMasking?: boolean;
};
```

---

### 마스킹 이미지 페이지 조회

```
GET /v1/documents/{document_id}/processing/masking/{masking_id}/pages/{page_num}
```

- Response: `image/png` blob
- `masking_id`: 마스킹 결과 조회 응답의 `results[].id`
- 페이지 번호는 1-based

---

### 문서 페이지 미리보기 목록

```
GET /v1/documents/{document_id}/pages/preview
```

- Response: `multipart/mixed; boundary={uuid}` 형식
- 각 파트 헤더에 `X-Document-Id`와 `X-Page`(1부터 시작) 포함
- `responseType: 'arraybuffer'`로 수신 후 `parseMultipartMixed(buffer, contentType)`으로 ObjectURL[] 변환
- `src/shared/lib/multipart/index.ts` 유틸 사용

---

### 문서 페이지 원본 미리보기 (단건)

```
GET /v1/documents/{document_id}/pages/{page}/preview
```

- Response: 이미지 blob
- 특정 페이지를 원본 크기로 조회할 때 사용
