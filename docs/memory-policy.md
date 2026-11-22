# 메모리 정책 및 상태 저장 아키텍처 (`MC-12`)

이 문서는 Mighty Code의 상태 저장소(`workspace-state.json` 인덱스 + `sessions/<stem>.json` 본문 + 증분 저널 + 부속 캐시/스냅샷)의 메모리 수명주기와 보존·가비지 컬렉션(GC) 정책을 정의합니다.

- **관련 일감**: `MC-12` (메모리 정책 정비), `MC-13` (원격 상태 동기화 고도화), `MC-14` (`mighty-workspace` 교차 워크스페이스 조회)
- **관련 계획**: [[2026-11 사이클 계획 (11/22 ~ 11/30)]]

---

## 1. 기존 구조의 3대 병목 진단

1. **유휴 창 메모리 미회수 (Unbounded Hot Body Residency)**
   - 분할 저장소 도입 이후 시작 시 비활성 워크스페이스는 지연 로드(`body_on_disk = true`)되지만, 한 번 열람하거나 실행한 창 본문(`logs`, `graphRuns`, `planHistory`, `todoProgress`, `backgroundWork`, `shellRuns`)은 다른 워크스페이스로 이동하거나 유휴 상태가 되어도 종료 시까지 RAM에 계속 상주했습니다.
   - 호스트 계층의 `history.rs`(`SessionHistoryState`, 최대 8 MiB + `retained` 참조)와 `panes.rs`(`PaneUi.seen`의 `Arc<RunSession>`) 및 `Store.written` / `PaneCache`도 동일 본문 `Arc`를 붙잡고 있어 실제 힙 회수가 일어나지 않았으며, `UsageLedger.last`에도 닫힌 창의 누적 기준점이 계속 남았습니다.

2. **반복적인 전체 직렬화·덮어쓰기 및 정규화 비용 (Write Amplification & Re-normalization)**
   - 에이전트 실행 중(`status == "running"`) 1.5초 주기 저장(`Saver::schedule`)마다 수 MB 규모의 `sessions/<stem>.json` 전체를 다시 직렬화하고 `.bak` 파일까지 바이트 복사해 디스크 쓰기 증폭이 발생했습니다.
   - `RunService::try_update` 등에서 `PaneCache` 없이 전체 창 정규화가 수행되었고, `PaneCache` 역시 `started_at`이 확정된 실행 중 창의 변경 없는 `Arc<RunSession>` 포인터를 재사용하지 못했습니다.

3. **분산된 보존·용량 상한·정리 정책 (Fragmented Retention & GC)**
   - 창 본문(8 MiB · 400개 로그 · 2 MiB 텍스트/그래프 예산), git 체크포인트 스냅샷(14일 보관, 용량 상한 없음), 손상 격리 파일(`.unreadable-*`, `.before-session-files-*`), 비정상 종료가 남긴 `.tmp` / `.part` 파일, 이미지 캐시(200 MiB, 기간 정리 없음), 작업 지도(`work-map`), 원격 캐시(`remote-cache`), 호스트 로그(`host.log`)의 수명주기와 상한이 개별 모듈에 흩어져 있었습니다.

---

## 2. 메모리 계층화 (Hot / Cold `SessionBody`) 및 Eviction · 지연 복원

### 2.1 상태 계층
| 계층 | 조건 (`RunSession`) | 메모리 보유 범위 | 디스크 정본 |
| :--- | :--- | :--- | :--- |
| **Hot** | `body_on_disk == false` | 전체 `logs`(최대 400개), `graphRuns`(최대 2 MiB), `planHistory`, `todoProgress`, `backgroundWork`, `shellRuns` | `sessions/<stem>.json` (+ 실행 중 `.jsonl`) |
| **Cold** | `body_on_disk == true` | `SessionDigest` 요약(`logCount`, `runIds`, 최신 `"user"`·`"assistant"`가 보존된 `digest.tail`), `bodyRevision`, `graphBlockSizes` 및 인덱스 메타 필드 | `sessions/<stem>.json` (컴팩션 완료 정본) |

### 2.2 Hot 고정(Pinning) 규칙 — 절대 언로드하지 않는 창
다음 조건 중 하나라도 해당하는 창은 자동 Eviction 대상에서 제외합니다:
- 현재 활성 워크스페이스(`activeWorkspaceId`)에 속한 창
- 실행 중(`status == "running"`)이거나 실행 라우트(`RunService::has_route`)가 열려 있는 창
- 대기 중인 도구 권한/질문/계획 요청(`pending_permissions`)이 있는 창
- 대기 중인 Antigravity 권한 카드(`antigravity_card`)가 있는 창
- 컴포저 입력 대기열(`panes::queued`)에 예약된 입력이 남아 있는 창

