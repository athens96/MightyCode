# 턴 종료 검증 게이트와 브라우저 점검 (MC-48)

에이전트 창의 요청이 잘 끝나면 `.mighty.yaml`의 `verify` 명령(타입 검사·린트·단위 테스트, 브라우저 점검)을 실행하고, 실패하면 실패 요약을 같은 에이전트에 다음 요청으로 보내 고치게 한 뒤 다시 확인한다. 사용자 설명은 도움말의 [턴 종료 검증](help/content/ko/delegation.md)과 [검증 영수증](help/content/ko/agent-pane.md)에 있다. (이 문서와 이름이 비슷한 [verification.md](verification.md)는 앱 자체를 검증하는 명령 모음이다.)

## `.mighty.yaml`

```yaml
verify:
  commands:                 # 이름 → 명령, 또는 { command, cwd, timeoutSeconds }. `- name: … command: …` 목록도 된다. 최대 16개
    typecheck: npx tsc --noEmit
    test:
      command: npm test
      cwd: web              # 실행 폴더 안의 상대 경로(작업 `cwd`와 같은 규칙)
      timeoutSeconds: 600   # 기본 300, 1–3600
  maxRounds: 2              # 자동 수리 요청 횟수, 기본 2, 0–5 (0: 확인만)
  when: changed             # changed(기본: 요청이 파일을 바꿨을 때만) | always
  browser:                  # 선택
    runAction: dev          # 또는 url: http://localhost:5173 (이 컴퓨터 주소만). 둘 다 없으면 이 폴더에서 도는 첫 실행 액션의 주소
    paths: [/, /settings]   # 기본 ["/"], 최대 10개, `/`로 시작
```

- 파서: `mighty-delegation/src/mighty_yaml.rs`의 `decode_verify`. `VerifyConfig`는 비어 있으면 직렬화하지 않아 `verify`가 없는 파일의 `content_hash`는 그대로이고, `verify`를 더하거나 바꾸면 해시가 바뀌어 다시 승인해야 한다. `browser.runAction`은 병합된 `run`에 없으면 거절한다. `.mighty.local.yaml`의 명령은 이름별로 덮어쓰고 나머지 값은 로컬이 우선한다.
- 승인: 작업 패널의 설정 칩에 `verify: 이름: 명령` 줄이 그대로 보인다(`repo_tasks::describe_verify`). 승인 전에는 아무것도 실행하지 않고, 창 기록에 한 번(해시가 바뀔 때마다) 알린다.
- 로컬 워크스페이스의 worktree 창은 그 worktree의 `.mighty.yaml`을 읽고 워크스페이스 키로 승인을 확인한다. 에이전트가 worktree의 `verify`를 고치면 해시가 달라져 승인되지 않은 것으로 보고 실행하지 않는다.

## 루프 (`src-tauri/src/verify/mod.rs`, `mighty-delegation/src/verify.rs`)

