# 진단 묶음과 요청별 감사 기록 (MC-54)

[devtool-gap-review.md](devtool-gap-review.md) 10번 항목(신뢰와 진단)의 두 가지: 버그를 알릴 때 첨부할 **진단 묶음 내보내기**와, 한 요청이 한 일을 한 줄씩 보는 **감사 기록**. 같은 항목의 커밋 전 비밀값 검사는 Git 워크벤치에 이미 있다(`src-tauri/src/git_workbench.rs`의 `SECRET_PATTERNS`, `scan_text_secrets`). 사용자 설명은 도움말의 [진단 묶음](help/content/ko/settings.md)과 [감사 기록](help/content/ko/agent-pane.md)에 있다.

## 진단 묶음 (`src-tauri/src/diagnostics/`)

설정 › 정보 › **진단** 상자(`settings/about.rs::diagnostics`, `settings/mod.rs`의 `about` 분류에 세 번째 상자로 붙는다)의 **진단 묶음 내보내기…** 단추.

1. OS 저장 대화상자(Tauri dialog 플러그인, 앱 창 소유)로 위치를 묻는다. 기본 이름은 `mighty-code-diagnostics-YYYYMMDD-HHMMSS.zip`(로컬 시각). 취소하면 아무것도 하지 않는다.
2. 입력(`bundle::Inputs`)을 모은다: 앱 버전(`host.version`), `std::env::consts::{OS, ARCH}`, Mac의 OS 버전(`UpdateInstaller::system_version`), 실행 엔진이 마지막으로 읽은 CLI별 설치 여부와 버전(`setup::runtimes`; 새로 `--version`을 돌리지 않는다), 설정 요약, 로그 경로.
3. 블로킹 스레드에서 묶음을 만들고(`bundle::build`) zip으로 쓴다(`bundle::write_zip`: 옆의 `.zip.part`에 다 쓴 뒤 옮기므로 실패해도 반쪽 파일이 남지 않는다).
4. 상자에 결과를 보인다: 만드는 동안 단추가 꺼지고 링이 돈다. 끝나면 `{name}에 저장했습니다 · 파일 n개 · 지운 값 m개`와 Finder(Windows는 탐색기)에서 보기 단추, 실패하면 이유. 앱 로그에는 파일 수와 지운 값 수만 남긴다(경로 없음).

묶음은 어디로도 보내지 않는다. 네트워크 호출이 없다.

### 들어가는 파일

| 이름 | 내용 |
|---|---|
| `manifest.json` | 형식 번호, 앱 버전, 만든 시각, `sentAnywhere: false`, 파일마다 이름·바이트·설명·규칙별 지운 개수(로그는 원래 크기와 잘렸는지도), 없던 로그(`notFound`), 규칙 목록, 전체 지운 개수, 넣지 않는 것 목록 |
| `system.json` | 앱 버전, OS 이름·아키텍처·버전, CLI마다 id·이름·설치 여부·버전, CLI를 읽었는지 |
| `settings-summary.json` | `summary::settings_summary`: 워크스페이스 수(원격, SSH, 샌드박스, 창 worktree 모드, 이슈 트래커, MightyWork 연결), 창 수(제공자별, 상태별), 켜진 기능(상태 줄, 완료 알림, CLI·플러그인 자동 업데이트·로그인, 체크리스트, 다른 워크스페이스 참조, 직접 사용량 조회, 앱 자동 업데이트, 업데이트 주소를 바꿨는지, 단계별 모델·모델 기본값이 있는지), 휴대폰(켜짐, 직접 연결, 예전 앱 허용, 공유한 것만, 릴레이 주소를 바꿨는지), 화면(언어, 테마, 배치). 이름·경로·주소·키는 넣지 않는다 |
| `logs/host.log`, `logs/host.log.1` | 앱 로그와 그 앞 파일(`LogFile::dir`, 보통 `<데이터>/tauri-logs`) |
| `logs/daemon.log` | 백그라운드 데몬(`<데이터>/daemon/daemon.log`) |
| `logs/startup.log`, `logs/perf.log` | 시작 단계와 성능 기록(`<데이터>/`) |