### 2.3 자동 Eviction 및 참조 해제 흐름 (`60s` 주기)
1. `RunService`는 창별 마지막 접근 시각(`last_touched`)을 `ensure_bodies`, `ensure_workspace_bodies`, `launch`, 실행 종료, `update_pane` 시점마다 갱신합니다. 기록이 없는 창(시작 후 한 번도 접근하지 않은 창)은 유휴로 보지 않고 "지금 접근한 것"으로 기록해 그 시점부터 유휴 시간을 셉니다.
2. `bodies::evict_idle` (`60초` 주기)가 `RunService::evict_idle_bodies`를 호출하여 비활성 워크스페이스의 비고정 Hot 창 중:
   - 유휴 경과 시간이 `idle_body_ttl_secs` (`300초`, 5분) 이상이거나,
   - 비활성 Hot 창 개수가 `max_hot_inactive_panes` (`8개`)를 초과한 경우(`last_touched`가 가장 오래된 순서부터) 언로드합니다.
3. `Store::evict_live_body(pane)` (`Store::evict_loaded_body(id)`는 스토어의 마지막 저장본으로 같은 일을 하는 얇은 래퍼):
   - **서비스가 지금 들고 있는 창(`RunService`의 `before`)**을 넘겨받아, 그 창의 본문을 그대로 쓰고(`encode_body`) 그 창에서 Cold 사본을 만듭니다. 스토어의 마지막 저장본에서 Cold 사본을 만들면 저장 전의 새 로그·상태·제목·초안·사용량이 덮어써져 사라지기 때문입니다. 서비스는 반영 직전 같은 `Arc`가 그대로일 때만 Cold 사본으로 교체하고, 그 사이 창이 바뀌었거나 서비스가 닫히는 중이면 스토어를 `load_pane_body`로 되돌려 서로 어긋나지 않게 합니다.
   - 쓰기 허용 모드(`writes == true`)에서는 미반영 변경이나 `sessions/<stem>.jsonl` 저널이 있으면 먼저 `sessions/<stem>.json` 정본으로 컴팩션합니다(읽기 전용 모드에서는 디스크 정본이 존재하고 미저장 변경이 없을 때만 언로드).
   - `RunSession::evicted_to_disk()`로 Cold 사본을 생성하면서 `written_revisions`에 단조 증가 `bodyRevision`을 유지하고 `Store.written` 및 `Store.cache`에서 기존 Hot `Arc<RunSession>`을 제거합니다.
   - `normalize.rs`의 `cold_block_sizes`는 `body_on_disk == true`인 창의 `graphBlockSizes`를 빈 배열로 초기화하지 않고 `kept_run_ids()` 및 `"pending-input"` 기준으로 최대 `256`개까지 보존합니다.
4. 호스트 계층에서 `history::evict_panes`(창별 `SessionHistoryState` + `retained` 해제)와 `panes::prune_inactive_seen`(`PaneUi.seen` 해제)을 즉시 호출하고 `View::sync` 및 델타 플러시를 예약하여 모든 구버전 `Arc<RunSession>`을 즉시 해제합니다.

### 2.4 재진입 시 지연 복원 (Lazy Restore)
- 워크스페이스 전환(`View::sync` → `workspace_scheduler`), 실행 시작(`launch`), 창 상태 갱신(`update_pane`), 이전 그래프 기록 조회(`history::load_older` 진입 시 `bodies::ensure_in_view`), 모바일/원격 미러 상세 조회 시 `Store::load_pane_body`가 정본 `<stem>.json`(및 크래시 잔여 `.jsonl`)을 읽어 100% 무손실로 Hot 상태를 복원합니다.

---

## 3. 증분 Append-Only 본문 저널 (`sessions/<stem>.jsonl`) 및 컴팩션

### 3.1 디렉터리 레이아웃 불변식
- 최상위 `<data_dir>`에는 기존 호환성 계약(`workspace-state.json`, `workspace-state.json.bak`, `workspace-state.json.before-session-files-*`, `sessions/` 등)을 유지하며, 모든 증분 저널 파일은 반드시 `sessions/<stem>.jsonl` (`0700` 폴더 내부 `0600` 파일) 아래에만 생성합니다.