1. `runs.rs`의 이벤트 펌프가 상태 `completed`를 받으면 `verify::on_run_finished`. 수리 턴을 기다리는 중이면 그 결과(`completed` → 다시 확인, `stopped` → 사람, `error` → 수리 실패)를 넘기고 끝.
2. 대상: `kind == "claude"`인 에이전트 창, 미러·위임 자식·비교 자식이 아님, 원격(페어링) 워크스페이스가 아님, 대기열이 비어 있음, Antigravity 카드 없음. 의견(advisor) 실행은 창 실행이 아니라서 오지 않는다. 스모크·도움말 캡처 중에는 하지 않는다.
3. `when: changed`: 그 요청의 MC-18 체크포인트(`start` 이후 파일이 있는 것)를 최대 10초 기다린다. `start`가 있는데 바뀐 파일이 없으면 건너뛰고, 체크포인트가 전혀 없으면(스냅샷 불가) 확인한다.
4. 명령: 실행 폴더(worktree 또는 워크스페이스)에서 `repo_tasks::shell_request`(Windows는 MC-35의 cmd 인용 규칙) + 로그인 셸 환경 + `worktree.env` + 창 변수(`PaneWorktree::environment`, 메인 체크아웃은 `main_place`의 `MIGHTY_PORT` 등)로 `run_actions::run_bounded`를 시간 제한과 함께 실행한다. 출력은 ANSI를 지우고 비밀값을 가린 뒤 끝 4,000자만 영수증에 둔다.
   - SSH 워크스페이스: `ssh::worktrees::command_request`로 그 기계의 실행 폴더(원격 worktree 또는 워크스페이스 폴더)에서 실행한다. 승인 확인은 `load_worktree_config`(원격에서 읽은 설정). 에이전트 실행처럼 SSH에는 샌드박스가 없다.
   - 샌드박스가 켜진 워크스페이스: `sandbox::verify_hooks`의 설정으로 Mac은 `mighty_run::sandbox::prepare`(같은 프로필, `sandbox-exec`), Windows는 `prepare_windows`(제한 토큰, 승인된 변경)로 감싸고, 네트워크 허용 목록 밖은 묻지 않고 거절한다(`refuse_unlisted`). 준비하지 못하면 그 명령은 실행하지 않고 실패로 기록한다(샌드박스 밖에서 실행하지 않음).
