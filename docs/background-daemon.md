# 백그라운드 데몬 (앱을 닫아도 작업 계속)

> MC-3, 0.6.0. 구현: 순수 부분은 `native/tauri/crates/mighty-core/src/daemon/`(프로토콜·링 버퍼·유휴 시계), 엔진은 `native/tauri/crates/mighty-run/src/daemon/`(서버·클라이언트·프로세스 소유자 전환), 앱 쪽은 `native/tauri/src-tauri/src/daemon/`(실행 모드·파일·설정·종료·다시 붙기)과 `src-tauri/src/settings/background.rs`. 운영체제별 부분은 `mighty-platform-macos`/`mighty-platform-windows`의 `agent_io.rs`(데몬 소켓·파이프·잠금)와 macOS `child_groups.rs`(`reap_child_groups`).

설정 › 일반 › **백그라운드 작업**의 **앱을 닫아도 작업 계속**(기본 꺼짐, 환경설정 키 `daemon.keepRunning`)을 켜면, 에이전트 실행과 터미널 창의 셸을 앱이 아니라 백그라운드 데몬이 소유한다. 앱을 ⌘Q로 끝내거나 자동 업데이트로 다시 시작해도 작업이 멈추지 않고, 앱을 다시 열면 이어 붙는다. 참고한 설계는 MC-24 비교 조사의 herdr-core(헤드리스 PTY 서버, GUI는 클라이언트), Happy Agent(로컬 소켓 데몬), Paseo(데몬이 중심, 데스크톱·폰·CLI는 클라이언트)다.

## 구조

- **데몬**은 앱과 같은 실행 파일이다: `MightyCode --daemon [--headless] [--profile <절대 경로>]`. 창·웹뷰·상태 파일을 열지 않고, `exe_mode`에서 Tauri보다 먼저 갈라진다(`daemon/entry.rs`). 앱이 `ProcessHost::start_detached`로 띄우므로(맥은 새 세션, Windows는 앱의 job 밖) 앱보다 오래 산다. 데이터 폴더마다 하나: `<data>/daemon/daemon.lock`을 맥은 `flock`, Windows는 공유 없이 연 핸들로 잡고, 두 번째 데몬은 "already running"을 남기고 0으로 끝난다.
- **엔드포인트**: 맥은 `<data>/daemon/mighty.sock`(0700 폴더의 0600 소켓, 경로가 `sun_path`를 넘으면 `$TMPDIR/mc-<uid>/<hash>-daemon.sock`), Windows는 현재 사용자만 열고 원격 클라이언트를 거부하는 named pipe `mighty-daemon-<32 hex>`. 데몬은 듣기 시작한 뒤 `<data>/daemon/endpoint.json`(transport, address, pid, app 버전, protocol; 0600)을 쓰고 끝날 때 지운다. 토큰은 `<data>/daemon/token`(0600, 64 hex)으로 MC-9 제어 API의 토큰과 따로다. 로그는 `<data>/daemon/daemon.log`(1MB를 넘으면 다음 시작 때 새로 쓴다; 토큰·환경 변수 값·출력은 적지 않는다).
- **소유하는 것**: (a) 터미널 창의 PTY, (b) 에이전트 실행의 CLI 자식 프로세스. 앱은 시작할 때 플랫폼의 `ProcessHost`와 `PtyHost`를 한 번 감싼다(`RoutedProcesses`, `RoutedPtys`). 감싼 호스트는 **키가 있는** 두 호출만 데몬으로 보낸다: 실행 엔진의 `spawn_keyed(요청, "run:<창 id>")`와 터미널 창의 `open_keyed(요청, <창 id>)`. git, 상태 줄, 계정 로그인 같은 다른 모든 자식은 지금처럼 앱 안에서 돈다. 데몬이 연결돼 있지 않거나 설정이 꺼져 있으면 키가 있는 호출도 앱 안에서 돈다(**in-process** 모드, 이전과 같다).
- **나뉘는 일**: 데몬은 프로세스와 바이트만 맡는다. 출력 파싱, 대화 기록, 승인 카드, 창 상태는 그대로 앱의 실행 엔진(`mighty-run` `manager`/`service`)과 터미널 창(`mighty-terminal`)이 한다. 엔진이 받는 것은 늘 같은 `Child`(stdin, stdout, stderr, `ChildControl`)와 `PtySession`이고, 데몬 모드에서는 그 뒤가 데몬 연결이다.
- **데몬 안**: 데몬은 앱이 쓰는 것과 같은 플랫폼 호스트로 자식을 띄운다(`daemon_ports`). 그래서 자식은 늘 소유자와 함께 죽는다: 맥은 프로세스 그룹(데몬이 끝날 때 `atexit`이 그룹 전체에 SIGKILL), Windows는 데몬이 가진 kill-on-close job object.

