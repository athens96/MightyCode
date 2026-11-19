---
title: 설정
order: 8
section: settings
---
{{ui:menu.settings}}({{kbd:⌘,}})이나 사이드바 아래의 설정 단추로 엽니다. 왼쪽에서 탭을 고르고, {{ui:settings.closeButton}} 단추나 {{kbd:Esc}}로 닫습니다. 마지막에 본 탭을 기억합니다. 탭은 {{ui:settings.nav.general}}, {{ui:settings.nav.models}}, {{ui:settings.nav.cli}}, {{ui:settings.nav.styles}}, {{ui:settings.remote.navTitle}}, {{ui:settings.nav.companion}}, {{ui:settings.nav.tools}}, {{ui:settings.nav.about}} 순서입니다.

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
- {{ui:settings.editor.label}}: {{ui:editor.open}}가 시작할 편집기입니다. 자동(찾은 것 중 첫 번째), 이 컴퓨터에서 찾은 편집기, {{ui:settings.editor.system}} 중에서 고릅니다. 설정을 열 때마다 다시 찾습니다([편집기에서 열기](terminal-files-browser.md#open-in-editor)).
- 아래에 키 안내가 있습니다. {{kbd:Enter}}로 보내고 {{kbd:Shift+Enter}}로 줄을 바꿉니다. 탭을 끌어 다른 탭과 합치거나 가장자리로 옮겨 나눕니다. 명령 실행 창은 Mac에서는 로그인 셸, Windows에서는 PowerShell(없으면 cmd)을 여는 대화형 터미널입니다.

## 모델 {#models}
{{ui:settings.nav.models}} 탭의 {{ui:settings.phaseModels.sectionTitle}}에서 작업 단계별로 쓸 모델을 이 컴퓨터 전체에 정합니다. 단계는 {{ui:settings.phaseModels.phase.planning}}, {{ui:settings.phaseModels.phase.execution}}, {{ui:settings.phaseModels.phase.review}}, {{ui:settings.phaseModels.phase.subagents}}입니다.
- {{ui:settings.phaseModels.defaultOption}}으로 두면 CLI가 정합니다.
- {{ui:settings.phaseModels.paneModel}}은 실행 창의 모델 칩에서 고른 모델을 씁니다.
- 목록에 없는 모델 이름은 {{ui:settings.phaseModels.registeredTitle}}에 {{ui:settings.phaseModels.addButton}}합니다. 그 모델이 사고 강도를 지원하면 {{ui:settings.phaseModels.supportsEffortLabel}}을 켭니다.

![[settings-models]]

## CLI {#cli}
{{ui:settings.nav.cli}} 탭에서 CLI의 로그인과 업데이트를 관리합니다. CLI마다 설치 여부, 버전, 설치·업데이트 단추는 {{ui:settings.nav.tools}} 탭의 [{{ui:settings.components.sectionTitle}}](#components) 목록에 있습니다.

### 로그인 {#cli-accounts}
{{ui:settings.cliAccounts.sectionTitle}}에서 CLI마다 로그인합니다.
1. {{ui:settings.cliAccounts.buttonLogin}}을 누릅니다. Claude는 {{ui:settings.cliAccounts.buttonLoginClaude}}과 {{ui:settings.cliAccounts.buttonLoginConsole}} 중에서 고릅니다.
2. CLI의 로그인이 터미널에서 시작됩니다. 브라우저에서 승인합니다.
   - {{platform:mac}} 앱 안에 터미널 창이 열립니다. 이 창은 로컬 워크스페이스에 열리므로 워크스페이스가 하나 이상 열려 있어야 합니다.
   - {{platform:windows}} 별도의 콘솔 창이 열립니다. 워크스페이스가 없어도 됩니다.
3. 로그인이 끝나면 상태가 바뀝니다. 바뀌지 않으면 그 CLI 줄의 {{ui:settings.cliAccounts.refreshTooltip}} 단추를 누릅니다. 바뀐 계정은 다음 요청부터 쓰입니다.

계정을 바꾸려면 {{ui:settings.cliAccounts.buttonChange}}, 로그아웃하려면 {{ui:settings.cliAccounts.buttonLogout}}을 누릅니다. {{ui:settings.cliAccounts.autoLoginToggle}}을 켜면 로그인이 끊겼을 때 자동으로 다시 로그인하고 실패한 요청을 다시 보냅니다.

Claude 줄에는 {{ui:settings.cliAccounts.resetModelsButton}}, {{ui:settings.cliAccounts.bedrockButton}} 단추도 있습니다. {{ui:settings.cliAccounts.bedrockButton}}은 로그인과 같은 방식으로 터미널에서 Claude CLI의 AWS Bedrock 설정 마법사를 엽니다. 마친 뒤 상태를 새로고침하세요. {{platform:windows}} Windows에는 {{ui:settings.cliAccounts.resetBedrockButton}} 단추가 하나 더 있어, Claude 사용자 설정을 백업한 뒤 Bedrock 키와 Bedrock 모델 지정을 지웁니다.

### CLI 업데이트 {#cli-update}
{{ui:settings.cliUpdate.sectionTitle}}에서 설치된 CLI를 최신으로 바꿉니다.
- {{ui:settings.cliUpdate.updateButton}} 단추: 지금 업데이트합니다.
- {{ui:settings.cliUpdate.autoUpdateToggle}}: 앱을 열 때, 그리고 6시간마다 확인합니다.
- {{ui:settings.cliUpdate.autoUpdatePluginsToggle}}: Claude와 Codex 플러그인도 함께 업데이트합니다.

CLI를 처음 설치한 방법(npm, Homebrew 등)을 그대로 씁니다.

![[settings-cli]]

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

## 펫과 알림 {#companion}
{{ui:settings.nav.companion}} 탭에서 데스크톱 펫과 완료 알림을 정합니다.
- {{ui:companion.settings.enabled}}: 화면 위를 다니는 너구리 펫을 켭니다. 펫을 끌어 옮기고, 누르면 말풍선이 열리고 닫힙니다.
- {{ui:companion.settings.task}}: 말풍선에 요청과 지금 하는 일, 걸린 시간을 보여 줍니다. 말풍선에서 승인({{ui:permission.allowOnce}} / {{ui:permission.deny}})에 답할 수도 있습니다. 승인·질문·계획 말풍선의 기호 단추는 요청 전체를 보거나 답을 직접 쓸 수 있게 그 실행 창을 엽니다.
- 말풍선 위에는 그 창의 최근 요청 {{ui:graph.progress.title}} 카드가 붙습니다. 실행 창의 진행 패널과 같은 내용으로, 요청 번호와 끝난 단계 수, 상태, 지금 하는 단계 앞뒤의 단계 몇 개와 백그라운드 작업이 각자 걸린 시간과 함께 보입니다. 카드 머리를 누르면 머리만 남게 접히고 다시 누르면 펼쳐지며, 접은 상태는 다음에 앱을 열어도 그대로입니다. 단계 줄을 누르면 그 실행 창이 열립니다. 단계도 백그라운드 작업도 없는 요청이거나 말풍선이 승인·질문·계획을 보이는 동안, 말풍선이 닫혀 있을 때는 카드가 없습니다.
- {{ui:companion.settings.pet}}: 펫을 고릅니다. {{ui:companion.settings.import}}로 다른 펫을 더하고, {{ui:companion.settings.reload}} 단추로 펫 목록을 다시 읽고, {{ui:companion.settings.resetPosition}} 단추로 펫의 위치와 말풍선 크기를 처음대로 돌립니다.
- {{ui:companion.settings.motion}}: 움직임을 줄입니다.
- {{ui:companion.settings.macNotifications}}(Windows는 {{ui:windows.notifications.toggleLabel}}): 요청이 성공적으로 끝나면 Mac 알림을 보냅니다. 알림을 누르면 그 실행 창이 열립니다. 알림에는 요청이나 결과 내용이 들어가지 않습니다.
- 그 아래에 운영체제의 알림 허용 상태가 보이고, {{ui:windows.notifications.settingsButton}} 단추로 시스템의 알림 설정을 엽니다.

![[settings-companion]]

![[companion-pet]]

## 구성 요소 {#components}
{{ui:settings.nav.tools}} 탭은 앱에 필요한 것이 준비됐는지 보여 줍니다.
- {{ui:settings.components.sectionTitle}}: [{{ui:setup.title}}](getting-started.md#setup) 목록이 여기 있습니다. 에이전트 CLI, 기본 도구, 스타일 도구, Tailscale, gh·glab마다 {{ui:setup.status.installed}}, {{ui:setup.status.missing}}, {{ui:setup.status.outdated}}, {{ui:setup.status.attention}}, {{ui:setup.status.installing}}, {{ui:setup.status.failed}} 중 하나가 보이고, 빠진 항목을 골라 {{ui:setup.button.install}}를 누르면 명령을 모두 확인한 뒤 설치합니다. Mac에서는 앱 안의 터미널 창에서, Windows에서는 앱이 따로 여는 PowerShell 창에서 실행됩니다. CLI마다 버전과 로그인 상태도 여기서 보고, 너무 오래된 Claude Code에는 {{ui:setup.button.update}} 단추가 나옵니다. {{ui:setup.button.recheck}} 단추와 목록이 열려 있는 동안의 자동 확인은 CLI를 매번 다시 찾습니다.
- {{ui:settings.toolkit.sectionTitle}}: 자주 쓰는 도구 목록입니다. 설치 전에 실행할 명령을 모두 보여 주고 {{ui:settings.toolkit.confirmTitle}}을 받습니다. 목록을 {{ui:settings.toolkit.exportButton}}하거나 {{ui:settings.toolkit.importButton}}할 수 있습니다.

![[settings-tools]]

## 앱 정보 {#about}
{{ui:settings.nav.about}} 탭에서는 앱을 업데이트하고 데이터 저장 위치를 봅니다.
- {{ui:settings.appUpdate.sectionTitle}}: 지금 버전, 자동 업데이트 스위치, 상태와 단추, 릴리스 노트가 있습니다. [업데이트](getting-started.md#update)를 보세요. 접힌 {{ui:settings.advancedTitle}} 항목에는 {{ui:settings.appUpdate.manifestUrlAddressLabel}} 칸과 서명 안내가 있고, 주소를 넣어 두었으면 처음부터 펼쳐져 있습니다.
- {{ui:settings.appInfo.sectionTitle}}: {{ui:settings.appInfo.openHelpButton}} 단추와 {{ui:settings.appInfo.stateLocationLabel}}가 있습니다. {{ui:settings.appInfo.stateLocationLabel}}는 앱의 설정과 기록이 있는 폴더이고, Mac에서는 {{ui:menu.showInFinder}}, Windows에서는 {{ui:menu.showInExplorer}} 단추로 엽니다.

![[settings-about]]