### 3.2 저널 레코드 포맷 (`BodyJournalRecord`)
한 줄당 하나의 compact JSON 레코드(`O_APPEND` + `sync_data()`)를 추가합니다:
- `rev: u64`: 단조 증가 `bodyRevision`
- `sessionId: String`: 대상 창 ID 검증용 식별자
- `ops: Vec<BodyJournalOp>`:
  - `logAppend`: 새 `LogEntry` 끝에 추가
  - `logReplaceLast`: 스트리밍 중인 마지막 구간의 `LogEntry` 갱신 (`index`, `entry`). 재생은 위치(`index`)가 아니라 `entry.id`로 대상 엔트리를 찾습니다(정본이 앞부분 트림됐거나 `.bak` 폴백이면 위치가 어긋나므로). 기반 본문에 없는 엔트리의 갱신은 다른 엔트리 위에 덮지 않고 건너뜁니다. `index`는 id가 없는 엔트리에만 쓰입니다.
  - `graphRunUpsert`: 활성 또는 알림 `MightyGraphRun` 추가·갱신
  - `meta`: `planHistory`, `todoProgress`, `backgroundWork`, `shellRuns` 스냅샷 갱신

### 3.3 증분 기록 조건 및 자동 컴팩션
- **증분 Append 경로**:
  - `pane.status() == "running"`이고 디스크에 기반 `sessions/<stem>.json`이 이미 존재하며,
  - 직전 저장 본문(`Store.written`) 대비 순수 증분 diff가 성립하고,
  - 직렬화된 단일 레코드 크기가 `< 256 KiB`이며 기존 `.jsonl` 파일이 `< 512 KiB` 및 `< 128`줄 미만일 때 `sessions/<stem>.jsonl`에 한 줄만 추가합니다.
- **컴팩션(`sessions/<stem>.json` 병합 후 `.jsonl` 삭제) 조건**:
  - 실행이 완료·중단·오류로 종료(`pane.status() != "running"`)된 첫 저장 시점
  - 저널 파일이 `512 KiB` 또는 `128`레코드 상한에 도달한 경우
  - 400개 로그 캡 또는 2 MiB 그래프 예산 트림 등으로 앞부분 엔트리가 제거되어 순수 증분 diff가 아닌 경우
  - `Store::evict_live_body` 호출 시점 (`Store::compact_pane_journal`은 같은 컴팩션을 단독으로 하는 API이지만 현재 앱 안에 호출처가 없습니다)
  - 창이 닫혀 인덱스에서 제거될 때(`remove_unlisted_bodies`가 `.json`, `.json.bak`, `.jsonl` 일괄 삭제)
- **백업 최적화 (`portable_replace`)**:
  - Unix에서 정본 교체 전 `.bak` 생성 시 기존 `0600` 불변 정본 파일에 대해 `hard_link` + 원자적 `rename`을 우선 시도하여 수 MB 바이트 복사를 O(1) 메타데이터 연산으로 단축하고, 파일시스템 제약 시에만 바이트 복사로 폴백합니다.
- **다운그레이드**: 이전 버전은 `sessions/<stem>.jsonl`을 알지 못하고 `<stem>.json`만 읽습니다. 실행 중 비정상 종료로 `.jsonl`이 남은 채 이전 버전으로 내려가면 그 저널에만 있던 마지막 증분(최대 `512 KiB` / `128`레코드)은 이전 버전에 보이지 않고, 이후 이전 버전이 `<stem>.json`을 다시 쓰면 `.jsonl`은 고아 파일이 됩니다. 정상 종료·실행 종료 시에는 항상 컴팩션되므로 영향은 비정상 종료 직후의 다운그레이드로 한정됩니다.
- **크래시 복구**:
  - `Store::load`, `Store::load_pane_body`, `store::read_cold_body_logs` 실행 시 `<stem>.jsonl`이 함께 존재하면 정본의 `bodyRevision`보다 큰 `rev`를 가진 유효 레코드들을 순서대로 재생(전원 차단 등으로 잘린 마지막 불완전 줄은 안전하게 무시)한 뒤 정규화합니다.

---

## 4. 통합 보존 기한(Retention) · 용량 상한 · GC 규격

모든 수치는 `mighty_core::retention::RetentionPolicy`에 일원화되어 있으며, 백그라운드 GC(`retention_gc.rs`)는 쓰기 허용 일반 실행(`host.writes() && host.smoke.is_none()`)에서만 동작합니다 (`시작 8초 후 1회` + `30분 주기`, 유휴 본문 Eviction은 `60초 주기`).