## 프로토콜

MC-9 제어 API와 같은 방식이다(`mighty_core::daemon::protocol`, 응답·오류 줄과 `LineBuffer`는 `control::protocol`의 것을 그대로 쓴다). 한 줄에 JSON 하나, 줄마다 최대 1MiB.

- 연결의 첫 줄은 `{"id":1,"cmd":"hello","protocol":1,"app":"0.6.0","token":"…"}`. 토큰이 다르거나 hello 전에 다른 요청이 오면 `unauthorized`로 답하고 끊는다. hello는 10초 안에 와야 한다. 답 `{"protocol","app","pid","accepted"}`: 프로토콜이 다르면 `accepted:false`이고 그 연결은 `status`와 `shutdown`만 할 수 있다(두 명령은 모든 버전에서 같은 모양으로 유지한다).
- 요청: `spawn`(key, meta, program, args, cwd, env, envRemove, shellCommand), `pty.open`(key, meta, program, args, cwd, env, columns, rows), `attach`(proc, from), `detach`, `write`(base64, 최대 64KiB), `stdin.close`, `resize`, `kill`, `busy`, `release`, `meta`(앱의 메모를 프로세스 meta에 덧씌운다), `list`, `status`, `shutdown`. 답은 `{"id":n,"ok":true,"result":…}` 또는 `{"id":n,"ok":false,"error":{"code","message"}}`이고 순서는 섞일 수 있다.
- 이벤트(`{"id":0,"event":…}`): `output`(proc, offset, stream `out`/`err`/`pty`, data base64), `replayed`(attach할 때 보관돼 있던 것을 모두 보냄, 이제부터 실시간), `exit`(code 또는 error; 그 프로세스의 마지막 바이트를 받은 뒤에만).
- 한도: 인자·환경 변수 각 4,096개, meta 64KiB, 동시에 데몬이 가진 프로세스 256개(가득 차면 아무도 붙지 않은 끝난 프로세스부터 버린다), 동시 연결 16개.

## 링 버퍼와 이어 받기

- 프로세스마다 출력을 링 버퍼에 보관한다(`ring::OutputRing`): 에이전트 실행 8MiB(stdout과 stderr를 읽은 순서대로 함께), 터미널 2MiB(대략 화면 기록 크기). 위치는 지금까지 나온 바이트 수(offset)이고 줄지 않는다. 넘치면 오래된 것부터 버리고, 남은 첫 바이트가 글자 중간(UTF-8 연속 바이트)이면 그것도 버려 늘 글자 처음에서 시작한다.
- 연결마다 쓰는 쪽이 하나이고, 붙은 프로세스마다 커서를 가진다. 출력이 오면 링에 넣고 붙은 연결을 깨운다; 연결은 커서 뒤를 링에서 읽어 보낸다. 느린 클라이언트는 뒤처질 뿐 데몬이 링보다 더 쥐고 있지 않는다(뒤처진 사이 링에서 밀려난 부분은 offset 차이로 "잃은 바이트"가 된다). 폴링은 없다.
- 클라이언트 쪽 `ring::Cursor`가 받은 offset으로 겹침을 걸러, 다시 보낸 것과 실시간 출력이 겹쳐도 바이트마다 한 번만 넘긴다. 글자 중간에서 끊긴 덩어리도 이어 붙으면 온전한 글자가 된다.

## 앱 종료와 다시 시작

