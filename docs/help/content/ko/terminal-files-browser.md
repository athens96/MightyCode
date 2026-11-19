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