| 대상 | 경로 | 보존 기한 / 조건 | 용량 / 개수 상한 | 정리 방식 및 안전 보호 규칙 |
| :--- | :--- | :--- | :--- | :--- |
| **유휴 창 본문 (RAM)** | `RunService` / `Store` | 유휴 `300초` (5분) | 비활성 Hot 최대 `8개` | Cold 전환(`digest.tail`만 유지), 활성·실행·권한대기·큐 보유 창 제외 |
| **창 본문 파일** | `sessions/<stem>.json` | 창 열려 있는 동안 유지 | 파일당 `8 MiB`, 로그 `400개`, 텍스트·그래프 각 `2 MiB` | 창 닫힘 시 `.json` / `.bak` / `.jsonl` 즉시 삭제 |
| **본문 증분 저널** | `sessions/<stem>.jsonl` | `"running"` 동안만 유지 | 라인 `< 256 KiB`, 파일 `< 512 KiB`, `< 128줄` | 실행 종료·상한 도달·언로드 시 `<stem>.json`으로 컴팩션 후 삭제 |
| **손상 격리본** | `<data_dir>` 및 `sessions/` 내 `.unreadable-*` | `30일` | 기한 경과분 중 최신 `3개` | 30일 이내 격리본은 개수와 무관하게 삭제하지 않고, 30일이 지난 것 중 최신 3개만 남기고 오래된 순 삭제. 분할 이전 상태의 유일한 사본인 `.before-session-files-*`는 **절대 정리하지 않음** (§3.1) |
| **비정상 종료 임시 파일** | `<data_dir>` 하위 `*.tmp` | `10분` 경과 | — | 진행 중인 원자적 쓰기를 보호하기 위해 10분 초과분만 삭제 |
| **Git 체크포인트 스냅샷** | `snapshots/<repo-hash>/` | 미사용 `14일` | 저장소당 `256 MiB`, 전체 `1 GiB` | 14일 기한은 모든 저장소에 적용. 용량 상한(저장소당·전체)은 **열려 있는 창이 쓰는 저장소의 스토어를 지우지 않음**(GC가 열린 창의 폴더로 스토어 이름을 구해 넘김). 최근 `5분` 이내 수정된 스토어는 건드리지 않고, 스토어별 잠금 파일(`gc.lock`)을 스냅샷과 GC가 함께 지켜 스냅샷 도중 삭제·삭제 도중 스냅샷이 겹치지 않음(잠금은 10분이면 크래시의 잔여물로 봄); 초과 시 오래된 `used` 순 정리 |
| **파일 원본 (변경 검토)** | `file-originals/` | 참조 중인 `kept_run_ids` | `FileOriginalStore` 상한 | Cold 창도 `SessionDigest.run_ids`로 참조 유지 (`files.rs` 2초 감시) |
| **에이전트 이미지 캐시** | `image-cache/` | 미참조 `30일` | 전체 `200 MiB` | 메모리의 창과 디스크 본문(`sessions/*.json`·`.jsonl`)이 참조 중인 해시 경로는 삭제 제외. 정리 순서는 엄밀한 LRU가 아니라 파일 수정 시각(캐시가 이미지를 쓸 때 갱신하는 mtime)이 오래된 순이며, 아직 보내지 않은 컴포저 초안에만 붙은 이미지는 참조로 보지 않아 보호되지 않음 |
| **작업 지도 요약 캐시** | `work-map/` | `30일` | 전체 `32 MiB` | 기한 경과 및 상한 초과분 오래된 순 삭제 |
| **원격 세션 캐시** | `remote-cache/` | `14일` | 전체 `64 MiB` | 기한 경과 및 상한 초과분 오래된 순 삭제 |
| **사용량 원장** | `usage-ledger.json` | 일별 기록 `120일` | 열린 창 ID만 `last` 유지 | 원장의 소유자(`usage_report`)가 메모리의 원장에서 `prune_closed_sessions`를 실행하고 그 원장을 잠금 아래에서 저장 (GC가 별도 사본을 읽어 덮어쓰지 않음) |
| **모바일 업로드 임시 조각** | `mobile-remote/uploads/*.part` | `15분` 경과 (만료 10분) | 동시 최대 `64개` | 열린 업로드가 없을 때 만료된 `.part` 정리 |
| **호스트 로그** | `tauri-logs/host.log` | 직전 1세대 백업 | `10 MiB` (`host.log.1` 회전) | 오픈 시점 및 주기적 GC에서 10 MiB 초과 시 `host.log.1`로 회전 |
| **사용자 자산 (영구 보호)** | `saved-prompts.json`, `~/.claude`, `~/.codex`, `~/.gemini` 등 | **영구 보존 (GC 제외)** | — | 앱 GC는 사용자 프롬프트와 외부 CLI 에이전트 메모리를 **절대 수정·삭제하지 않음** |

### 진단 및 수동 실행 IPC
- `memoryPolicyStatus`: 현재 적용 중인 `RetentionPolicy`, Hot/Cold 창 개수, 마지막 GC 실행 시각 및 `GcReport` 반환
- `runRetentionGc`: 즉시 유휴 창 Eviction + 프로필/스냅샷/원장 GC 패스를 실행하고 `GcReport` 반환

