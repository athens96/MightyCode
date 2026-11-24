---
title: 터미널·파일 창
order: 7
section: terminal-files-browser
---
에이전트 창 옆에 터미널과 파일 창을 함께 열어 둘 수 있습니다.

## 터미널 창 {#terminal}
워크스페이스 폴더에서 열리는 실제 터미널입니다.
1. {{ui:menu.addTerminalPane}}({{kbd:⌘T}})를 누르거나 {{ui:workspace.addPane}} → {{ui:workspace.newTerminal}} 항목을 고릅니다.
2. 명령을 직접 입력합니다.

셸이 끝나면 {{ui:terminal.restart}}을 누릅니다. 예전 명령 기록은 창 머리 … 메뉴의 {{ui:pane.menu.terminalHistory}}에서 봅니다. 창 머리를 오른쪽 클릭한 메뉴의 {{ui:terminal.history.title}} 항목도 같습니다. 앱을 끄면 터미널은 다시 열리지 않습니다.

![[terminal-pane]]

### 알림과 벨 {#terminal-signals}
터미널 창의 프로그램이나 앱이 해석하지 않는 CLI가 알림이나 벨을 보내면 그 창에 표시합니다. 알림은 OSC 9(`printf '\e]9;빌드 끝\a'`), OSC 777(`notify;제목;본문`), OSC 99(kitty, 제목과 본문만)를 받습니다. 진행률을 알리는 `9;4;…`는 알림이 아닙니다. 본문은 제어 문자를 빼고 200자까지 보이며, 한 창에서 1초에 하나만 받습니다.
- 다른 창을 보고 있거나 앱이 뒤에 있으면 사이드바의 그 창 줄과 탭에 점이 붙고 창 테두리가 강조됩니다. 알림이면 OS 알림도 옵니다. 벨은 표시만 합니다.
- 에이전트 창이 승인이나 질문의 답을 기다릴 때와 worktree의 setup이 실패했을 때도 같은 표시가 붙습니다. {{ui:phone.dashboard.title}}에는 기다리는 창의 개수가 보이고, 누르면 그 창으로 갑니다.
- {{kbd:⇧⌘U}}(Windows: {{kbd:Ctrl+Shift+U}})를 누르면 가장 최근에 알린 창으로 갑니다. 다시 누르면 그다음 창으로 갑니다. 메뉴 막대의 {{ui:menu.workspace}} 메뉴에 있는 {{ui:attention.next}} 항목도 같습니다.
- 창을 보면 표시가 사라집니다. 지금 보고 있는 창의 알림은 표시도 OS 알림도 하지 않습니다.
- 알림을 받지 않으려면 [설정](settings.md#companion)의 {{ui:terminalNotifications.toggle}}를 끕니다.

### 열린 포트와 PR {#pane-ports}
창의 셸과 에이전트, run 액션, 그리고 그 아래에서 돌아가는 프로그램이 연 TCP 포트를 10초마다 읽어 사이드바의 창 줄과 worktree 막대에 보여 줍니다. 앱 창이 보이는 동안 지금 워크스페이스의 창만 읽습니다. worktree 막대의 포트를 누르면 브라우저에서 `localhost`로 엽니다.

worktree 창에는 그 브랜치의 PR 번호와 상태(열림·초안·병합됨·닫힘), 검사 결과(통과·실패·진행)도 보입니다. PR은 2분마다 다시 읽고, 누르면 PR 페이지를 엽니다. 사이드바가 좁으면 포트와 PR은 아이콘만 보입니다.

### 에이전트 터미널 {#agent-terminal}
Claude나 Codex가 사용자가 봐야 하는 명령(개발 서버, 로그인 등)을 실행하면 에이전트 창 오른쪽에 전용 터미널 창이 생깁니다. 실행 중인 명령에 직접 입력할 수 있습니다. 창을 닫아도 명령은 앱을 끌 때까지 계속 돌고, 창 머리의 {{ui:agentTerminal.terminalPane.open}} 단추로 다시 엽니다.

## 파일 창 {#files}
워크스페이스의 파일을 둘러보는 읽기 전용 창입니다. 파일을 고치지는 않습니다.
1. {{ui:menu.showFiles}}({{kbd:⇧⌘E}})를 누르거나 워크스페이스 머리의 폴더 단추를 누릅니다.
2. 왼쪽 목록에서 파일을 고릅니다. {{ui:files.tree.filter}}로 이름을 찾고, 방향키와 {{kbd:Return}}으로 움직입니다.
3. 오른쪽에서 내용을 봅니다.

| 파일 | 보이는 방식 |
|---|---|
| 소스·텍스트 | 줄 번호와 색 강조. 큰 파일은 처음 1 MB만 보입니다. |
| Markdown | {{ui:files.markdown.rendered}}와 {{ui:files.markdown.source}} 보기를 전환합니다. 128 KB가 넘는 파일은 소스로 보입니다. |
| HTML | 페이지로 그린 {{ui:files.markdown.rendered}}와 {{ui:files.markdown.source}} 보기를 전환합니다. |
| 이미지·PDF | {{ui:files.image.fit}}, {{ui:files.image.actualSize}}, {{ui:files.image.zoomIn}}, {{ui:files.image.zoomOut}}. PDF는 첫 쪽만 보입니다. 50 MB가 넘는 이미지는 미리 보지 않고, 1억 화소가 넘는 이미지는 맞춤 크기로만 보입니다. |
| 그 밖 | {{ui:files.preview.unsupported}}. {{ui:menu.showInFinder}}로 엽니다. |

워크스페이스 밖의 파일은 보이지 않습니다. 파일이 바뀌었으면 {{ui:files.tree.refresh}}을 누릅니다.

![[files-pane]]

### 편집기에서 열기 {#open-in-editor}
파일을 고치려면 쓰던 편집기에서 엽니다. 줄을 아는 곳에서는 그 줄로 바로 갑니다.
- 파일 창: 미리보기 머리의 {{ui:editor.open}} 단추, 목록에서 파일이나 폴더를 오른쪽 클릭한 메뉴({{ui:editor.openFolder}}).
- 대화와 마이티 흐름: 파일 이름을 {{kbd:⌘}}-클릭(Windows: {{kbd:Ctrl}}-클릭)하면 미리보기 대신 편집기에서 그 줄을 엽니다. 미리보기 머리와 결과 파일 목록의 줄마다 있는 단추도 같습니다.
- 변경 보기: [요청 변경 검토](agent-pane.md#request-changes)와 worktree 창의 변경 보기에서 줄을 오른쪽 클릭하면 그 줄을, 단추는 고른 파일을 엽니다.
- worktree 창 막대의 메뉴와 [검토 대기열](layout.md#review)에서는 worktree 폴더를 엽니다.

어떤 편집기를 쓸지는 [설정](settings.md#agents)의 {{ui:settings.editor.label}}에서 고릅니다. 자동이면 VS Code, Cursor, Zed, JetBrains IDE(IntelliJ IDEA, WebStorm, PyCharm 등), Sublime Text 순으로 찾은 첫 번째를 씁니다. 로그인 셸의 PATH에 있는 명령(`code`, `cursor`, `zed`, `idea`, `subl`)을 먼저 찾고, Mac에서는 응용 프로그램 폴더의 앱 안에 든 명령을, Windows에서는 보통의 설치 폴더에 있는 실행 파일을 찾습니다. {{ui:settings.editor.system}}은 줄 번호 없이 파일의 기본 앱으로 엽니다. 코드를 실행하는 파일은 열지 않습니다. 작업 공간이나 worktree 밖의 경로는 열지 않습니다.

## 웹 링크 {#web-links}
답변 속 웹 링크와 에이전트가 여는 웹 페이지는 기본 브라우저에서 열립니다.

## 라이브 프리뷰 {#live-preview}
에이전트 창 머리의 지구본({{ui:preview.button.name}})을 누르면 이 워크스페이스에서 듣고 있는 dev 서버를 찾아 보여 줍니다. {{ui:preview.menu.main}}과 각 worktree 창의 포트 블록, 그 폴더에서 도는 실행 동작이 찾은 주소를 한 번씩 물어 웹 서버만 고릅니다. 하나를 누르면 그 서버가 속한 창의 프리뷰로 열리고, 창이 여럿이면 모두 열기로 창마다 하나씩 엽니다.

1. 프리뷰는 이 컴퓨터에 설치된 Chrome, Edge, Chromium, Brave 중 하나를 따로 띄운 창입니다. 매번 새 프로필을 쓰고, 평소 쓰는 브라우저의 로그인·쿠키는 쓰지 않습니다. 없으면 기본 브라우저로 열리고 요소 고르기는 쓸 수 없습니다.
2. 프리뷰가 열린 창은 입력창 위에 띠가 생깁니다. {{ui:preview.strip.pick}}를 누르고 프리뷰 창에서 요소를 누르면 그 요소의 태그, 고유 선택자, 주요 스타일, 텍스트 일부, React·Vue·Svelte·Angular 컴포넌트 단서가 입력창에 첨부되고 앱이 그 창으로 돌아옵니다. "이 버튼 간격 줄여줘"처럼 덧붙여 보내면 됩니다.
3. {{ui:preview.strip.shotPage}}은 보이는 화면을, 요소를 고른 뒤에는 그 요소 영역만 찍어 첨부합니다. 콘솔 오류와 실패한 요청 단추는 페이지를 마지막으로 연 뒤의 오류를 한 번에 첨부합니다(없으면 꺼져 있습니다).
4. {{ui:preview.strip.close}}를 누르거나 프리뷰 창을 닫으면 끝납니다. 마지막 프리뷰가 닫히면 그 브라우저도 끝나고 프로필은 지워집니다.

> [!note]
> 프리뷰는 `localhost`·`127.0.0.1`·`::1`이면서 그 창의 포트 블록이나 실행 동작 주소 안의 주소만 엽니다. 페이지가 그 밖으로 가려 하면 막고 띠에 알려 줍니다. 창의 프로그램이 포트 블록 밖에서 듣는 포트는 메뉴에 보이기만 하고 열지 않습니다. `MIGHTY_PORT`로 서버를 띄우면 열 수 있습니다. 프리뷰 페이지는 앱과 다른 프로그램에서 열리므로 앱에 닿을 수 없습니다. SSH와 원격 워크스페이스는 아직 프리뷰가 없습니다.
