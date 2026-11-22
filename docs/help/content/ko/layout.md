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
- {{ui:review.sidebar}}: 검토할 변경이 있는 창과 요청을 모아 봅니다([검토 대기열](#review)).
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

## 검토 대기열 {#review}
검토할 것을 한 곳에 모아 봅니다. 사이드바 {{ui:phone.dashboard.title}} 아래의 {{ui:review.sidebar}} 항목을 누르거나 {{kbd:⇧⌘R}}(Windows: {{kbd:Ctrl+Shift+R}})를 누르면 {{ui:review.open}} 패널이 열립니다. 기다리는 개수가 항목 옆에 숫자로 붙습니다.
- 나오는 것: 변경을 남기고 끝난 [worktree 창](delegation.md#pane-worktree)과 [자식 창](delegation.md#states), 그리고 창마다 마지막으로 끝난 요청 중 바뀐 파일이 보관된 요청([요청 변경 검토](agent-pane.md#request-changes))입니다. 아직 일하는 창은 끝날 때까지 나오지 않습니다.
- 줄마다 창 이름, 에이전트, 종류, 워크스페이스, 브랜치와 기준 브랜치, 바뀐 파일 수와 더한·지운 줄 수, 상태({{ui:review.status.ready}}, {{ui:review.status.conflict}}, {{ui:review.status.merged}})가 보입니다. 충돌이 맨 위, 병합된 것이 맨 아래에 옵니다.
- 단추: {{ui:review.action.changes}}, {{ui:review.action.merge}}(커밋하지 않은 파일이 있는 worktree 창은 {{ui:review.action.commitMerge}}), {{ui:editor.open}}, {{ui:review.action.pr}}(worktree 창), {{ui:review.action.discard}}, {{ui:review.action.focus}}, {{ui:review.action.dismiss}}. 병합·버리기·PR은 창 위 막대나 자식 카드의 같은 동작이라 같은 확인을 거칩니다. 요청의 {{ui:review.action.changes}}는 요청 변경 검토 창을 엽니다.
- {{ui:review.action.dismiss}}를 누르면 그 항목은 작업이 다시 바뀔 때까지 숨겨집니다.
- 목록은 실행이 끝날 때, 패널을 열 때, {{ui:review.refresh}}을 누를 때 git을 다시 읽습니다. 화면을 그릴 때마다 읽지는 않습니다.
- worktree 창과 자식 창의 줄에는 머지 전 점검 요약({{ui:review.checks.ok}} 또는 경고 개수)이 붙습니다. 누르면 그 창의 {{ui:git.checks.tab}} 탭이 열립니다([점검](#checks)). 요약은 마지막으로 읽은 점검과 앱이 이미 아는 것(커밋 안 된 파일, 남은 할 일, 창의 PR 상태)으로 만들고, 이 목록 때문에 gh를 따로 부르지는 않습니다.

## 커밋·PR {#git}
워크스페이스의 git 작업을 한 패널에서 합니다. 사이드바 워크스페이스 메뉴의 {{ui:git.workbench.open}} 항목, 워크스페이스 머리의 브랜치 표시, 입력창의 `/git`으로 엽니다. worktree 창에서 열면 그 창의 worktree에서 일합니다.
- {{ui:git.checks.changesTab}} 탭: 바뀐 파일과 diff, 파일·청크 단위 스테이징, 브랜치 전환, 커밋 메시지 초안, 커밋, push, PR 만들기, 실패한 CI 고치기 요청이 있습니다. 기본 브랜치에서는 PR을 만들지 않습니다.
- {{ui:git.checks.tab}} 탭: 병합이나 머지 전에 확인할 것을 한 화면에 모읍니다([점검](#checks)).

### 점검 {#checks}
worktree 창(그 밖의 창은 지금 브랜치)에 대해 다섯 가지를 보여 줍니다.
- {{ui:git.checks.git.title}}: 커밋 안 된 변경 수와, 기준 브랜치보다 앞선(↑)·뒤처진(↓) 커밋 수.
- {{ui:git.checks.pr.title}}: PR(GitLab은 MR) 번호, 상태, 초안 여부, 충돌, 리뷰 결과(승인, 변경 요청).
- {{ui:git.checks.ci.title}}: 검사마다 결과. 실패한 검사가 있으면 {{ui:git.pr.fixChecks}} 단추로 실패 로그와 함께 에이전트에게 맡깁니다.
- {{ui:git.checks.threads.title}}: 해결 안 된 리뷰 스레드의 파일과 줄, 작성자, 첫 댓글, 답글 수, 링크.
- {{ui:git.checks.checklist.title}}: 그 창 마지막 요청의 체크리스트에서 끝나지 않은 항목.

- 탭을 열 때와, 열려 있는 동안 30초마다 다시 읽습니다. 닫혀 있으면 읽지 않습니다.
- 항목마다 따로 읽으므로, gh·glab이 없거나 로그인하지 않았거나 PR이 없으면 그 항목만 {{ui:git.checks.unknown}}으로 나옵니다.
- 스레드를 골라 {{ui:git.checks.threads.toComposer}} 단추를 누르면 "PR #42 리뷰 의견을 반영해 주세요." 아래에 `파일:줄 (작성자): 댓글` 목록이 그 창의 입력창에 들어갑니다. 바로 보내지는 않습니다. 비밀값처럼 보이는 줄은 가리고 링크의 계정 정보는 지우며, 2,000자까지만 넣습니다.
- 병합 전 확인: 실패하거나 진행 중인 검사, 해결 안 된 스레드, 남은 할 일, 커밋 안 된 변경(커밋하고 병합은 빼고), 초안, 충돌, 변경 요청 중 하나라도 있으면 worktree 막대, 자식 카드, 검토 대기열의 병합 단추가 이유를 보여 주고 한 번 더 묻습니다. {{ui:git.checks.merge.confirm}}을 누르면 그대로 병합합니다. 막지는 않습니다.

## 실행 창 추가 {#add-pane}
실행 창은 AI 에이전트(Claude, Codex, Antigravity, pi, opencode, Hermes), 터미널, 파일 중 하나입니다.

1. 사이드바의 {{ui:workspace.addPane}} 단추를 누릅니다.
2. 만들 창을 고릅니다: 에이전트(Claude, Codex, Antigravity, pi, opencode, Hermes) 실행 창, {{ui:workspace.newTerminal}}, 또는 {{ui:menu.showFiles}}({{kbd:⇧⌘E}}). 메뉴 맨 아래 묶음에는 워크스페이스를 추가하는 {{ui:workspace.add.existing}}, {{ui:workspace.add.newFolder}}, {{ui:workspace.add.fromGit}} 세 항목이 있습니다([워크스페이스 열기](getting-started.md#open-workspace)).
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
| {{kbd:⇧⌘R}} | {{ui:review.open}} |
| {{kbd:⇧⌘U}} | {{ui:attention.next}} |
| {{kbd:⌘K}} | {{ui:menu.searchWorkspaces}} |
| {{kbd:⌃⌘S}} | {{ui:sidebar.collapse}} / {{ui:sidebar.expand}} |
| {{kbd:⌘,}} | {{ui:menu.settings}} |
| {{kbd:Enter}} | 요청 보내기. 실행 중인 창에서는 {{ui:composer.busySend.title}}에 따라 대기열에 넣거나 고른 에이전트의 새 worktree에서 바로 실행([실행 중에 이어서 요청하기](agent-pane.md#queue)) |
| {{kbd:⇧Enter}} | 줄바꿈 |
| {{kbd:⌘Enter}} | 실행 중인 창에서 새 worktree에서 바로 실행([마이티 오케스트레이션](delegation.md#worktree)이 켜진, git 저장소의 창). 그 밖의 실행 중인 창에서는 대기열에 추가 |
| {{kbd:Esc}} | 시트 닫기, 슬래시 목록 닫기 |
| {{kbd:⌘=}} / {{kbd:⌘-}} / {{kbd:⌘0}} | {{ui:menu.textBigger}} / {{ui:menu.textSmaller}} / {{ui:menu.textActualSize}} |
| {{kbd:⌘?}} | {{ui:menu.help}} |
