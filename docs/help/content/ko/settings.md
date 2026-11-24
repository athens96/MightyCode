---
title: 설정
order: 8
section: settings
---
{{ui:menu.settings}}({{kbd:⌘,}})이나 사이드바 아래의 설정 단추로 엽니다. 왼쪽에서 탭을 고르고, {{ui:settings.closeButton}} 단추나 {{kbd:Esc}}로 닫습니다. 마지막에 본 탭을 기억합니다. 탭은 {{ui:settings.nav.general}}, {{ui:settings.nav.models}}, {{ui:settings.nav.styles}}, {{ui:settings.remote.navTitle}}, {{ui:settings.nav.companion}}, {{ui:settings.nav.tools}}, {{ui:settings.nav.about}} 순서입니다.

## 일반 {#general}
{{ui:settings.nav.general}} 탭의 {{ui:settings.display.sectionTitle}} 항목에서 앱의 모양을 바꿉니다.
- {{ui:settings.display.themeLabel}}: {{ui:settings.display.themeDarkWindows}} 또는 {{ui:settings.display.themeLightWindows}}. 사이드바 아래 단추로도 바꿉니다.
- {{ui:settings.display.languageLabel}}: 앱의 언어. [언어 바꾸기](getting-started.md#language)를 보세요.

![[settings-general]]

### 글자 크기와 글꼴 {#text}
같은 탭의 {{ui:settings.text.sectionTitle}} 항목에서 앱 전체의 글자를 바꿉니다.
- {{ui:settings.text.sizeLabel}}: 85%에서 150% 사이에서 고릅니다. 대화, 입력창, 사이드바, 설정, 터미널이 함께 커지고 작아집니다.
- {{ui:settings.text.uiFontLabel}}: 사이드바, 메뉴, 단추, 설정 같은 앱 화면의 글꼴입니다. 이 컴퓨터에 설치된 글꼴 가운데 고르고, {{ui:settings.text.fontSystem}}은 운영체제의 기본 글꼴(Mac은 macOS 글꼴, Windows는 Segoe UI)입니다.
- {{ui:settings.text.transcriptFontLabel}}: 에이전트의 답, 내가 보낸 말, 입력창, 마이티 카드와 보드의 본문 글꼴입니다. {{ui:settings.text.sameAsInterface}}로 두면 화면 글꼴을 따라갑니다.
- {{ui:settings.text.codeFontLabel}}: 코드 블록, 인라인 코드, 도구 출력과 명령 줄, 터미널 창의 글꼴입니다. 고정폭 글꼴만 고를 수 있습니다. 설치된 고정폭 글꼴과 함께 JetBrains Mono, D2Coding, Fira Code, Cascadia Code·Cascadia Mono, Consolas 같은 코딩 글꼴이 설치되어 있으면 나옵니다. 맨 위의 기본값은 Mac에서는 코드에 SF Mono, 터미널에 Menlo를, Windows에서는 Cascadia Mono(없으면 Consolas)를 씁니다. Mac에서 고른 글꼴이 없어지면 터미널은 Menlo로 돌아갑니다. 열려 있는 터미널에도 바로 적용됩니다. 경로, 버전, 개수 같은 앱 화면의 고정폭 글자는 시스템 고정폭 글꼴을 그대로 씁니다.
- 글꼴마다 아래에 그 글꼴로 쓴 한 줄 미리 보기가 나옵니다.
- {{ui:settings.text.reset}} 단추는 크기와 세 글꼴을 처음 상태로 되돌립니다.

Mac에서는 보기 메뉴의 {{ui:menu.textBigger}}({{kbd:⌘=}}), {{ui:menu.textSmaller}}({{kbd:⌘-}}), {{ui:menu.textActualSize}}({{kbd:⌘0}})로, Windows에서는 {{kbd:Ctrl+=}} / {{kbd:Ctrl+-}} / {{kbd:Ctrl+0}}으로도 크기를 바꿉니다(Windows에서 터미널 창에 커서가 있으면 이 키는 터미널이 씁니다). 바꾼 값은 바로 적용되고, 앱을 다시 열어도 그대로입니다.

### 에이전트 창 {#agents}
같은 탭의 {{ui:settings.agents.sectionTitle}} 항목에는 에이전트 창에 관한 설정과 키 안내가 있습니다.
- {{ui:settings.display.statusLineToggle}}: Claude 설정의 상태 줄 명령을 실행해 입력창 아래에 보여 줍니다.
- {{ui:settings.delegation.toggle}}: 기본으로 켜져 있습니다. Claude 창이 자기 worktree를 가진 자식 창을 열 수 있고, 실행 중인 Claude 창에서 {{kbd:⌘Enter}}를 누르면 요청이 새 자식 창에서 실행됩니다. [마이티 오케스트레이션](delegation.md#enable)을 보세요.
- {{ui:settings.agents.checklistToggle}}: 기본으로 켜져 있습니다. 켜져 있으면 Gemini, Codex, opencode, Hermes에게 작업하는 동안 계획(체크리스트)을 남겨 달라는 짧은 지시를 함께 보내, 진행 상황 패널에 실제 계획 단계가 보입니다. 창의 요청에는 입력한 내용만 남습니다. 끄거나 계획이 오지 않으면 에이전트가 하는 일이 단계로 보입니다([마이티 보기](agent-pane.md#diagram)).
- {{ui:settings.editor.label}}: {{ui:editor.open}}가 시작할 편집기입니다. 자동(찾은 것 중 첫 번째), 이 컴퓨터에서 찾은 편집기, {{ui:settings.editor.system}} 중에서 고릅니다. 설정을 열 때마다 다시 찾습니다([편집기에서 열기](terminal-files-browser.md#open-in-editor)).
- 아래에 키 안내가 있습니다. {{kbd:Enter}}로 보내고 {{kbd:Shift+Enter}}로 줄을 바꿉니다. 탭을 끌어 다른 탭과 합치거나 가장자리로 옮겨 나눕니다. 명령 실행 창은 Mac에서는 로그인 셸, Windows에서는 PowerShell(없으면 cmd)을 여는 대화형 터미널입니다.
- 이슈 트래커(MightyWork, GitHub, Linear, Jira, GitLab) 연결은 설정 창이 아니라 워크스페이스마다 사이드바 워크스페이스 메뉴의 {{ui:integrations.menu.settings}}에서 합니다. [이슈에서 시작하기](delegation.md#issues)를 보세요.

### 백그라운드 작업 {#background}
같은 탭의 {{ui:daemon.settings.sectionTitle}} 항목에서 {{ui:daemon.settings.toggle}} 스위치를 켜면, 앱을 종료하거나 자동 업데이트로 다시 시작해도 에이전트 실행과 터미널이 멈추지 않습니다. 처음에는 꺼져 있습니다.
- 켜면 앱이 같은 실행 파일을 창 없는 백그라운드 데몬으로 띄웁니다(`MightyCode --daemon`). 데이터 폴더마다 하나만 실행되고, 그 뒤에 시작하는 에이전트 실행(Claude, Codex, Antigravity, pi, opencode, Hermes)과 터미널 창의 셸은 데몬이 맡습니다. 화면, 기록, 승인 카드는 그대로 앱이 그립니다.
- 실행 중인 작업이 있을 때 앱을 종료하면 {{ui:daemon.quit.keep}}와 {{ui:daemon.quit.stopAll}} 중에서 고릅니다. 자동 업데이트로 다시 시작할 때는 늘 계속 둡니다.
- 앱을 다시 열면 이어집니다. 에이전트 창은 앱을 닫은 동안 나온 출력부터 이어서 보여 주고, 기다리던 승인·질문 카드(pi의 대화 상자, opencode·Hermes의 질문 포함)도 다시 나와 그 창에서 답할 수 있습니다. [위임된 자식 창](delegation.md)도 이어지며, 끝나면 보고와 worktree 흐름이 평소처럼 부모에게 갑니다. 터미널 창은 창을 열 때 셸과 화면 기록(최근 2MB)이 그대로 돌아옵니다. 데몬은 실행마다 출력의 최근 8MB를 보관합니다.
- 상태 줄에는 데몬의 PID, 실행 중인 작업 수, 보관 중인 출력의 크기가 보입니다. {{ui:daemon.settings.stop}} 단추는 데몬과 그 안의 작업을 모두 끝냅니다. 실행 중인 작업도 연결된 앱도 없으면 데몬은 10분 뒤 스스로 종료합니다.

> [!note]
> 이어지는 것은 모든 에이전트의 실행과 터미널 창입니다. 셸 명령 실행과 샌드박스로 도는 실행은 앱을 닫을 때 멈춥니다. 다시 붙은 실행도 Mighty 연결(터미널·위임 도구, 활동 표시)을 그대로 씁니다. 앱을 닫기 직전에 시작된 도구 줄이나 답은 끝부분만 보일 수 있고, Antigravity가 앱을 닫기 전에 거절된 작업은 턴이 끝난 뒤의 권한 카드에 나오지 않습니다. 기다리던 요청이 너무 커서 다음 앱에 넘길 수 없으면 앱을 닫을 때 거부로 답하고 창에 그렇게 적습니다. 데몬이 비정상으로 끝나면 그 안의 작업도 끝나고 창은 멈춤으로 바뀝니다. 업데이트로 데몬과 앱의 버전이 달라지면 앱은 연결하지 않고, 이전 데몬의 작업을 멈출지 묻습니다. 데몬 연결은 이 컴퓨터의 같은 사용자만 할 수 있습니다(Mac은 권한 0600의 소켓, Windows는 현재 사용자만 여는 named pipe, 데이터 폴더 `daemon/token`의 토큰).

### 창 없이 실행 (서버) {#server}
창을 열지 않고 Mighty Code 전체(휴대폰 연결, 원격 미러, `mighty` 명령, 예약과 트리거, 백그라운드 데몬 연결)를 실행할 수 있습니다. 로그인해 둔 Mac이나 PC를 휴대폰이나 스크립트로 쓸 때 알맞습니다.
- `mighty server start`는 앱을 창 없이 시작하고(`MightyCode --server`, 데몬의 `--daemon --headless`와 다릅니다), 앱이 답할 때까지 기다린 뒤 상태를 JSON으로 보여 줍니다. Mac에서는 Dock 아이콘과 메뉴 막대가 보이지 않습니다.
- `mighty status`, `mighty agent new`, `send`, `wait` 같은 명령은 그대로 씁니다. `mighty server status`는 창이 있는지, 버전, PID, 실행 시간을 보여 줍니다.
- 창이 필요하면 앱을 다시 엽니다(Mac은 Dock이나 Finder에서, Windows는 앱을 한 번 더 실행). 같은 데이터 폴더의 앱이 둘 실행되지 않고, 실행 중인 서버가 창을 엽니다. `mighty server show`도 같습니다. 이 창을 닫으면 서버는 창 없이 계속 실행되고, ⌘Q를 누르면 {{ui:server.quit.closeWindow}}와 {{ui:server.quit.stop}} 중에서 고릅니다.
- `mighty server stop`은 평소처럼 앱을 종료합니다. {{ui:daemon.settings.toggle}}이 켜져 있으면 데몬의 작업은 계속 두고, `--stop-all`을 주면 함께 멈춥니다. Mac의 SIGTERM과 Windows의 콘솔 닫기도 같은 종료입니다.
- {{ui:daemon.settings.sectionTitle}}의 {{ui:server.settings.loginToggle}}을 켜면 로그인할 때 서버로 시작합니다(Mac은 `~/Library/LaunchAgents`의 LaunchAgent, Windows는 현재 사용자의 Run 레지스트리 키). 처음에는 꺼져 있고 다음 로그인부터 적용됩니다.

> [!note]
> 터미널 창의 셸은 창이 그 창을 처음 보일 때 시작합니다(창과 함께 시작할 때와 같습니다). 창을 닫아도 이미 실행 중인 셸은 계속 돌고 `mighty pane read`로 읽을 수 있습니다. 휴대폰 연결 승인처럼 사람의 확인이 필요한 일이 생기면 창이 열립니다.

## 모델 {#models}
{{ui:settings.nav.models}} 탭의 {{ui:settings.phaseModels.sectionTitle}}에서 작업 단계별로 쓸 모델을 이 컴퓨터 전체에 정합니다. 단계는 {{ui:settings.phaseModels.phase.planning}}, {{ui:settings.phaseModels.phase.execution}}, {{ui:settings.phaseModels.phase.review}}, {{ui:settings.phaseModels.phase.subagents}}입니다.
- {{ui:settings.phaseModels.defaultOption}}으로 두면 CLI가 정합니다.
- {{ui:settings.phaseModels.paneModel}}은 실행 창의 모델 칩에서 고른 모델을 씁니다.
- 목록에 없는 모델 이름은 {{ui:settings.phaseModels.registeredTitle}}에 {{ui:settings.phaseModels.addButton}}합니다. 그 모델이 사고 강도를 지원하면 {{ui:settings.phaseModels.supportsEffortLabel}}을 켭니다.

![[settings-models]]

## 스타일 {#styles}
{{ui:settings.nav.styles}} 탭에서 스타일 목록을 관리합니다. 파일에서 스타일을 등록하고, 내용을 확인해 허용하거나 차단합니다. 목록이 맞지 않으면 {{ui:settings.styles.rescanButton}} 단추를 누릅니다. 스타일의 ID, 해시, 개수는 이름 위에 포인터를 올리면 보입니다. 자세한 내용은 [스타일](styles.md#custom)에 있습니다.

![[settings-styles]]

## 원격 연결 {#remote}
{{ui:settings.remote.navTitle}} 탭에서 다른 컴퓨터(Mac·Windows)나 휴대폰이 이 컴퓨터의 워크스페이스를 쓰게 하거나, 다른 컴퓨터의 워크스페이스를 엽니다. 맨 위의 안내 아래에 탭이 셋 있습니다.
- {{ui:settings.remote.shareTitle}}: 위쪽 상태 줄 옆의 {{ui:settings.remote.startSharing}} / {{ui:settings.remote.stopSharing}} 단추로 공유를 켜고 끕니다. {{ui:settings.remote.pickWorkspaces}}를 고르고, 아래 {{ui:settings.remote.methodsTitle}}에서 {{ui:settings.remote.viaPairing}}과 {{ui:settings.remote.viaTailscale}} 줄을 봅니다. [이 컴퓨터 공유](remote.md#share)를 보세요.
- {{ui:settings.remote.connectTitle}}: 페어링 링크나 Tailscale 주소로 다른 컴퓨터에 연결하고, 아래 {{ui:settings.remote.connectionsTitle}} 목록에서 연결을 관리합니다. [다른 컴퓨터 연결](remote.md#connect)을 보세요.
- {{ui:settings.remote.phoneTab}}: 휴대폰을 연결합니다. 아래 [휴대폰](#mobile)을 보세요.

{{platform:windows}} {{ui:settings.remote.shareTitle}}와 {{ui:settings.remote.phoneTab}} 탭 아래에 {{ui:settings.remote.firewallTitle}} 항목이 있습니다. [Windows 방화벽](remote.md#firewall)을 보세요.

### 휴대폰 {#mobile}
{{ui:settings.remote.navTitle}} › {{ui:settings.remote.phoneTab}} 탭에서 휴대폰 연결을 허용하고, QR 코드로 휴대폰을 연결하고, 연결된 기기를 관리합니다. 릴레이를 따로 준비할 필요는 없습니다. [휴대폰 원격](phone.md#pair)을 보세요. 접힌 {{ui:settings.advancedTitle}} 항목에는 직접 띄운 릴레이를 쓸 때의 {{ui:settings.mobileRemote.relayLabel}} 칸({{ui:settings.mobileRemote.applyButton}} 단추, 아래에 기본 주소)과 {{ui:settings.mobileRemote.legacyAppsToggle}} 스위치가 있습니다. 둘 중 하나라도 기본값에서 바꿨으면 처음부터 펼쳐져 있습니다.

화면 보기·조작은 지금 잠시 멈춰 있어서 설정에 그 항목이 없습니다([화면 보기](phone.md#screen)).

## Mighty Pet과 알림 {#companion}
{{ui:settings.nav.companion}} 탭에서 Mighty Pet과 완료 알림을 정합니다.
- {{ui:companion.settings.enabled}}: 화면 위를 다니는 너구리 Mighty Pet을 켭니다. Mighty Pet을 끌어 옮기고, 누르면 말풍선이 열리고 닫힙니다.
- {{ui:companion.settings.task}}: 말풍선에 요청과 지금 하는 일, 걸린 시간을 보여 줍니다. 말풍선에서 승인({{ui:permission.allowOnce}} / {{ui:permission.deny}})에 답할 수도 있습니다. 승인·질문·계획 말풍선의 기호 단추는 요청 전체를 보거나 답을 직접 쓸 수 있게 그 실행 창을 엽니다.
- 말풍선 위에는 그 창의 최근 요청 {{ui:graph.progress.title}} 카드가 붙습니다. 실행 창의 진행 패널과 같은 내용으로, 요청 번호와 끝난 단계 수, 상태, 모든 단계와 백그라운드 작업이 각자 걸린 시간과 함께 보입니다. 줄이 여덟 줄보다 많으면 카드 안에서 스크롤해 보고, 처음에는 지금 하는 단계가 보이게 열립니다. 카드 머리를 누르면 머리만 남게 접히고 다시 누르면 펼쳐지며, 접은 상태는 다음에 앱을 열어도 그대로입니다. 단계 줄을 누르면 그 실행 창이 열립니다. 단계도 백그라운드 작업도 없는 요청이거나 말풍선이 승인·질문·계획을 보이는 동안, 말풍선이 닫혀 있을 때는 카드가 없습니다.
- {{ui:companion.settings.pet}}: 펫을 고릅니다. {{ui:companion.settings.import}}로 다른 펫을 더하고, {{ui:companion.settings.reload}} 단추로 펫 목록을 다시 읽고, {{ui:companion.settings.resetPosition}} 단추로 Mighty Pet의 위치와 말풍선 크기를 처음대로 돌립니다.
- {{ui:companion.settings.motion}}: 움직임을 줄입니다.
- {{ui:companion.settings.macNotifications}}(Windows는 {{ui:windows.notifications.toggleLabel}}): 요청이 성공적으로 끝나면 Mac 알림을 보냅니다. 알림을 누르면 그 실행 창이 열립니다. 알림에는 요청이나 결과 내용이 들어가지 않습니다.
- {{ui:terminalNotifications.toggle}}: 터미널 창이나 CLI가 보낸 알림과 벨을 그 창에 표시합니다. OS 알림은 앱이 뒤에 있거나 다른 창을 보고 있을 때만 보냅니다. 처음에는 켜져 있습니다([알림과 벨](terminal-files-browser.md#terminal-signals)).
- 그 아래에 운영체제의 알림 허용 상태가 보이고, {{ui:windows.notifications.settingsButton}} 단추로 시스템의 알림 설정을 엽니다.

![[settings-companion]]

![[companion-pet]]

## 자동화와 명령줄 {#automation}
{{ui:settings.nav.companion}} 탭의 {{ui:automation.settings.sectionTitle}} 상자에서 에이전트 창의 상태가 바뀔 때 알릴 곳을 정합니다. 요청 글, 파일 내용, 토큰은 어디에도 보내지 않습니다.
- {{ui:automation.settings.events}}: {{ui:automation.event.started}}, {{ui:automation.event.blocked}}, {{ui:automation.event.done}}, {{ui:automation.event.failed}}, {{ui:automation.event.stopped}} 중에서 고릅니다. 처음에는 승인·질문 대기, 완료, 실패가 켜져 있습니다. 웹훅과 상태 명령이 함께 따릅니다.
- {{ui:automation.webhooks.title}}: Slack, Discord, 다른 HTTPS 주소를 다섯 개까지 넣습니다. 주소는 시스템 키체인(Windows는 자격 증명 관리자)에 저장되고 화면에는 일부만 보입니다. 보내는 내용은 제목, 상태, 워크스페이스와 창 이름뿐입니다. {{ui:automation.webhooks.test}} 단추로 시험 메시지를 보냅니다. 같은 창의 같은 상태는 5초에 한 번, 주소마다 1분에 여섯 번까지 보내고, 서버 오류가 나면 한 번 더 보냅니다.
- {{ui:automation.hook.title}}: 고른 상태가 되면 이 명령을 그 창의 폴더에서 실행합니다. 상태는 인자가 아니라 `MIGHTY_EVENT`, `MIGHTY_STATE`, `MIGHTY_PREVIOUS_STATE`, `MIGHTY_REASON`, `MIGHTY_PANE_ID`, `MIGHTY_PANE_TITLE`, `MIGHTY_AGENT`, `MIGHTY_WORKSPACE_ID`, `MIGHTY_WORKSPACE_NAME`, `MIGHTY_WORKSPACE_PATH` 환경 변수로 받습니다. 20초가 지나면 멈춥니다.
- {{ui:automation.api.title}}: 실행 중인 앱을 `mighty` 명령으로 다룰 수 있는지 보여 줍니다. {{ui:automation.api.rotate}} 단추는 토큰을 바꾸고 열려 있던 연결을 끊습니다.

저장소마다 상태 명령을 두려면 `.mighty.local.yaml`에 적습니다. [다른 설정처럼](delegation.md#mighty-yaml) 승인한 뒤에만 실행되고, 함께 쓰는 `.mighty.yaml`에는 넣을 수 없습니다.

```yaml
hooks:
  command: ./scripts/notify.sh
  on: [blocked, done, failed]   # started, blocked, done, failed, stopped
  timeoutSeconds: 20            # 1-300
```

### mighty 명령 {#cli}
Mac에서는 {{ui:menu.workspace}} 메뉴의 {{ui:shellCommand.install}} 항목으로 `~/.local/bin/mighty`를 설치합니다. `mighty`, `mighty 폴더`, `mighty "요청"`은 예전처럼 폴더를 엽니다. 이름이 명령과 같은 폴더는 `mighty ./status`처럼 엽니다. 아래 명령은 실행 중인 앱에 연결하고 JSON으로 답합니다. 0.6.0 전에 설치했다면 한 번 다시 설치하세요.

Windows에서는 설치 폴더의 `bin\mighty.exe`가 `mighty` 명령입니다. 설치 프로그램의 "mighty 명령을 PATH에 추가"(기본으로 켜짐)가 그 폴더를 내 사용자 PATH에 넣습니다. 앱을 업데이트로만 받았다면 {{ui:settings.nav.companion}} 탭의 {{ui:automation.api.title}}에서 {{ui:shellCommand.windows.addPath}}를 누르세요. 그 뒤 새로 연 cmd, PowerShell, Windows Terminal에서 Mac과 같은 명령을 씁니다. 인자, 파이프(`| jq`, `> out.json`, `-`로 표준 입력), 끝 코드, Ctrl+C가 그대로 전달됩니다. 폴더 열기(`mighty 폴더`)는 아직 Mac에서만 됩니다. `MightyCode.exe --cli status`처럼 앱을 직접 불러도 출력이 콘솔에 보이지만, 셸이 기다리지 않아 프롬프트 뒤에 찍힐 수 있으니 `mighty`를 쓰세요.

```sh
mighty status                                   # 워크스페이스, 창, 상태, 주의 표시, worktree, 포트 범위
mighty workspace list
mighty pane list --workspace <id>
mighty pane read <pane> --lines 100
mighty agent new --workspace <id> --agent codex --worktree
mighty agent send <pane> "테스트를 고쳐 줘"
mighty agent wait <pane> --until done --timeout 600
mighty pane wait-output <pane> --regex "passed|failed" --timeout 300
mighty events --type pane.state
mighty server start                             # 창 없이 시작 (status, show, stop [--stop-all])
```

- 창 id는 앱이 쓰는 그대로이고 바뀌지 않습니다. 상태는 `idle`, `running`, `blocked`(승인·질문 대기), `done`, `failed`, `stopped`입니다.
- `agent send`는 입력창에서 보낸 것과 같습니다. 실행 중이면 대기열에 들어갑니다. `-`를 주면 표준 입력에서 읽습니다.
- `agent wait`와 `pane wait-output`은 상태와 출력이 바뀔 때만 깨어납니다. `pane wait-output`은 기다리기 시작한 뒤의 새 줄을 한 줄씩 봅니다.
- `pane read`와 출력에서 비밀값처럼 보이는 줄은 가려집니다. `events`는 한 줄에 사건 하나를 씁니다(`pane.state`, `attention`, `run.completed`와 비용).
- 끝 코드는 성공 0, 오류 1, 시간 초과 2입니다.
- 연결은 이 컴퓨터의 같은 사용자만 할 수 있습니다(Mac은 권한 0600의 소켓, Windows는 현재 사용자만 여는 named pipe). 요청마다 데이터 폴더 `control/token`의 토큰이 필요합니다. `--profile`로 실행한 앱에는 `mighty --profile <폴더> status`로 연결합니다.
- Windows에서 앱을 관리자 권한으로 실행했다면 `mighty`도 관리자 권한으로 연 터미널에서만 연결됩니다. 일반 권한 터미널은 까닭을 알리고 끝 코드 1로 끝납니다(그렇지 않으면 내 계정의 아무 프로그램이나 관리자 권한으로 에이전트를 움직일 수 있습니다). 관리자 권한 터미널에서는 일반 권한 앱에도 연결됩니다. 다른 사용자 계정은 관리자여도 연결할 수 없습니다.

### 예약과 트리거 {#schedules}
{{ui:settings.nav.companion}} 탭의 {{ui:automation.jobs.sectionTitle}} 상자에서 정한 시간이나 이벤트에 에이전트 작업을 시작합니다. 워크스페이스를 고르면 그 워크스페이스의 예약과 트리거, 다음 실행 시각, 마지막 결과가 보입니다. 맨 위에는 오늘 쓴 금액과 하루 예산, 오늘의 자동화 요약이 있습니다.
- **예약**: 5칸 cron(`0 9 * * 1-5`) 또는 RRULE 일부(`FREQ=DAILY|WEEKLY|HOURLY`, `BYHOUR`, `BYMINUTE`, `BYDAY`)로 정합니다. 이 컴퓨터의 시간대를 따르고, 서머타임으로 건너뛴 시각은 바로 다음에 한 번, 두 번 오는 시각은 처음에 한 번 실행합니다.
- **트리거**: GitHub(PR 열림, 리뷰 요청, CI 실패), Linear(이슈가 고른 상태로 바뀜), 웹훅, 상시 점검(셸 명령을 몇 분마다 돌려 출력이 바뀔 때만 에이전트를 깨우고, 하루 상한까지만 깨움). GitHub는 `gh`로 로그인한 계정으로 5분에 한 번까지만 확인하고, 바뀐 것이 없거나 연결이 끊기면 더 드물게 확인합니다. Linear는 워크스페이스에 연결한 Linear를 씁니다.
- **실행 방식**: 새 창(필요하면 워크트리에서)에 프롬프트를 보내거나, {{ui:automation.form.heartbeat}}를 켜면 같은 이름의 창에 같은 대화로 보냅니다. 에이전트와 모델을 고를 수 있습니다.
- **예산**: 오늘 쓴 금액에 이 실행의 예상 비용을 더해 하루 예산을 넘으면 실행하지 않고 "건너뜀"으로 남깁니다.
- **알림**: 자동으로 시작한 작업이 승인이나 답을 기다리면 휴대폰과 OS 알림으로 바로 알립니다. 나머지는 실행 기록과 오늘 요약에 모읍니다. 모든 실행은 `automation.fired`, `automation.skipped`, `automation.blocked`, `automation.finished` 이벤트로 `mighty events`에서도 볼 수 있고, {{ui:automation.settings.events}}에서 {{ui:automation.event.automation}} 항목을 켜면 웹훅과 상태 명령도 따릅니다.
- {{ui:automation.history.title}}: 실행마다 시각, 상태, 비용(사용량 장부 기준), 결과 링크(창이 연 PR), 오류가 남습니다. {{ui:automation.history.retry}} 단추로 다시 실행하고, {{ui:automation.history.continue}} 단추로 그 창을 엽니다.

**앱이 꺼져 있을 때**: {{ui:automation.wake.toggle}}을 켜면(기본 꺼짐) 다음 예약 시각에 컴퓨터가 Mighty Code를 창 없이 시작합니다(Mac은 LaunchAgent, Windows는 작업 스케줄러, 관리자 권한 없이 내 계정으로). 시작한 앱은 놓친 예약을 처리하고, 실행과 승인·질문 대기가 모두 끝난 뒤 5분 동안 할 일이 없으면 다음 예약을 다시 등록하고 스스로 닫힙니다. 컴퓨터가 잠자거나 꺼져 있었으면 깨어난 뒤에 시작합니다. 앱이 이미 열려 있으면 열린 앱이 실행합니다. 상자에서 등록 여부, 다음 깨우기 시각, 마지막 깨우기 결과를 보고, {{ui:automation.wake.again}} 단추로 다시 등록할 수 있습니다. [{{ui:server.settings.loginToggle}}](#background)를 켜 두었다면 그 서버가 계속 실행됩니다.

> [!warning]
> **트리거**(GitHub, Linear, 상시 점검, 웹훅)는 앱이 실행 중일 때만 동작합니다. 창 없이 시작한 서버도 실행 중으로 칩니다. 컴퓨터가 깨우는 것은 예약뿐이므로, 앱을 닫아 둔 동안에도 트리거를 쓰려면 [{{ui:server.settings.loginToggle}}](#background)를 켜 두세요. {{ui:automation.wake.toggle}}을 끈 채 앱을 닫으면 예약도 멈추고([백그라운드 작업](#background)을 켜도 이미 실행 중인 작업만 이어짐), 놓친 예약은 다음에 앱을 열 때 가장 최근 것 하나만 실행합니다. {{ui:automation.form.skipMissed}}를 켜면 실행하지 않고 "건너뜀"으로만 남깁니다.

**웹훅 받기**: {{ui:automation.listener.toggle}}를 켜면 이 컴퓨터의 `127.0.0.1:<포트>`에서만 요청을 받습니다(기본 꺼짐). 웹훅 트리거마다 주소(`http://127.0.0.1:<포트>/hooks/<id>`)와 비밀 키가 있고, 비밀 키는 시스템 키체인에 저장됩니다. 요청 본문을 비밀 키로 HMAC-SHA256 서명해 `X-Mighty-Signature-256: sha256=<hex>`(GitHub의 `X-Hub-Signature-256`도 됨) 헤더로 보내야 합니다. 256KB를 넘는 본문, 서명이 틀린 요청, 같은 전달 ID(`X-Mighty-Delivery`)는 버립니다. 밖(GitHub, 다른 서비스)에서 받으려면 ngrok이나 Cloudflare Tunnel 같은 터널을 직접 앞에 두세요. 앱이 터널을 열지는 않습니다.

저장소에 함께 둘 예약과 트리거는 `.mighty.yaml`에 적습니다. [다른 설정처럼](delegation.md#mighty-yaml) 승인한 뒤에만 실행되고, 상자에서는 읽기 전용으로 보이며 승인 전이면 {{ui:automation.jobs.approve}} 단추가 나옵니다.

```yaml
schedules:
  deps-check:
    cron: "0 9 * * 1-5"        # 또는 rrule: FREQ=DAILY;BYHOUR=9
    prompt: 의존성을 점검하고 필요하면 PR을 여세요.
    agent: claude              # codex, gemini, pi, opencode, hermes
    model: sonnet
    worktree: true
    catchUp: once              # once(기본) 또는 skip
  night-watch:
    rrule: FREQ=DAILY;BYHOUR=22
    mode: heartbeat
    pane: 야간 점검
    prompt: 이어서 진행하세요.
triggers:
  fix-ci:
    github: [ci_failed]        # pr_opened, review_requested, ci_failed
    prompt: CI 실패를 고치세요.
    worktree: true
  ready-issues:
    linear: [Ready]
    prompt: 이 이슈를 시작하세요.
  deploy:
    webhook: true
    prompt: 배포 결과를 확인하세요.
  flaky-tests:
    standing: npm test 2>&1 | tail -5
    everyMinutes: 30           # 5-1440
    dailyCap: 3                # 1-50
    prompt: 테스트 결과가 바뀌었습니다. 원인을 찾아 고치세요.
```

## 구성 요소 {#components}
{{ui:settings.nav.tools}} 탭은 앱에 필요한 것이 준비됐는지 보여 주고, 에이전트 CLI의 설치, 로그인, 업데이트를 한곳에서 관리합니다. 이 컴퓨터의 운영체제에 맞는 항목만 보입니다.

맨 위 줄에는 보기 선택과 단추 둘이 있습니다.
- {{ui:settings.components.filterAll}} / 문제 있음: 문제 있음을 고르면 손볼 것이 있는 줄만 보입니다. 설치 안 됨, 로그인되지 않음, 업데이트 있음, 실패, 확인 필요인 항목입니다. 옆의 숫자가 그 개수입니다.
- 새로고침(↻, {{ui:setup.button.recheck}}): PATH를 다시 읽고 모든 항목을 다시 확인합니다.
- {{ui:settings.components.updateAllButton}}: 설치된 에이전트 CLI를 지금 모두 업데이트합니다. CLI마다 처음 설치한 방법(npm, Homebrew 등)을 그대로 씁니다.

목록은 세 묶음입니다. {{ui:setup.group.agent}}(Claude, Codex, Antigravity, pi, opencode, Hermes), {{ui:settings.components.groupTools}}(Git(Mac에서는 Xcode 명령줄 도구), Homebrew(Mac), Node.js, Bun(Mac), uv, Tailscale, gh, glab), {{ui:settings.components.groupPlugins}}(ouroboros, omc, gstack(Mac))입니다. 항목마다 한 줄이고, 줄에는 펼침 표시(▸), 이름, 버전, 에이전트 CLI라면 로그인한 계정이 있습니다. 오른쪽에는 상태와, 지금 가장 필요한 단추 하나가 있습니다.
- 상태: {{ui:setup.status.installed}}, {{ui:settings.components.statusInstalled}}(에이전트 CLI가 설치되고 로그인까지 됨), {{ui:settings.components.statusUpdateAvailable}}, {{ui:settings.cliAccounts.summarySignedOut}}, {{ui:settings.cliAccounts.statusNotInstalled}}, {{ui:setup.status.attention}}, {{ui:setup.status.installing}}, {{ui:setup.status.failed}}.
- 단추: 없으면 {{ui:settings.components.installButton}}(내려받기 페이지로만 설치하는 항목은 {{ui:setup.button.openPage}}), 로그인이 안 됐으면 {{ui:settings.cliAccounts.buttonLogin}}, 오래됐으면 {{ui:setup.button.update}}. 문제가 없으면 단추가 없습니다.

줄을 누르면 그 자리에서 펼쳐집니다(▾). 펼친 칸에는 자세한 내용(어느 버전이 필요한지, 마지막 업데이트 결과, 계정 정보, 확인이 필요한 이유, 그 항목이 무엇에 쓰이는지)과 그 항목의 단추가 모두 있습니다. 에이전트 CLI라면 {{ui:settings.cliAccounts.buttonChange}}, {{ui:settings.cliAccounts.buttonLogout}}, {{ui:settings.cliAccounts.refreshTooltip}} 등입니다. 다시 누르면 접힙니다. 어느 줄을 펼쳤는지는 앱을 끌 때까지 기억합니다.

목록 아래 {{ui:settings.components.groupAutoUpdate}} 상자에는 스위치 셋과 마지막 실행 시각이 있습니다. 스위치에 포인터를 올리면 설명이 보입니다.
- {{ui:settings.components.autoUpdateClis}}: 앱을 열 때, 그리고 6시간마다 에이전트 CLI 업데이트를 확인합니다.
- {{ui:settings.components.autoUpdatePlugins}}: Claude와 Codex 플러그인도 함께 업데이트합니다.
- {{ui:settings.components.autoLogin}}: 로그인이 끊기면 다시 로그인하고, 실패한 요청을 다시 보냅니다.

{{ui:settings.components.installButton}}를 누르면 그 항목과, 그 항목에 필요한데 빠져 있는 선행 도구만 설치합니다. 실행할 명령을 확인 창 하나에 모두 먼저 보여 줍니다.
- {{platform:mac}} 앱 안의 터미널 창에서 실행합니다. 열린 로컬 워크스페이스가 없으면 터미널(Terminal.app)에서 실행합니다.
- {{platform:windows}} 별도의 PowerShell 창에서 실행합니다.

설치됐는지는 끝난 뒤 다시 확인해서 정합니다. 처음 실행할 때 열리는 [{{ui:setup.title}}](getting-started.md#setup) 창은 빠진 것을 골라 {{ui:setup.button.install}}를 누르는 체크리스트입니다.

같은 탭의 맨 아래에는 {{ui:settings.toolkit.sectionTitle}} 상자가 있습니다. 자주 쓰는 도구를 직접 모아 두는 목록이고, 처음에는 비어 있습니다. {{ui:settings.toolkit.addButton}}로 도구를 넣고, 항목마다 {{ui:settings.toolkit.approveButton}}과 {{ui:settings.toolkit.removeButton}} 단추가 있습니다. 승인한 도구만 설치할 수 있고, 설치 전에 실행할 명령을 모두 보여 주고 {{ui:settings.toolkit.confirmTitle}}을 받습니다. 목록을 {{ui:settings.toolkit.exportButton}}하거나 {{ui:settings.toolkit.importButton}}할 수 있습니다.

![[settings-tools]]

### 로그인 {#cli-accounts}
에이전트 CLI마다 그 줄에서 로그인합니다. 로그인이 안 된 CLI는 줄에 {{ui:settings.cliAccounts.buttonLogin}} 단추가 바로 보이고, 그 밖의 계정 단추는 줄을 펼치면 보입니다.
1. {{ui:settings.cliAccounts.buttonLogin}}을 누릅니다. Claude는 {{ui:settings.cliAccounts.buttonLoginClaude}}과 {{ui:settings.cliAccounts.buttonLoginConsole}} 중에서 고릅니다.
2. CLI의 로그인이 터미널에서 시작됩니다. 브라우저에서 승인합니다.
   - {{platform:mac}} 앱 안에 터미널 창이 열립니다. 이 창은 로컬 워크스페이스에 열리므로 워크스페이스가 하나 이상 열려 있어야 합니다.
   - {{platform:windows}} 별도의 콘솔 창이 열립니다. 워크스페이스가 없어도 됩니다.
3. 로그인이 끝나면 줄의 상태가 바뀝니다. 바뀌지 않으면 그 줄을 펼쳐 {{ui:settings.cliAccounts.refreshTooltip}}을 누릅니다. 바뀐 계정은 다음 요청부터 쓰입니다.

계정을 바꾸려면 줄을 펼쳐 {{ui:settings.cliAccounts.buttonChange}}, 로그아웃하려면 {{ui:settings.cliAccounts.buttonLogout}}을 누릅니다. {{ui:settings.components.groupAutoUpdate}} 상자의 {{ui:settings.components.autoLogin}}을 켜면 로그인이 끊겼을 때 자동으로 다시 로그인하고 실패한 요청을 다시 보냅니다.

Claude 줄을 펼치면 {{ui:settings.cliAccounts.resetModelsButton}}, {{ui:settings.cliAccounts.bedrockButton}} 단추도 있습니다. {{ui:settings.cliAccounts.bedrockButton}}은 로그인과 같은 방식으로 터미널에서 Claude CLI의 AWS Bedrock 설정 마법사를 엽니다. 마친 뒤 상태를 새로고침하세요. {{platform:windows}} Windows에는 {{ui:settings.cliAccounts.resetBedrockButton}} 단추가 하나 더 있어, Claude 사용자 설정을 백업한 뒤 Bedrock 키와 Bedrock 모델 지정을 지웁니다.

### 업데이트 {#cli-update}
에이전트 CLI는 앱 업데이트와 따로 업데이트합니다.
- {{ui:settings.components.updateAllButton}} 단추: 설치된 에이전트 CLI를 지금 모두 업데이트합니다. 오래된 CLI 줄의 {{ui:setup.button.update}} 단추도 CLI 업데이트를 실행합니다.
- {{ui:settings.components.groupAutoUpdate}} 상자의 {{ui:settings.components.autoUpdateClis}}: 앱을 열 때, 그리고 6시간마다 확인합니다.
- {{ui:settings.components.autoUpdatePlugins}}: Claude와 Codex 플러그인도 함께 업데이트합니다.

CLI를 처음 설치한 방법(npm, Homebrew 등)을 그대로 씁니다. CLI마다 마지막 업데이트의 결과는 그 CLI 줄을 펼치면 보이고, 플러그인 업데이트의 결과는 {{ui:settings.components.groupPlugins}} 묶음 아래에 보입니다.

## 앱 정보 {#about}
{{ui:settings.nav.about}} 탭에서는 앱을 업데이트하고 데이터 저장 위치를 봅니다.
- {{ui:settings.appUpdate.sectionTitle}}: 지금 버전, 자동 업데이트 스위치, 상태와 단추, 릴리스 노트가 있습니다. [업데이트](getting-started.md#update)를 보세요. 접힌 {{ui:settings.advancedTitle}} 항목에는 {{ui:settings.appUpdate.manifestUrlAddressLabel}} 칸과 서명 안내가 있고, 주소를 넣어 두었으면 처음부터 펼쳐져 있습니다.
- {{ui:settings.appInfo.sectionTitle}}: {{ui:settings.appInfo.openHelpButton}} 단추와 {{ui:settings.appInfo.stateLocationLabel}}가 있습니다. {{ui:settings.appInfo.stateLocationLabel}}는 앱의 설정과 기록이 있는 폴더이고, Mac에서는 {{ui:menu.showInFinder}}, Windows에서는 {{ui:menu.showInExplorer}} 단추로 엽니다.

![[settings-about]]

### 진단 묶음 {#diagnostics}
버그를 알릴 때는 {{ui:diagnostics.settings.sectionTitle}} 상자의 {{ui:diagnostics.settings.export}} 단추로 진단 묶음을 만들어 첨부하세요. 저장할 곳을 고르면 zip 파일 하나가 만들어지고, 다 되면 파일 이름과 Finder(Windows는 탐색기)에서 보는 단추가 나옵니다.

- 들어가는 것: 앱 로그(앱 로그와 그 앞 파일, 백그라운드 작업, 시작 단계, 성능 기록의 각 마지막 2MB), 앱 버전, 설치된 CLI 버전, OS와 아키텍처, 설정 요약(켜진 기능과 워크스페이스·창 수), 그리고 무엇이 들어갔는지 적은 `manifest.json`.
- 지우는 것: API 키, 토큰, 비밀번호, 페어링 키, Authorization 헤더, 주소 속 아이디·비밀번호, 이메일, 경로 속 사용자 이름(홈 폴더는 `~`). `manifest.json`에 파일마다 몇 개를 지웠는지 나옵니다.
- 넣지 않는 것: 대화, 요청과 답, 파일 내용, 워크스페이스와 창의 이름, 경로, 환경 변수.
- 앱은 묶음을 어디로도 보내지 않습니다. 보내기 전에 zip을 열어 확인할 수 있습니다.