로그는 각각 마지막 2MB(`MAX_LOG_BYTES`)만, 잘렸으면 첫 줄바꿈 뒤부터 넣는다. 없는 로그는 `notFound`에 이름만 적는다. 대화, 요청과 답, 파일 내용, 워크스페이스·창 이름, 경로, 환경 변수는 넣지 않는다.

### 비밀값 제거 (`diagnostics/redact.rs`)

묶음의 모든 텍스트(manifest 포함)는 `Redactor::redact`를 거친다. 규칙은 이 순서로 돌고, 규칙마다 지운 개수를 센다(`Tally`).

1. `homePath`: 이 사용자의 홈 폴더(`HOME`, 없으면 `USERPROFILE`; Windows 경로는 `/` 표기도) → `~`.
2. `userPath`: 다른 `/Users/<이름>`, `/home/<이름>`, `X:\Users\<이름>` → `~` (경로 앞이 공백·따옴표·`=`·괄호 등일 때만, 웹 주소 속 `/home/`은 그대로).
3. `authorizationHeader`: `Authorization`, `Proxy-Authorization`, `X-Api-Key`, `Cookie`, `Set-Cookie` 헤더·필드의 값.
4. `urlCredentials`: `scheme://user:pass@host`의 `user:pass`.
5. `secretTokens`: Core의 `mighty_core::shared_memory::sanitize_secrets` — PEM 개인 키, `*_TOKEN=`·`*_PASSWORD=` 같은 대입 줄, `Bearer` 값, 커밋 검사와 같은 제공자 키 접두사(AWS, GitHub, GitLab, Slack, Anthropic, OpenAI, Google, Stripe).
6. `secretFields`: 이름이 password, passwd, passphrase, secret, token, api key, access key, private key, pairing key/code, client secret, session key, signature로 끝나는 필드·대입·쿼리·플래그의 값(`"pairingKey": "…"`, `password=…`, `?token=…`, `--token …`). 값이 `null`·`true`·`false`·빈 값·이미 지운 값이면 두고, `inputTokens`·`tokenCount`처럼 이름이 그 말로 끝나지 않으면 건드리지 않는다.
7. `longHex`: 48자리 이상의 16진수(제어 API·데몬 토큰, 해시). 40자리 커밋 해시는 남긴다.
8. `email`: 이메일 주소(계정 로그인 이름).
9. `commitScan`: 마지막으로 Git 워크벤치의 커밋 전 검사 패턴(`git_workbench::scan_text_secrets`)을 다시 돌려, 아직 걸리는 줄은 통째로 `[redacted: 종류]`(`redact_secret_text`)로 바꾼다. 커밋 시트가 막는 것은 묶음에도 남지 않는다.

규칙은 `redact.rs`의 테스트로 고정한다: 홈·다른 사용자 경로, 헤더·URL 자격 증명, Core 가드(제공자 키, Bearer, PEM, 대입), 필드·플래그·쿼리, 긴 16진수·이메일·커밋 해시, 커밋 검사가 찾는 아홉 종류가 하나도 남지 않음, 평범한 로그 줄은 그대로. `bundle.rs`는 비밀값이 든 로그로 만든 묶음의 파일 목록·manifest 수치·zip 내용·로그 꼬리 자르기를, `summary.rs`는 요약에 이름·경로·주소가 없음을 확인한다. 테스트는 임시 폴더만 쓴다.

## 감사 기록 (`native/windows/web/src/mighty/audit.ts`, `AuditPanel.tsx`)

결과 블록 머리의 **감사 기록** 단추(`scroll-text`, 빈 결과에는 없음)나 머리 오른쪽 클릭 메뉴의 **감사 기록**, **감사 기록을 Markdown으로 복사**. 패널(C형)은 줄마다 시각·종류·내용(명령·경로·주소는 고정폭, 400자까지 보이고 전체는 툴팁)·메타를 보이고, 위에 종류별 거르기, 아래에 개수 요약과 **Markdown으로 복사** 단추가 있다.

