---
title: 화면 구성
order: 2
section: layout
---
창은 왼쪽 사이드바와 오른쪽 작업 영역으로 나뉩니다. 작업 영역에는 실행 창을 탭으로 겹치거나 좌우·상하로 나눠 놓을 수 있습니다.

![[overview]]

## 사이드바 {#sidebar}
사이드바에는 위에서부터 다음이 있습니다.
- {{ui:sidebar.searchPlaceholder}}: {{kbd:⌘K}}를 누르면 검색칸으로 바로 갑니다.
- {{ui:phone.dashboard.title}}: 모든 워크스페이스의 실행 창 상태를 한곳에서 봅니다.
- {{ui:sidebar.workspacesHeader}} 목록: 워크스페이스를 누르면 그 워크스페이스로 바뀝니다. 옆의 화살표를 누르면 실행 창 목록을 펼치거나 접습니다.
- 아래쪽: 도움말 단추, 화면 테마 단추(어둡게·밝게)와 설정 단추.

워크스페이스 목록을 펼치면 마지막 줄에 {{ui:workspace.addPane}} 단추가 있습니다. 실행 창 줄을 오른쪽 클릭하면 {{ui:menu.rename}}, {{ui:menu.closePane}} 항목이 있습니다.

### 사이드바 폭 조절 {#sidebar-resize}
사이드바와 작업 영역 사이의 경계선을 끌면 사이드바 폭이 바뀝니다({{ui:sidebar.resize}}).
- 경계선에 마우스를 올리면 강조색으로 바뀝니다. 좌우로 끌어 폭을 맞춥니다. 폭은 정해진 범위 안에서만 바뀌고, 앱을 다시 열어도 유지됩니다.
- 경계선을 두 번 클릭하면 기본 폭으로 돌아갑니다.
- 경계선을 Tab 키로 고른 뒤 {{kbd:←}} / {{kbd:→}}를 누르면 10px씩 좁아지거나 넓어집니다.
- 경계선을 왼쪽 끝까지 끌면 사이드바가 접힙니다. 다시 펼치면 끌기 전의 폭으로 돌아옵니다.

### 사이드바 접기 {#sidebar-collapse}
작업 영역을 넓게 쓰려면 사이드바를 접습니다.
1. 작업 영역 머리글 왼쪽 위의 사이드바 단추를 누르거나 {{kbd:⌃⌘S}}를 누릅니다. {{ui:phone.dashboard.title}} 화면과 첫 화면에도 같은 자리에 이 단추가 있습니다. 메뉴 막대의 보기 메뉴에도 {{ui:sidebar.collapse}} 항목이 있습니다.
2. 다시 펼칠 때도 같은 단추나 {{kbd:⌃⌘S}}를 씁니다({{ui:sidebar.expand}}).

접은 상태는 앱을 다시 열어도 유지됩니다. 접힌 상태에서 {{kbd:⌘K}}를 누르면 사이드바가 펼쳐지고 검색칸으로 갑니다.

![[sidebar-collapsed]]

## 작업 현황 {#dashboard}
사이드바 검색칸 아래의 {{ui:phone.dashboard.title}}을 누르면 모든 워크스페이스의 실행 창을 상태 순으로 모아 봅니다.
- 위쪽 칸에서 {{ui:phone.dashboard.stat.running}}, {{ui:phone.dashboard.stat.waiting}}, {{ui:phone.dashboard.stat.done}} 개수를 봅니다.
- {{ui:dashboard.usage.title}} 카드에서 계정 사용량을 봅니다. 같은 계정을 쓰는 다른 앱·세션과 함께 쓰는 한도입니다.
- 워크스페이스마다 {{ui:files.pane.title}} 단추와 {{ui:workspace.addPane}} 메뉴가 있습니다.
- 카드를 누르면 그 실행 창으로 갑니다. 워크스페이스나 실행 창을 고르면 작업 현황에서 나옵니다.

![[dashboard]]

## 실행 창 추가 {#add-pane}
실행 창은 AI 에이전트(Claude, Codex, Antigravity, pi, opencode, Hermes), 터미널, 파일 중 하나입니다.

