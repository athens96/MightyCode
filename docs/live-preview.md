# 워크트리 라이브 프리뷰와 요소 고르기 (MC-7)

에이전트 창 머리의 지구본(`pane-preview`)이 그 창의 워크스페이스에서 듣고 있는 dev 서버를 찾아 보여 주고, 고른 서버를 그 서버가 속한 창의 프리뷰로 연다. 프리뷰가 열린 창은 입력창 위에 띠(`pane-preview-strip-<id>`)가 생기고, 거기서 요소 고르기·스크린샷·콘솔 오류·실패한 요청을 한 번 눌러 입력창에 첨부한다. 사용자 설명은 도움말 [라이브 프리뷰](help/content/ko/terminal-files-browser.md#live-preview)에 있다.

코드: `src-tauri/src/preview/{mod.rs,session.rs,element.rs}`, 화면은 `native/windows/web/src/preview/`. MC-48의 CDP 클라이언트(`verify/cdp.rs`)와 브라우저 찾기·실행(`verify/browser.rs`), 주소 규칙(mighty-delegation `verify::allowed_browser_url`), MC-25의 포트 블록과 실행 동작 주소, MC-26의 창별 열린 포트를 다시 쓴다.

## 왜 앱 안 창이 아니라 설치된 브라우저인가

0.5에서 앱 안 브라우저(CEF)는 뺐다([browser-pane.md](browser-pane.md)). 새 창 종류를 만들지 않고, Tauri 웹뷰(자식 웹뷰는 `unstable` 기능이다)에 dev 서버를 띄우지도 않는다. 프리뷰는 이 컴퓨터에 설치된 Chrome·Edge·Chromium·Brave(MC-48과 같은 순서, 내려받지 않는다)를 **창이 보이게** 띄운 것이다.

- 프로필: 실행마다 새 폴더 `<data>/preview-browser/<id>`. 브라우저가 끝나면 지우고, 시작할 때 남은 폴더도 지운다. 사람의 브라우저 프로필·쿠키는 쓰지 않는다.
- 인자: `verify::browser::private_args`(DevTools는 루프백의 아무 포트, `--user-data-dir`, 확장·동기화·첫 실행 화면·백그라운드 통신 끔) + `--window-size=1280,860 about:blank`. `--headless`가 없다는 것만 MC-48과 다르다.
- 브라우저 하나에 창마다 프리뷰 하나(브라우저 창 하나). 같은 창에서 다른 서버를 열면 그 창에서 이동한다.
- 마지막 프리뷰가 닫히면(사람이 창을 닫거나 띠의 닫기, 창을 닫음) `Browser.close`로 끝내고 프로필을 지운다. 사람이 브라우저를 끄면 DevTools 연결이 끊긴 것을 보고 정리한다. 앱이 끝나면(`RunEvent::Exit`) 프로세스를 죽이고 프로필을 지운다.
- 설치된 브라우저가 없으면 메뉴 맨 위에 그렇게 말하고, 서버는 기본 브라우저로 연다(같은 주소 규칙, 요소 고르기 없음).

## 서버 찾기 (`previewTargets {paneId}`)

1. 장소: 묻는 창의 장소, 같은 워크스페이스의 다른 에이전트 창(`kind == "claude"`)의 worktree, 기본 체크아웃 한 번(묻는 창이 거기서 일하면 그 창, 아니면 그런 첫 창). 같은 폴더는 한 번만.
2. 장소마다 열 수 있는 포트: worktree 창은 그 레코드의 포트 블록, 기본 체크아웃은 `ports.json`에 이미 있는 블록(`worktree_lifecycle::main_block`, 읽기만: 프리뷰는 블록을 배정하지 않는다), 그리고 그 폴더에서 도는 실행 동작들이 출력에서 찾은 주소(첫 것만이 아니라 전부).
3. 그 포트들과 MC-26이 읽은 창의 열린 포트(`attention::pane_ports`)를 루프백으로 한 번씩 묻는다(`preview::probe`: 127.0.0.1, 안 되면 ::1에 접속해 `HEAD / HTTP/1.0`을 보내 `HTTP/`로 답하면 http, TLS 경고(0x15)면 https, 그 밖은 웹 서버가 아님. 접속 250 ms, 답 600 ms, 한 번에 64개).
4. 답: 실행 동작 주소 → 블록에서 답한 포트 순서의 `servers`(`paneId`, `branch`(기본 체크아웃은 null), `url`, `port`, `action`, `open`), 창의 열린 포트 중 그 창의 포트 밖인 것은 `outside`(메뉴에 회색으로, 열지 않는다), `browser`(쓸 브라우저 이름, 없으면 null). SSH·원격(페어링) 워크스페이스는 `available: false`와 이유.

메뉴(`preview.ts` `previewMenu`): 장소 · 실행 동작 · 주소. 이미 열린 창의 서버는 ●. 창이 둘 이상이면 `모두 열기`(창마다 첫 서버).

## 여는 규칙 (보안)

- `previewOpen {paneId, url, external?}`은 **그 창의** 장소를 다시 계산해(페이지가 보낸 값은 믿지 않는다) `Allowed::check`로 주소를 확인한다: `http(s)`, 호스트 `localhost`·`127.0.0.1`·`[::1]`, 사용자 이름·암호 없음, 포트는 그 창의 블록 안이거나 도는 실행 동작 주소의 포트(MC-48 브라우저 점검과 같은 `allowed_browser_url`을 실행 동작마다 한 번씩).
- 프리뷰 탭의 최상위 프레임 이동은 모두 DevTools `Fetch`(`resourceType: Document`, 요청 단계)로 멈춰 세우고, 같은 규칙에 맞을 때만 보낸다(`session::navigation_allowed`). 맞지 않으면 `BlockedByClient`로 막고 띠에 `preview.note.blocked`를 보인다. 하위 프레임(페이지가 넣은 embed)은 그대로 둔다. 리디렉트도 요청마다 멈춘다.
- 프리뷰가 아닌 탭·창(페이지의 `window.open`, 사람이 연 새 탭)이 http·https·file 주소로 가는데 어느 프리뷰의 규칙에도 맞지 않으면 닫는다(`session::closes_other_tab`). `about:blank`, `chrome://newtab`, `devtools://`는 그대로 둔다.

### 프리뷰 페이지는 앱의 Tauri IPC에 닿지 않는다

- 페이지는 앱의 웹뷰가 아니라 다른 프로그램(설치된 브라우저)의 창에 뜬다. 앱은 프리뷰를 위해 웹뷰를 만들지 않는다.
- 설령 앱 웹뷰에 떠도: 앱의 capability는 `main` 웹뷰 하나(`capabilities/default.json`, `remote` 없음), `withGlobalTauri: false`, `host_post`는 `main` 라벨 + 앱 자신의 origin(`AppOrigin::trusts`) + 이번 실행의 nonce만 받는다. 프리뷰 주소는 앱 origin이 아니다(배포 빌드의 `tauri://localhost`·`http://tauri.localhost`).
- 페이지가 DevTools 소켓을 열 수 없다: `--remote-allow-origins`를 주지 않으므로 브라우저가 `Origin`이 붙은(웹 페이지가 여는) 연결을 거절한다. 앱의 CDP 클라이언트는 `ws://127.0.0.1`·`localhost`에만 붙는다.
- 요소 고르기는 DevTools의 검사 모드(`Overlay.setInspectMode`)다. 페이지에 앱이 부를 수 있는 함수·바인딩(`Runtime.addBinding`)·메시지 통로를 넣지 않는다. 읽기 스크립트(`element::PICK_SCRIPT`)는 DevTools가 고른 노드 위에서 `Runtime.callFunctionOn`으로 한 번 돌린다.

테스트: `preview::tests::the_previewed_page_has_no_way_to_the_apps_ipc`(설정·capability·웹뷰를 만들지 않음), `tests::preview_addresses_are_never_the_apps_origin`(main.rs), `session::tests::the_preview_browser_is_another_program_on_a_private_profile_no_page_can_drive`(실행 인자), `element::tests::the_pick_script_reaches_nothing_but_the_page`, 주소 규칙 `a_preview_opens_only_on_this_computer_within_the_panes_ports`, 이동 규칙 `a_previews_top_frame_stays_within_the_panes_ports`, `another_tab_off_every_previews_ports_is_closed`.

## 세션 (`session.rs`)

`BROWSER`(tokio 뮤텍스)가 도는 브라우저의 명령 채널을 쥐고, 브라우저 하나를 한 작업(`Pump`)이 DevTools 연결과 함께 갖는다. 작업은 페이지의 명령(`Open`, `Pick`, `Screenshot`, `Findings`, `Close`)과 브라우저 이벤트를 `select!`로 번갈아 받는다. 응답을 기다리면 스스로를 막는 호출(`Page.navigate`: 그 탐색의 요청을 이 작업이 멈춰 세우므로)은 `Cdp::send`로 보내고 답을 기다리지 않는다. 닫히는 중에 온 명령에는 `preview.error.closed`로 답하고, `Open`은 한 번 새 브라우저로 다시 한다.

창마다 기억하는 것: 대상·세션·주 프레임 id, 주소·제목, 규칙, 마지막으로 연 뒤의 콘솔 오류·예외·실패한 요청(MC-48 `PageFindings::observe`, 각 50개), 고르는 중인지, 마지막으로 고른 요소(백엔드 노드 id, 태그, 선택자). 주 프레임이 새 문서로 가면(`Page.frameNavigated`, 부모 없음) 오류와 고른 요소를 비운다. 페이지에 미는 `preview` 메시지는 창마다 300 ms에 한 번까지.

## 요소 고르기와 첨부

- `previewPick {on}`: 검사 모드를 켜고 브라우저 창을 앞으로. 사람이 요소를 누르면(`Overlay.inspectNodeRequested`) 모드를 끄고 `DOM.resolveNode` → `PICK_SCRIPT`로 읽어 `preview-element-<tag>.txt`를 그 창의 입력창에 첨부하고(`composer::add_files`, 끌어다 놓기와 같은 개수·크기 규칙), 앱을 그 창으로 앞으로 가져온다(`notifications::focus`). 사람은 "이 버튼 간격 줄여줘"처럼 덧붙여 보낸다.
- 읽는 것: 태그, 선택자(`data-testid` 같은 테스트 속성 → 안정된 `id` → 태그·안정된 클래스 두 개·`:nth-of-type`을 위로 붙여 페이지에서 하나만 찾을 때까지, 최대 8단계; 하나가 아니면 그렇다고 적는다), 주요 계산 스타일(기본값은 뺌), 텍스트 300자, 여는 태그 300자, 속성 몇 개(입력란 값은 읽지 않고, 암호 입력란은 텍스트·HTML도 읽지 않는다), 상자와 뷰포트, 컴포넌트 단서(React 파이버의 컴포넌트 이름과 개발 빌드의 `_debugSource`, Vue 3의 `__vueParentComponent`·`__file`, Svelte 개발 빌드의 `__svelte_meta.loc`, Angular 개발 빌드의 `ng.getOwningComponent`). 첨부 글은 제어 문자를 빼고 8,000자까지(`element::describe`).
- `previewScreenshot`: 고른 요소가 있으면 지금 상자(+8px, 한 변 4,000px까지)를 `captureBeyondViewport`로, 없으면 보이는 화면을 PNG로(5 MB가 넘으면 JPEG 70). 요소가 페이지에서 사라졌으면 `preview.error.elementGone`.
- `previewFindings {which: console|requests}`: `preview-console.txt`(console.error·assert, 잡히지 않은 예외, Log 오류) 또는 `preview-requests.txt`(4xx·5xx·네트워크 오류). 없으면 단추가 꺼져 있다.
- 실패는 띠에 한 줄로(`note`, × 로 닫음) 남고, 다음에 잘 된 동작이 지운다.

## 범위 밖 (남은 일)

- SSH·원격 워크스페이스의 프리뷰(MC-48처럼 `ssh::forward`로 포트를 당겨 올 수 있다).
- 휴대폰에서 프리뷰 보기·요소 고르기.
- 사람의 실제 브라우저 프로필(로그인 유지)로 여는 것. 지금은 실행마다 새 프로필이다.