- 설정이 켜져 있고 데몬이 무언가 실행 중일 때 ⌘Q(`ExitRequested`)는 종료를 멈추고 묻는다: "실행 중인 작업 n개를 계속 둘까요?" — **계속 두고 종료**, **모두 멈추고 종료**, **취소**. 창이 이미 없으면(Windows에서 마지막 창을 닫음) 설정의 뜻대로 계속 둔다. 자동 업데이트의 재시작(`app_update`의 `launch_helper`)은 늘 계속 둔다(`keep_for_update`).
- **계속 두기**: 터미널을 닫고 실행을 정리하기 전에 `Route::detach`가 먼저 돈다. 다시 붙을 수 있는 실행(`run:` 키 = 승인 채널을 쓰는 Claude 실행)은 읽던 쪽이 받은 것을 다 읽기를 기다린 뒤 "다음 앱이 이어 받을 위치"(`resumeAt`, 앱이 읽은 stdout의 마지막 온전한 줄 바로 뒤)를 데몬의 meta에 남기고 놓는다. 다시 붙을 수 없는 실행(`run-stop:` 키, 다른 에이전트와 셸 명령)은 끝낸다. 터미널은 놓는다. 그 뒤 엔진의 정리는 kill/close 대신 "놓기"가 되어, 읽기가 끝나고 대기는 바로 답한다(종료 코드 −2). 놓은 뒤에는 stdin에 쓰기와 닫기도 데몬에 보내지 않는다(엔진의 정리가 입력을 닫아 계속 둔 실행을 끝내지 않도록). 창은 앱이 닫힐 때 "멈춤"으로 저장된다.
- **모두 멈추기**: 이전처럼 실행과 터미널을 끝내고, 마지막에 데몬에 `shutdown`을 보내 바로 끝낸다.
- **다시 시작**: 앱이 데몬에 붙으면 `list`로 이전 앱이 남긴 것을 받는다(`take_back`). 셸 창의 터미널은 그 창에 "제안"해 두었다가, 창이 셸을 열 때(`open_keyed`) 새로 띄우지 않고 offset 0부터 붙여 화면 기록을 다시 그리고 창 크기로 맞춘다. Claude 창의 실행은 `RunService::adopt`가 창을 다시 실행 중으로 만들고(사용자 기록·제목은 건드리지 않는다), 실행 엔진의 `spawn_keyed`가 그 키의 실행을 `resumeAt`부터 붙인다(아무것도 새로 띄우지 않는다). 승인 채널은 "다시 붙은" 모드로 만들어져 `initialize`만 보내고 프롬프트는 쓰지 않는다. Claude의 `initialize` 답의 `pending_permission_requests`와 이어 받은 출력의 `control_request`로 기다리던 승인·질문 카드가 다시 나온다(같은 요청 id는 한 번만). 창이 없어졌거나 받을 수 없는 프로세스는 끝내고 버린다.
- 데몬은 실행 중인 프로세스도 붙은 앱도 없이 10분이 지나면 스스로 끝난다(`IdleClock`; `--headless`는 끝나지 않는다). 끝난 프로세스의 출력은 앱이 끝까지 읽고 `release`하거나 데몬이 끝날 때까지 보관한다.

## 실패와 안전

| 상황 | 결과 |
|---|---|
| 데몬이 죽음(충돌, SIGKILL) | 앱 연결이 끊기며 기다리던 요청은 실패하고, 데몬의 모든 자식은 "끝남"으로 읽혀 실행은 오류·멈춤, 터미널은 종료로 바뀐다. 새 작업은 앱 안에서 돌고, 설정이 켜져 있으면 2초 뒤 한 번 데몬을 다시 띄운다. 맥의 PTY 셸은 마스터가 닫혀 SIGHUP으로 끝나고, 파이프 자식은 데몬이 `<data>/daemon/child-groups`에 적어 둔 그룹으로 다음 데몬 시작과 앱 시작(`reap_daemon_groups`) 때 끝낸다. Windows는 job object가 바로 끝낸다. |
| 앱과 데몬의 프로토콜이 다름(업데이트 뒤) | 앱은 붙지 않는다. 실행 중인 작업이 없으면 옛 데몬을 끄고 새로 띄우고, 있으면 "이전 버전의 백그라운드 작업 n개" 대화 상자로 **멈추고 새로 시작** / **그대로 두기**를 묻는다. 설정에는 "다른 버전의 데몬이 실행 중"이 보인다. |
| 데몬이 이미 실행 중 | 앱은 새로 띄우지 않고 붙는다. 두 번째 데몬은 잠금을 못 잡고 바로 끝난다. |
| 다른 사용자·원격 | 맥 소켓은 0600 폴더 0700, 연결할 때 상대 uid를 확인한다. Windows 파이프는 현재 사용자 SID만 허용하고 원격 클라이언트를 거부한다. 모든 연결은 토큰이 있어야 한다. |
| 너무 큰 줄·요청 | 1MiB를 넘는 줄은 `too_large`로 답하고 연결을 끊는다. |
| 끊긴 엔드포인트 파일 | 600ms 안에 답이 없으면 없는 것으로 보고 새로 띄운다. |

## 한계(0.6.0)