1. 사이드바의 {{ui:workspace.addPane}} 단추를 누릅니다.
2. 만들 창을 고릅니다: 에이전트(Claude, Codex, Antigravity, pi, opencode, Hermes) 실행 창 또는 {{ui:workspace.newTerminal}}. 메뉴 맨 아래 묶음에는 워크스페이스를 추가하는 {{ui:workspace.add.existing}}, {{ui:workspace.add.newFolder}}, {{ui:workspace.add.fromGit}} 세 항목이 있습니다([워크스페이스 열기](getting-started.md#open-workspace)).
3. 새 AI 실행 창은 같은 에이전트로 가장 최근에 쓴 창의 모델·사고 강도·권한·보기 방식을 이어받습니다. 대화는 새로 시작합니다.

단축키: {{ui:menu.newClaudePane}}은 {{kbd:⌘N}}, {{ui:menu.addTerminalPane}} 항목은 {{kbd:⌘T}}, {{ui:menu.showFiles}} 항목은 {{kbd:⇧⌘E}}입니다. 실행 창은 모든 워크스페이스를 합쳐 128개까지 열 수 있습니다(다른 컴퓨터의 창을 그대로 보여 주는 원격 창은 세지 않습니다).

> [!note]
> Claude를 뺀 에이전트(Codex, Antigravity, pi, opencode, Hermes)의 실행 창에는 {{ui:badge.beta}} 표시가 붙습니다.

### 이전 세션 이어가기 {#resume}
에이전트 창을 추가할 때 이 폴더에 이전 세션이 있으면 고르는 창이 뜹니다. Antigravity는 이 폴더의 대화가 나오고, 하위 에이전트가 실행한 대화는 나오지 않습니다.
1. 새 대화는 {{ui:resume.choice.startNew}}, 이전 대화는 {{ui:resume.choice.resume}} 단추를 누릅니다.
2. {{ui:resume.title}} 목록에서 세션을 고릅니다. {{ui:resume.search}}으로 찾을 수 있습니다.
3. 새 창이 그 세션을 이어갑니다.

![[resume-choice]]

![[resume-list]]

### 지난 대화 검색 {#conversation-search}
{{ui:conversationSearch.open}}({{kbd:⇧⌘F}})은 이 폴더의 지난 세션에서 요청과 답에 들어 있는 말을 찾습니다. 워크스페이스 머리의 돋보기 단추, 사이드바에서 워크스페이스를 오른쪽 클릭한 메뉴, 메뉴 막대의 워크스페이스 메뉴에서도 엽니다.
- 모든 단어가 한 메시지에 들어 있는 세션이 최근 것부터 50개까지 나오고, 찾은 말에 표시가 붙습니다. 도구 실행 결과는 찾지 않습니다.
- Claude, Codex, Antigravity, pi의 세션을 찾고 opencode와 Hermes의 세션은 찾지 않습니다. 지금 창에서 열려 있는 세션과 60일보다 오래된 세션은 빠집니다.
- {{kbd:↑}} {{kbd:↓}}로 고르고 {{kbd:Enter}}를 누르거나 {{ui:conversationSearch.resume}}를 누르면 새 창이 그 세션을 이어갑니다.
- {{ui:conversationSearch.export}}은 그 대화의 요청과 답을 Markdown 파일로 저장합니다.
- 원격 워크스페이스에서는 쓸 수 없습니다. 세션 기록이 그 컴퓨터에 있기 때문입니다.

## 탭과 분할 {#tabs-splits}
- 탭을 끌어 다른 그룹 가운데에 놓으면 탭으로 합쳐집니다({{ui:layout.drop.merge}}).
- 그룹의 왼쪽·오른쪽·위·아래 가장자리에 놓으면 그쪽으로 나뉩니다.
- 나눈 경계선을 끌면 크기가 바뀌고, 두 번 클릭하면 반반으로 나뉩니다.
- 끌던 중 {{kbd:Esc}}를 누르면 취소됩니다.
- 탭을 두 번 클릭하면 이름을 바꿉니다. 오른쪽 클릭하면 {{ui:menu.rename}}, {{ui:menu.focusPane}}, {{ui:menu.closeTab}} 항목이 있습니다.

배치는 워크스페이스마다 저장됩니다. 워크스페이스를 바꿔도 각자의 배치가 그대로 남습니다.

### 실행 창 메뉴 {#pane-menu}
실행 창 머리의 … 단추에는 다음이 있습니다.
- {{ui:menu.rename}}
- {{ui:menu.focusPane}}: 이 창만 크게 봅니다. 다시 누르면 {{ui:pane.menu.restoreLayout}}로 돌아갑니다.
- {{ui:pane.menu.copyLog}}: 에이전트 창의 실행 기록을 복사합니다. 터미널 창에는 이 자리에 {{ui:pane.menu.terminalHistory}} 항목이 있습니다([터미널 창](terminal-files-browser.md#terminal)).
- {{ui:pane.menu.newConversation}}: 다음 요청부터 이전 대화를 잇지 않습니다(에이전트 창).
- {{ui:menu.closePane}}

창 머리를 오른쪽 클릭하면 {{ui:menu.rename}}, {{ui:session.menu.focus}}, {{ui:session.menu.close}} 항목이 있고, 터미널 창에는 {{ui:terminal.history.title}} 항목도 있습니다.

## 단축키 {#shortcuts}
| 단축키 | 동작 |
|---|---|
| {{kbd:⌘O}} | {{ui:menu.openProject}} |
| {{kbd:⌘N}} | {{ui:menu.newClaudePane}} |
| {{kbd:⌘T}} | {{ui:menu.addTerminalPane}} |
| {{kbd:⇧⌘E}} | {{ui:menu.showFiles}} |
| {{kbd:⇧⌘F}} | {{ui:conversationSearch.open}} |
| {{kbd:⌘K}} | {{ui:menu.searchWorkspaces}} |
| {{kbd:⌃⌘S}} | {{ui:sidebar.collapse}} / {{ui:sidebar.expand}} |
| {{kbd:⌘,}} | {{ui:menu.settings}} |
| {{kbd:Enter}} | 요청 보내기 |
| {{kbd:⇧Enter}} | 줄바꿈 |
| {{kbd:⌘Enter}} | 실행 중인 창에서 새 worktree에서 바로 실행([마이티 오케스트레이션](delegation.md#worktree)이 켜진, git 저장소의 창). 그 밖의 실행 중인 창에서는 대기열에 추가 |
| {{kbd:Esc}} | 시트 닫기, 슬래시 목록 닫기 |