이미 저장된 것에서만 만든다(새로 수집하지 않음). `buildTurnAudit`이 읽는 것:

| 줄 종류 | 출처 |
|---|---|
| 요청 | `run.input`의 첫 줄, 첫 사용자 항목의 시각 |
| 명령 | 활동 `kind: command`, MCP의 `run_in_terminal`·`run_command` |
| 편집 | 활동 `kind: edit` |
| 변경 파일 | 실행의 체크포인트(MC-18) 중 `end`, 없으면 파일이 있는 마지막 것: 상태(A/M/D/R/T), 더한·지운 줄, 옛 이름. 200개까지, 나머지는 "그 밖에 n개" |
| 웹 | 활동 `kind: web`, MCP의 `open_url`·브라우저 도구 |
| 도구 | 나머지 MCP·기타 도구 |
| 위임 | 활동 `kind: agent`(Task 등), 도구 이름에 `delegat`·`spawn_agent`·`follow_up`이 든 것 |
| 자식 창 | `parentSessionId`가 이 창이고 요청 시간(시작 1초 전 ~ 끝 5초 뒤, 실행 중이면 지금까지) 안에 만들어진 창 |
| 승인 | 이 실행의 계획 카드 기록(`planHistory`의 `graphRunId`/`runId`, 결과는 `plan.outcome.*`), 그리고 출력이 거부 문구("denied this tool request", "permission to use … denied", "doesn't want to proceed" 등)인 실패한 도구 호출(`거부됨`). 승인을 기다리는 호출은 그 종류 그대로 `승인 대기` 상태 |
| 검증 | 이 요청의 MC-48 영수증(`receiptForRun`): 명령마다 한 줄(통과·실패, 걸린 시간, 종료 코드·시간 초과), 영수증 제목 한 줄 |
| 결과 | 실행 상태(완료·실패·중지됨·진행 중) |

- 하위 에이전트 블록의 활동은 그 블록 제목을 `who`로 단다. 같은 활동 id는 한 번만. 읽기·검색(`read`, `search`)은 줄로 넣지 않고 개수만 센다. `turn`은 뺀다.
- 정렬: 시각순, 시각이 없는 줄은 바로 앞 줄 뒤에 머문다(안정 정렬). 요청은 맨 위, 결과는 맨 아래.
- 한계: 저장 기록에 따로 묻지 않고 허용된 도구 호출과 사용자가 "이번만 허용"한 호출은 구별되지 않으므로, 허용은 승인 줄로 나오지 않고 거부와 계획 카드 결정만 나온다. 앱이 보관하는 실행 수를 넘은 오래된 요청은 기록이 없으면 비어 보인다.
- Markdown(`auditMarkdown`): `## 감사 기록 · 요청 n`, 요청 인용, 개수 줄, 줄마다 `- 시각 · **종류** 내용 — 메타`. 명령·경로·주소는 코드 스팬(안의 백틱보다 긴 울타리, 줄바꿈은 `⏎`, 1000자까지).
- 복사는 호스트 메서드 `auditCopy {text}`(`diagnostics/mod.rs`)를 거친다: 진단 묶음과 같은 `Redactor`로 비밀값과 홈 경로를 지우고 시스템 클립보드에 쓴 뒤 `{copied, redacted}`를 돌려준다. 패널은 "복사했습니다 · 비밀값 n개를 지웠습니다"를 보인다. 메뉴에서 바로 복사할 때는 따로 알리지 않는다(실행 기록 복사와 같음).
- 문구 키는 `turnAudit.*`(Mac Swift 앱의 기존 `audit.*` 키와 겹치지 않게)와 `diagnostics.settings.*`.

테스트: `audit.test.ts`(줄 종류·순서·개수, 거부·대기·하위 에이전트 표시, 자식 창 시간 범위, 빈 요청, Markdown과 코드 스팬, 패널 렌더, 결과 블록의 단추, 소스에 한국어 문구 없음), Rust의 `diagnostics::*` 테스트.