- 다시 붙는 실행은 승인 채널을 쓰는 로컬 Claude 에이전트 창뿐이다(위임된 자식 창 제외). Codex·Antigravity·pi·opencode·Hermes와 셸 명령 실행도 데몬에서 돌지만 계속 두기로 앱을 닫으면 끝난다. 각 실행기의 채널(Codex 승인 app-server, ACP, pi 대화)이 "다시 붙은" 모드를 갖추면 넓힐 수 있다.
- 다시 붙은 Claude 실행의 Mighty 연결은 그 턴 동안 옛 앱의 것을 가리킨다: 모드 브리지(활동·사용량·다이어그램)와 위임 MCP 바인딩 토큰이 새 앱과 맞지 않아 그 턴이 끝날 때까지 쓰지 못할 수 있다. 다음 요청부터는 새로 띄운 프로세스라 정상이다.
- 실제 Claude CLI가 실행 도중의 두 번째 `initialize`에 `pending_permission_requests`로 답하는지는 유료 실행으로 확인하지 않았다. 답하지 않거나 거부해도 다시 붙은 채널은 실패로 끝내지 않고, 이어 받은 출력의 `control_request`로 카드가 나온다.
- 터미널은 창을 처음 열 때 돌려받는다. 앱을 다시 연 뒤 한 번도 보이지 않은 셸 창의 터미널은 그때까지 데몬에서 계속 돈다.
- `--headless`는 데몬을 끝나지 않게 띄울 뿐이고, 아직 MC-9의 `mighty status`/`agent new`/`send`/`wait`를 직접 받지 않는다(그 명령은 실행 엔진이 있는 앱에 붙는다). 휴대폰·미러의 헤드리스 접속도 후속이다.
- 설정의 "메모리"는 데몬이 보관 중인 출력의 크기다(프로세스 RSS가 아니다).

## 검사

- `cargo test -p mighty-core --lib daemon`: 프로토콜 왕복과 한도, 링 버퍼(잘림, 글자 경계, 커서로 한 번씩), 유휴 시계.
- `cargo test -p mighty-run --test daemon`: 메모리 전송과 가짜 호스트로 서버·클라이언트·소유자 전환. 토큰·hello·다른 프로토콜 거부, 줄 한도, 실행 출력과 종료와 release, stdin·kill, 놓은 터미널이 다음 앱에 한 번만 다시 그려짐, 작은 링의 잃은 바이트, 데몬이 죽으면 자식이 끝남으로 읽히고 새 작업은 앱 안에서 돎, 데몬 없을 때 이전과 같음, 유휴 종료, shutdown, 계속 두기에서 다시 붙을 실행만 남기고 `resumeAt` 남기기.
- `cargo test -p mighty-run --test daemon_adopt`: 실제 자식(가짜 CLI, 운영체제의 프로세스 호스트)을 메모리 데몬이 소유한 채로, 첫 앱의 Claude 실행이 답하지 않은 승인을 기다리는 동안 계속 두기로 놓고, 다음 앱이 `RunService::adopt`로 받아 승인 카드를 다시 받고 답하며, 프롬프트는 한 번만 갔고(첫 앱) 사용자 기록도 새로 붙지 않으며 창이 완료로 끝나는지 본다. 놓은 뒤 엔진의 정리가 stdin을 닫아 실행을 끝내지 않는 것도 여기서 확인한다.
- `cargo test -p mighty-run --test tool_permission`: 다시 붙은 채널이 프롬프트를 쓰지 않고, `pending_permission_requests`의 카드를 한 번만 보이고, 두 번째 `initialize`를 거부하거나 답하지 않는 CLI에도 실패로 끝내지 않음.
- `cargo test -p mighty-platform-macos --lib agent_io`: 소켓 경로·권한, 두 번째 리스너 거부, 잠금(한 명만, 링크 따라가지 않음). Windows는 같은 검사가 `mighty-platform-windows`에 있다.
- `cargo test -p mightycode --test daemon`(맥): 빌드한 `MightyCode --daemon --profile <임시 폴더>`를 실제로 띄워, 파일 권한, 두 번째 데몬, 실제 소켓으로 `/bin/sh` 실행, 앱이 놓은 사이 나온 출력이 다음 앱에 정확히 한 번 이어짐, 다른 프로토콜 거부, shutdown 뒤 엔드포인트 정리, SIGKILL로 죽은 데몬의 자식을 다음 데몬이 끝냄을 본다. 사용자의 데이터 폴더는 쓰지 않는다.