---

## 5. `MC-13`(원격 동기화) 및 `MC-14`(`mighty-workspace`) 호환성 보장

1. **원격 증분 동기화 (`MC-13`)**
   - `Store`는 본문이 언로드되더라도 `written_revisions`에 창별 단조 증가 `bodyRevision`을 유지합니다.
   - `mighty_core::store::read_body_deltas_since(data_dir, session_id, since_rev)`는 `since_rev` 이후 연속된 `BodyJournalRecord` 증분 연산 목록을 조회하는 API이며, 이미 컴팩션된 경우에는 `None`을 반환해 전체 스냅샷 전송으로 폴백하도록 설계되었습니다. **현재 원격 동기화 경로(`mirror`·`phone`)는 이 함수를 호출하지 않고 전체 본문을 보냅니다.** 증분 전송으로의 연결은 후속 작업입니다.

2. **교차 워크스페이스 온디맨드 조회 (`MC-14`)**
   - **`recent_workspace_activity` O(1) 무디스크 응답**: `SessionDigest::of`가 마지막 8개(`DIGEST_TAIL`) 엔트리 바깥에 위치한 최신 `"user"` 요청뿐 아니라 최신 `"assistant"` 응답도 시간순으로 함께 보존(`clamp_for_digest` 적용)하므로, 비활성 워크스페이스의 창이 Cold 상태(`body_on_disk == true`)여도 디스크 I/O나 메모리 재적재 없이 최신 요청(`request`)과 응답 요약(`summary`)을 즉시 반환합니다.
   - **`search_workspace` 비오염 전체 검색**: `workspace_mcp.rs`의 `search_workspace`는 `session.body_on_disk == true`인 창을 만났을 때 `RunService`의 메모리에 본문을 적재하지 않고 `mighty_core::store::read_cold_body_logs(&host.data_dir, session.id())`로 디스크 정본 + `.jsonl`의 전체 대화 기록을 일회성으로 읽어 검색한 뒤 즉시 해제합니다.

3. **공유 에이전트 메모리 동기화 (`MC-13`, `/v2/memory/*`)**
   - 호스트는 피어가 가진 공유 워크스페이스의 항목만 주고받습니다. 피어가 푸시한 항목의 `project_slug`는 무시하고 호스트가 아는 그 워크스페이스의 슬러그로 바꾸며, 기존 항목을 덮거나 삭제 표시(tombstone)하려면 그 기존 항목의 범위도 피어에게 공유된 범위여야 합니다(공유하지 않는 워크스페이스·꺼진 `User` 범위는 id를 알아도 건너뜀). 책이 가득 차도 호스트의 살아 있는 로컬 항목은 원격 항목을 위해 밀어내지 않고(삭제 표시만 정리) 원격 항목을 건너뜁니다. `GET /changes`는 공유하지 않는 워크스페이스의 항목을 슬러그가 같아도 돌려주지 않습니다.
   - `GET /changes?since=<seq>&limit=<n>`는 한 번에 최대 `limit`(기본 200, 상한 500)개·`256 KiB`만 돌려주고, 잘린 경우 `hasMore: true`와 마지막 항목의 `seq`를 `serverSeq`로 줍니다(이전 피어는 `hasMore`를 무시하고 다음 동기화에서 이어 받음). 푸시는 항목 `128`개·`256 KiB` 이하의 배치로 나눠 보내고, 호스트가 받은 배치만 기록해 다음 동기화가 이어집니다.
   - 클라이언트는 받은 항목 중 그 연결의 원격 워크스페이스에 속한 `Project` 항목만 반영하고(슬러그는 이 컴퓨터가 아는 슬러그로 맞춤), `User` 항목은 이 컴퓨터의 `share_user_memory`가 켜져 있을 때만 받습니다. 덮어쓸 수 있는 기존 항목도 그 연결의 범위로 한정됩니다. 원격 워크스페이스의 슬러그는 이 컴퓨터 디스크의 `.git/config`를 읽지 않고 폴더 이름·워크스페이스 이름에서만 만듭니다.
   - `remote-cache/<연결>/state.json`에는 원격 창의 대화 꼬리(`tail`)를 저장하지 않으며(`Store::save`가 미러 창을 쓰지 않는 것과 같은 불변식), 미러 변경마다가 아니라 최대 2초에 한 번(진행 중 하나, 최신 요청만 이어서) 쓰고 두 쓰기 경로는 하나의 잠금으로 직렬화합니다.
