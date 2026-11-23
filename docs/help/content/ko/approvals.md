---
title: 승인·질문·로그인 복구
order: 5
section: approvals
---
에이전트가 일하다가 사용자의 판단이 필요하면 실행 창에 카드가 뜹니다. 카드에 답하면 작업이 이어집니다. 작업 현황과 Mighty Pet, 휴대폰에서도 같은 요청을 볼 수 있습니다.

## 권한 승인 {#permission}
에이전트가 권한 밖의 일(명령 실행, 파일 수정 등)을 하려 하면 승인 카드가 뜹니다.

![[permission-card]]

1. 카드에서 무엇을 하려는지 읽습니다. 명령이나 파일 경로가 상자에 보이고, 정확한 입력은 원본 JSON에서 봅니다.
2. {{ui:permission.allowOnce}} 또는 {{ui:permission.deny}}를 누릅니다.

승인은 이 요청 한 번에만 적용됩니다. 앞으로도 계속 허용하는 규칙은 만들지 않습니다.

> [!note]
> 승인 카드는 Claude 창({{ui:permission.label.claude.default}} 등)과 Codex의 {{ui:permission.label.onRequest}} 권한, {{ui:permission.label.fullAccess}}을 뺀 opencode 창의 모든 권한에서 뜹니다. Hermes 창에서는 파일 수정과 위험한 명령에 뜹니다. 다른 권한에서는 CLI 설정에 따라 허용되거나 막힙니다. Antigravity는 일하는 방식이 달라 [따로 묻습니다](#antigravity-permission).

## Antigravity의 권한 카드 {#antigravity-permission}
Antigravity는 일하는 도중에 앱에 물어볼 수 없습니다. 물어봐야 하는 작업(명령 실행, 파일 쓰기 등)을 만나면 그 작업만 건너뛰고 턴을 끝낸 뒤, 거절한 작업을 모두 모아 카드로 묻습니다. 권한이 {{ui:permission.label.default}}일 때도 뜹니다. 휴대폰에도 같은 카드가 뜹니다.

카드에는 거절된 작업이 하나도 빠짐없이 나옵니다. 작업마다 종류({{ui:antigravity.permission.kind.command}}, {{ui:antigravity.permission.kind.url}}, {{ui:antigravity.permission.kind.file}} 등), 명령·주소·경로, 허용에 쓸 규칙이 보입니다.

1. 무엇을 거절했는지 읽습니다.
2. 단추를 고릅니다.
   - {{ui:permission.allowOnce}}: 거절된 작업만 허용하는 규칙을 Antigravity에 잠깐 더해 같은 대화를 이어 갑니다. 이어지는 실행이 끝나면(끝나든, 실패하든, 중지하든, 앱을 닫든) 앱이 더한 규칙만 지웁니다.
   - {{ui:antigravity.permission.allowAlways}}: 같은 규칙을 Antigravity 설정(`~/.gemini/antigravity-cli/settings.json`의 `permissions.allow`)에 그대로 남깁니다. 다음에는 묻지 않고 실행합니다. Windows에서는 `%USERPROFILE%\.gemini\antigravity-cli\settings.json`입니다.
   - {{ui:antigravity.permission.deny}}: 카드를 닫고 아무것도 이어서 실행하지 않습니다.

허용하면 같은 대화에서 이어서 진행합니다. 창이 다른 일로 바쁘면 이어 가는 실행은 대기열에 들어갑니다.

규칙은 앞부분이 맞으면 허용합니다. 명령 규칙은 **그 글자로 시작하는 모든 명령**을 허용하고(`command(git status)`는 `git status --short`도 허용합니다), 폴더를 가리키는 파일 규칙은 그 안의 모든 파일을 허용합니다. {{ui:permission.allowOnce}}의 규칙도 Antigravity의 전체 설정에 들어가므로, 규칙이 있는 동안에는 이 컴퓨터의 **모든 Antigravity 실행**(다른 창과 워크스페이스, 터미널의 `agy`)에 적용됩니다. 항상 허용한 명령 규칙은 그 뒤에 붙는 어떤 인수든 허용하므로, 인수로 다른 일을 시킬 수 있는 명령(`find … -exec`, `curl … -o` 등)은 되도록 {{ui:permission.allowOnce}}을 고르세요.

명령을 잇거나(`;`, `&&`, `||`), 파이프(`|`)·리디렉션(`>`, `<`)·명령 치환(백틱, `$(`)·백그라운드(`&`)를 쓰거나, 데이터를 지우거나 권한을 올리는 명령(`rm`, `sudo`, `git push --force`, `git reset --hard`, `git clean`, `kill` 등)은 {{ui:permission.allowOnce}}만 할 수 있습니다. 카드의 {{ui:antigravity.permission.allowAlways}}이 꺼지고 그 이유가 나옵니다. 휴대폰에서는 그 단추가 보이지 않습니다.

영문자·숫자·띄어쓰기와 `._/:=@+,-` 밖의 문자(따옴표, `$`, `~`, `%`, `\` 등)가 든 명령도 {{ui:permission.allowOnce}}만 할 수 있습니다. 항상 허용한 규칙은 같은 글자로 시작하는 모든 명령을 허용하기 때문입니다. 보이지 않거나 글자 방향을 바꾸는 문자, 보통 띄어쓰기가 아닌 공백이 든 명령에는 규칙을 만들지 않습니다. 파일 규칙은 이 컴퓨터 자체 디스크의 평범한 경로(Windows에서는 고정 드라이브의 `C:\…` 같은 드라이브 경로)에만 만들고, `\\server\share` 같은 네트워크 경로에는 만들지 않으며 앱이 그 경로를 찾아보지도 않습니다. 주소 규칙은 호스트가 평범할 때(`@`, `\`, `%`, 공백, ASCII가 아닌 글자가 없을 때)만 만들고, 카드는 브라우저가 읽는 대로 호스트를 보여 줍니다.

`*`나 괄호가 든 명령, 디스크 전체(`/`)나 홈(`~`)을 가리키는 규칙은 만들지 않습니다. Antigravity 자신의 설정 폴더(`~/.gemini`)나 앱 데이터 폴더 안, 또는 그 위의 폴더(홈 폴더 등)에 쓰거나 읽는 파일 규칙도 만들지 않습니다. 거기에 규칙을 두면 Antigravity가 자기 권한을 고칠 수 있기 때문입니다. `.gemini`를 언급하는 명령, 그리고 셸·인터프리터만 실행하거나 코드를 바로 넘기는 명령(`bash`, `python3 -c …`, `node -e …`, `cmd /c …`)과 다른 명령을 실행하는 래퍼(`npx`, `xargs`, `timeout`, `env` 등)도 {{ui:permission.allowOnce}}만 할 수 있습니다. `command(bash)`는 `bash -c`로 시작하는 무엇이든 허용하기 때문입니다. 카드에는 그 이유가 따로 보입니다. 그런 작업은 규칙으로 허용할 수 없는 작업으로 보입니다.

Claude와 다른 점이 있습니다. Claude는 실행 전에 멈추고 답을 기다리지만, Antigravity는 그 턴을 끝낸 뒤에 묻고 허용하면 이어서 진행합니다. 그래서 카드가 보일 때는 나머지 작업이 이미 끝나 있을 수 있습니다.

> [!note]
> 규칙으로 허용할 수 없는 작업이 있으면 카드에 그 작업이 보이고, 권한 모드를 바꾸라는 안내가 나옵니다. 그 작업만 있는 카드에는 {{ui:antigravity.permission.deny}}만 있습니다. Antigravity 설정 파일을 읽을 수 없거나 JSON이 아니면 앱은 파일을 고치지 않고 카드에 이유를 보여 줍니다. 앱은 `permissions.allow`의 규칙만 더하고 지우며 다른 설정은 그대로 둡니다. 잠깐 더한 규칙을 지우지 못하면 잠시 뒤 다시 지우고, 앱을 다시 열 때도 지웁니다.

[원격 워크스페이스](remote.md)의 Antigravity 창은 그 창을 실행하는 컴퓨터의 설정을 고칩니다. 카드 하나의 허용은 한 번만 쓸 수 있고, {{ui:antigravity.permission.allowAlways}}은 그 컴퓨터에서 {{ui:settings.remote.allowFullAccess}}이 켜져 있을 때만 받습니다.

## 질문에 답하기 {#questions}
Claude, pi, opencode, Hermes가 선택지를 주고 물으면 {{ui:phone.questionnaire.title}} 카드가 뜹니다. 보기 없이 답을 써 달라는 질문에는 입력 칸만 나옵니다.

![[question-card]]

1. 질문을 읽고 답을 고릅니다. 여러 개를 고를 수 있는 질문도 있습니다.
2. 알맞은 답이 없으면 {{ui:phone.questionnaire.custom}}에 직접 씁니다.
3. 질문이 여럿이면 {{ui:phone.questionnaire.next}} 단추로 넘어갑니다.
4. 모두 답한 뒤 {{ui:phone.questionnaire.submit}}를 누릅니다. 고르기만 해서는 보내지지 않습니다.

## 터미널이 입력을 기다릴 때 {#terminal-prompt}

에이전트가 터미널에서 실행한 명령(또는 답의 셸 블록에서 {{ui:run.shellBlock.run}}을 누른 명령)이 입력을 기다리면, 그 에이전트 창의 입력창 위에 **{{ui:terminalPrompt.title}}** 카드가 뜹니다. 터미널 창으로 가지 않고 카드에서 답하면, 그 답이 터미널에 그대로 입력됩니다. 휴대폰의 같은 창에도 같은 카드가 뜹니다.

- **예/아니오 질문**(`(Y/n)` 같은 것): {{ui:terminalPrompt.yes}} 또는 {{ui:terminalPrompt.no}}를 누릅니다. Enter만 눌렀을 때의 답에는 ({{ui:terminalPrompt.default}})이 붙습니다.
- **비밀번호**: 가려지는 칸에 입력하고 {{ui:terminalPrompt.send}}를 누릅니다. 입력한 내용은 화면과 대화 기록 어디에도 남지 않습니다.
- **글 입력**: 칸에 입력하고 {{ui:terminalPrompt.send}}를 누르거나, 기본값을 받아들이려면 {{ui:terminalPrompt.enterOnly}}를 누릅니다.
- **목록에서 고르기**: {{ui:terminalPrompt.up}}·{{ui:terminalPrompt.down}}로 옮기고 {{ui:terminalPrompt.select}}을 누릅니다.

카드는 명령의 출력이 잠시 멈춘 뒤에 뜨고, 답하거나 명령이 다시 출력하거나 끝나면 사라집니다. 에이전트가 대신 답하는 일은 없습니다. 터미널 창에서 직접 입력해도 됩니다({{ui:terminalPrompt.showTerminal}}).

## 로그인이 끊겼을 때 {#login-recovery}
CLI 로그인이 만료되어 요청이 실패하면 입력창 위에 로그인 복구 카드가 뜹니다.
1. {{ui:loginRecovery.loginButton}}을 누릅니다.
2. 브라우저가 열리면 로그인을 승인합니다. 브라우저에 코드가 보이면 카드에 붙여 넣고 {{ui:loginRecovery.codeSubmit}}을 누릅니다.
3. 로그인이 끝나면 실패한 요청을 저절로 다시 보냅니다.

이미 다른 곳에서 로그인했다면 {{ui:loginRecovery.resendButton}}를 누르세요. 브라우저 로그인이 어려우면 {{ui:loginRecovery.terminalButton}}을 씁니다.

{{platform:mac}} Antigravity, pi, opencode, Hermes는 터미널에서 로그인합니다. 실패한 창 옆에 로그인 터미널이 열리고, Antigravity(agy)는 열리는 브라우저에서 Google 계정으로 로그인합니다. pi는 터미널에서 `/login`을 입력해 구독을 고르거나 API 키를 입력합니다. opencode는 `opencode auth login`이 실행되어 제공자를 고른 뒤 로그인하거나 API 키를 입력합니다. Hermes는 `hermes model`이 실행되어 모델 제공자를 고르고 로그인한 뒤 기본 모델을 고릅니다. 로그인이 확인되면 실패한 요청을 저절로 다시 보냅니다. 이 터미널은 로그인 뒤에도 열려 있습니다.

{{platform:windows}} Windows에서도 Antigravity, pi, opencode, Hermes는 터미널에서 로그인합니다. 로그인 터미널이 따로 창으로(Windows Terminal이 있으면 새 탭으로) 실패한 창의 폴더에서 열리고, 터미널에서 하는 일은 Mac과 같습니다. Antigravity(agy)는 열리는 브라우저에서 Google 계정으로 로그인합니다. pi는 `/login`을 입력해 구독을 고르거나 API 키를 입력합니다. opencode는 `opencode auth login`이, Hermes는 `hermes model`이 실행됩니다. 로그인이 확인되면 실패한 요청을 저절로 다시 보냅니다. 앱은 이 터미널을 닫지 않습니다.

> [!tip]
> {{ui:settings.nav.tools}} 탭의 {{ui:settings.cliAccounts.autoLoginToggle}}을 켜 두면 로그인이 끊긴 순간 바로 다시 로그인을 시작합니다.

## 백그라운드 작업 {#background}
Claude는 오래 걸리는 명령이나 하위 에이전트를 백그라운드에서 돌리기도 합니다. 이때는 Claude의 답이 끝나도 일이 남아 있습니다.
- 백그라운드 작업은 다이어그램 위 진행 상황 패널의 {{ui:graph.progress.background}} 묶음에 보입니다(요청을 시작할 때부터). 묶음 제목 옆의 "1/3"은 아직 도는 작업 수와 이번 요청이 띄운 전체 작업 수입니다. 행마다 상태 표시, 종류(명령 또는 하위 에이전트), 명령이나 프롬프트의 첫 줄, 걸린 시간이 나오고, 행에 마우스를 올리면 전체 내용이 보입니다.
- 작업이 끝나면 Claude에게 알림이 가고, Claude가 그 결과를 받아 이어서 답합니다. 끝난 작업은 끝난 표시로 바뀌어 다음 요청을 보낼 때까지 묶음에 남습니다.
- 도는 동안 백그라운드 작업은 마이티 보기의 요청 안에 그려지지 않고 진행 상황 패널에만 있습니다. 휴대폰은 마이티 보기 위에 {{ui:plan.background.title}} 줄을 접어 두고 끝난 수와 도는 수를 "끝남 1개 · 작업 중 2개"처럼 보이며, 펼치면 도는 작업의 행이 보입니다.
- 작업이 끝나 알림이 가면 요청에는 아무것도 붙지 않고, Claude의 답 하나마다 새 다이어그램이 생깁니다. 제목은 끝난 일의 이름을 따서 "{{ui:graph.background.noticeRun}} · 코드 검토 에이전트"처럼 붙고, 여러 개가 함께 끝나면 "… 외 N개"로 줄여 보입니다. 끝난 작업의 {{ui:graph.block.task}} 블록(명령, 끝난 상태, 요약)과 Claude의 답이 그 안에 보입니다. 이 다이어그램은 요청이 아니라서 요청 번호를 세지 않습니다. 흐름에서는 맨 아래 새 카드로, {{ui:graph.view.timeline}}에서는 새 행으로, {{ui:graph.view.structure}}에서는 맨 끝의 새 카드(보드)로 붙습니다. 이름이 없는 알림은 Claude가 답하지 않는 한 다이어그램을 만들지 않습니다. 모든 알림에 답할 때까지 원래 요청의 결과 카드는 {{ui:graph.block.awaitingNotice}} 블록으로 남습니다([결과 카드](agent-pane.md)).
- 백그라운드로 시작한 하위 에이전트도 같습니다. 끝나면 마이티 보기의 블록이 {{ui:graph.state.completed}}, {{ui:graph.state.error}}, {{ui:graph.state.stopped}} 상태로 바뀌고(앱이 다시 시작되어 끝까지 못 본 경우도 {{ui:graph.state.stopped}}), 에이전트가 낸 최종 보고가 블록의 결과가 됩니다. 에이전트가 도는 중에 보낸 메시지는 블록의 기록에 더해지고 블록을 끝내지 않으며, 그 메시지에 Claude가 한 답은 따로 {{ui:graph.background.messageRun}} 다이어그램에 보입니다. 같은 에이전트가 잇달아 보낸 메시지와 그 답은 같은 다이어그램에 이어 붙습니다. 끝난 알림에 Claude가 한 답은 {{ui:graph.background.noticeRun}} 다이어그램에 보입니다.
- 이때 보낸 요청은 기본으로 대기열에 들어가 백그라운드 작업이 끝난 뒤 시작합니다. 지금 시작하려면 중지 단추를 누릅니다. 대기열은 그대로 남고 첫 요청이 이어서 시작합니다. [마이티 오케스트레이션](delegation.md#worktree)이 켜진 git 저장소의 창에서는 {{kbd:⌘Enter}}(Windows는 {{kbd:Ctrl+Enter}})로 새 worktree에서 바로 실행할 수 있고, 입력창 아래 {{ui:composer.busySend.title}}에서 에이전트를 골라 두면 {{kbd:Enter}}도 그 에이전트의 새 worktree에서 바로 실행합니다([실행 중 보내기](agent-pane.md#queue)).
- 진행 상황 패널을 접으면 백그라운드 묶음도 함께 접힙니다.

![[background-work]]

## 샌드박스 {#sandbox}
{{platform:mac}} 워크스페이스마다 에이전트를 macOS 샌드박스에서 실행할 수 있습니다. 기본은 꺼져 있습니다. 사이드바에서 워크스페이스를 오른쪽 클릭해 {{ui:sandbox.menu.settings}} 항목을 고르고, {{ui:sandbox.settings.enabled}} 항목을 켠 뒤 {{ui:sandbox.settings.save}} 단추를 누릅니다. 다음 실행부터 적용되고, 창 머리에 {{ui:sandbox.badge}} 표시가 붙습니다. 표시를 누르면 설정이 열립니다.

### 막는 것 {#sandbox-blocked}
- **쓰기**: 워크스페이스(또는 창의 worktree) 폴더, 에이전트 자신의 설정 폴더, 임시 폴더, 패키지 캐시에만 쓸 수 있습니다.
- **git**: 커밋, 브랜치, 병합, 리베이스는 됩니다. `.git/config`, `.git/hooks`, `.git/info`는 고칠 수 없습니다.
- **지침 파일**: {{ui:sandbox.settings.instructions}} 항목이 켜져 있으면(기본) `.mighty.yaml`, `AGENTS.md`, `CLAUDE.md`와 에이전트의 프로젝트 설정을 고칠 수 없습니다. {{ui:sandbox.settings.readOnly}} 칸에 경로를 더 넣을 수 있습니다.
- **읽기**: `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.config/gh`, 브라우저 쿠키, 앱의 데이터 폴더, 다른 에이전트의 로그인 파일은 읽을 수 없습니다. 에이전트는 자기 로그인만 읽습니다.
- **접속**: {{ui:sandbox.settings.network}} 항목이 켜져 있으면(기본) 에이전트는 앱의 프록시를 거쳐서만 밖으로 나갑니다. 에이전트의 API 주소와 {{ui:sandbox.settings.hosts}} 칸에 넣은 곳만 바로 통과합니다.

### 접속 허용하기 {#sandbox-allow}
목록에 없는 곳에 접속하려 하면 입력창 위에 **{{ui:sandbox.network.cardTitle}}** 카드가 뜹니다.
- {{ui:sandbox.network.allowOnce}}: 이 실행이 끝날 때까지 허용합니다.
- {{ui:sandbox.network.allowAlways}}: 워크스페이스의 허용 목록에 넣고 허용합니다.
- {{ui:sandbox.network.deny}}: 막습니다. 5분 동안 답이 없어도 막습니다.

막힌 동작은 진행 상황에 한 줄로 남고, 휴대폰의 같은 창과 설정의 {{ui:sandbox.settings.blocked}} 목록에도 이유와 함께 보입니다. 휴대폰에서도 카드에 답할 수 있습니다({{ui:sandbox.network.allowAlways}} 단추는 Mac에서만).

### 심사 에이전트 {#sandbox-reviewer}
{{ui:sandbox.settings.reviewer}} 항목을 켜면, 승인 카드나 접속 카드가 뜰 때 읽기 전용 에이전트(기본은 Claude의 haiku)가 먼저 봅니다.
- 안전한 동작(파일 읽기, 워크스페이스 안 파일 수정, 빌드·테스트 명령, 패키지 저장소 접속)만 스스로 허용합니다.
- 워크스페이스 밖에 쓰기나 위험한 명령은 허용하지 못합니다. 거부하거나 사용자에게 넘깁니다.
- 20초 안에 답이 없거나 오류가 나면 허용하지 않고 사용자에게 묻습니다.
- 판단할 때마다 창에 한 줄이 남습니다. 그동안에도 카드는 떠 있어 먼저 답할 수 있고, 사용자의 답이 우선합니다.
- 허가는 사용자의 말뿐입니다. 심사 에이전트의 글은 허가로 치지 않습니다. 심사 실행에도 비용이 들고 사용량에 잡힙니다.

### 한계 {#sandbox-limits}
- 셸 창의 명령, SSH 워크스페이스(에이전트가 원격 머신에서 실행), 원격 워크스페이스에는 적용되지 않습니다.
- {{platform:windows}} {{ui:sandbox.settings.windows}}.
- 샌드박스에서 도는 실행은 앱을 닫으면 함께 끝납니다. 백그라운드에서 계속 실행되지 않습니다.
- macOS의 `sandbox-exec`는 Apple이 권장하지 않게 된 도구입니다. 없으면 샌드박스를 켠 워크스페이스의 실행을 거절합니다. 실행하려면 샌드박스를 끄세요.
- 키체인에 로그인을 두는 에이전트(Claude, Codex, Antigravity)는 로그인 키체인을 읽을 수 있습니다. 다른 앱의 항목은 macOS가 따로 묻습니다.