5. 실패 요약(`verify::failure_summary`): 실패한 명령마다 이름·종료 방식·명령과 출력 마지막 40줄, 그리고 브라우저 문제. 비밀값 줄은 `git_workbench::redact_secret_text`로 지우고 `worktree.env` 값(6자 이상)은 `[redacted]`로 바꾼다. 명령마다 같은 몫을 나눠 전체 6,000자 이내.
6. 수리 요청: `verify.repair.header`("검증 실패 — 고쳐 주세요 …") + 요약을 `phone::control_send`로 같은 창에 보낸다(창의 스타일·보낼 대상 규칙 그대로). 보낸 직후의 `PaneUi.submission`을 기억한다.
7. 멈춤(`verify::run_loop`): 통과 / `maxRounds`번 수리 후에도 실패(`maxRounds`) / 실패 해시가 직전과 같음(`sameFailure`: 출력 끝 40줄을 소문자로, 숫자 묶음은 0, 8자 이상 16진 id는 #으로 바꾼 뒤 SHA-256) / 한 번 더 돌리면 하루 예산 초과(`budget`: `compare::estimate_run_cost` + `budget_warning`, 자동화와 같은 계산) / 사람이 보냄·멈춤·대기열에 넣음·카드의 중지(`person`) / 수리 턴이 `completed`로 끝나지 않음(`repairEnded`) / 자동 수리 꺼짐·`maxRounds: 0`·직접 실행(`repairOff`).
8. 알림: 사람이 멈춘 경우와 직접 실행이 아니면 실패를 OS 알림(`notifications::notify_custom`)과 휴대폰 푸시(`failed` 종류)로 알린다. 통과·실패 모두 창 기록에 한 줄 남긴다.

## 브라우저 점검 (`src-tauri/src/verify/browser.rs`, `cdp.rs`)

- 브라우저: `verify::browser_candidates`가 OS별 알려진 설치 경로(Mac `/Applications`·`~/Applications`의 Chrome, Edge, Chromium, Brave / Windows `ProgramFiles`, `ProgramFiles(x86)`, `LocalAppData` 아래 / Linux `/usr/bin` 등)를 순서대로 본다. 내려받지 않는다. 없으면 "설치된 브라우저가 없다"고 적고 실패로 치지 않는다.
- 실행: `--headless=new --remote-debugging-port=0 --remote-debugging-address=127.0.0.1 --user-data-dir=<data>/verify-browser/<id>` 등(확장·동기화·백그라운드 네트워크 끔). 프로필의 `DevToolsActivePort`에서 주소를 읽고, 끝나면 `ChildControl::kill`로 브라우저와 그 자식을 모두 끝내고 프로필을 지운다.
- CDP 클라이언트: tokio-tungstenite(이미 lock에 있음)로 `ws://127.0.0.1`에만 연결한다. 페이지마다 `Target.createTarget` → `attachToTarget(flatten)` → `Runtime/Log/Network/Page.enable` → `Page.navigate` → `Page.loadEventFired`까지(검증 20초, 도구 5초) 기다린 뒤 잠깐(1.5초/0.7초) 더 모은다. `Runtime.consoleAPICalled`(error/assert), `Runtime.exceptionThrown`, `Log.entryAdded`(error, network 출처 제외), `Network.responseReceived`(400 이상), `Network.loadingFailed`(취소 제외)를 센다. `Accessibility.getFullAXTree`는 무시되지 않은 노드를 `역할 "이름"` 60줄로 줄이고 이름 없는 버튼·링크·이미지·입력칸을 따로 센다. `Page.captureScreenshot` PNG는 `<data>/verify-screenshots/<영수증 id>-<회차>-<n>.png`(최근 100개 유지).
- 실패로 치는 것: 페이지 오류(불러오지 못함), 콘솔 오류, 예외, 실패한 요청. 이름 없는 컨트롤은 세기만 한다.
- 주소 규칙(`verify::allowed_browser_url`): `http(s)`, 호스트는 `localhost`·`127.0.0.1`·`[::1]`, 사용자 정보 없음, 포트는 그 창의 포트 범위(worktree 또는 메인 체크아웃) 안이거나 실행 액션이 출력한 주소의 포트. 경로는 `/`로 시작하고 `//`로 시작하지 않는다. SSH 워크스페이스는 원격 포트로 확인한 뒤 MC-33의 `ssh::forward::open`으로 이 컴퓨터 포트에 연결해 연다.
- 에이전트 도구 `mighty_browser_check {url?, paths?}`: mighty-terminal MCP 서버의 다섯 번째 도구. 인자는 둘 다 선택이고(`paths`는 목록 또는 공백·쉼표로 나눈 문자열, 최대 3개), 앱은 창의 실행 폴더에서 도는 실행 액션 주소를 기본으로 쓴다. 에이전트가 준 주소도 같은 규칙(포트 범위 또는 실행 액션 포트)을 통과해야 한다. MCP 호출 30초 제한 안에 끝나도록 시작 8초, 페이지 5초로 줄인다. 결과는 JSON(개수, 페이지별 발견, 스크린샷 파일 이름). SSH 워크스페이스의 에이전트는 Mighty MCP 서버를 받지 않으므로 이 도구도 없다.

## 영수증

- 모델: `verify::VerifyReceipt { id, status(running|repairing|passed|failed), rounds, repairs, maxRounds, stopReason, commands[{name, command, passed, exitCode, timedOut, durationMs, output, error}], browser, browserCounts, configHash, startedAt, finishedAt, manual, runIds }`. 창마다 최근 10개를 메모리에 두고(앱을 다시 켜면 사라짐), 창을 닫으면 지운다.
- 화면: `verify` 푸시(`{kind:'verify', paneId, autoRepair, running, receipts}`)와 `verifyState`(설정·승인 여부 포함). `runIds`의 마지막 요청의 결과 블록에 `VerifyReceiptCard`(web `src/mighty/VerifyReceipt.tsx`)가 붙는다. 메서드: `verifyRun`(한 번, 수리 요청 없음), `verifyStop`, `verifySetAutoRepair{on}`(창별, 메모리), `verifyScreenshot{file}`(data URL).
- 휴대폰: 창 상세의 `verify`(출력과 브라우저 상세를 뺀 최신 영수증, 없으면 필드 없음). 휴대폰 앱은 창 표시 줄에 한 줄로 보여 준다.

## 검증하지 못한 것

- 실제 브라우저(Chrome 등)로 띄운 점검, 실제 에이전트 CLI로 도는 자동 수리, Windows 샌드박스 아래 검증 명령은 손으로 확인하지 않았다(테스트는 가짜 CDP 서버와 가짜 에이전트, 실제 셸 명령).
