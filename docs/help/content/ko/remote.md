---
title: 원격 연결
order: 10
section: remote
---
Mac이나 Windows 컴퓨터 두 대의 Mighty Code를 이으면, 한쪽에서 다른 쪽 컴퓨터의 워크스페이스를 열고 그 컴퓨터에서 Claude·Codex·Antigravity와 명령을 실행할 수 있습니다. 파일과 CLI 로그인은 실행하는 컴퓨터에 그대로 있고, 프로젝트 파일을 복사하거나 동기화하지 않습니다.

잇는 방법은 두 가지입니다.
- {{ui:settings.remote.viaPairing}}: 휴대폰처럼 한 번 페어링하고 Mighty 릴레이를 거쳐 연결합니다. Tailscale이 필요 없습니다. [페어링으로 연결](#pair)을 보세요.
- {{ui:settings.remote.viaTailscale}}: 같은 [Tailscale](https://tailscale.com/kb/1017/install) 네트워크(tailnet) 안에서 주소와 연결 키로 바로 연결합니다. 이 컴퓨터의 창을 그 컴퓨터에서 실행하는 원격 창과 큰 첨부는 Tailscale로만 됩니다.

모두 {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} 탭에서 합니다. 탭 맨 위에 안내 한 줄이 있고, 그 아래에 {{ui:settings.remote.shareTitle}} | {{ui:settings.remote.connectTitle}} | {{ui:settings.remote.phoneTab}} 탭이 있습니다. 안쪽 탭은 앱이 실행되는 동안만 기억합니다. 휴대폰 연결은 [휴대폰 원격](phone.md)을 보세요.

## 준비 {#prepare}
- 두 컴퓨터에 Mighty Code를 설치합니다.
- 일을 맡길 컴퓨터(호스트)에 쓸 CLI를 설치하고 로그인합니다. 원격 실행은 호스트의 CLI와 계정을 씁니다.
- Tailscale로 연결하려면 두 컴퓨터에 Tailscale을 설치하고 같은 tailnet에 로그인합니다. 공유기 포트 포워딩, Tailscale Serve·Funnel은 필요하지 않습니다. tailnet 정책이 두 기기 사이의 연결을 막지 않는지만 확인하세요.

## 이 컴퓨터 공유 {#share}
호스트 컴퓨터에서:
1. {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → {{ui:settings.remote.shareTitle}} 탭을 엽니다.
2. {{ui:settings.remote.pickWorkspaces}}에서 공유할 로컬 워크스페이스에 체크합니다.
3. 아래 {{ui:settings.remote.methodsTitle}}에서 쓸 방식을 준비합니다(아래 설명).
4. 위쪽 상태 줄 옆의 {{ui:settings.remote.startSharing}} 단추를 누릅니다. 공유하는 동안에는 그 자리에 {{ui:settings.remote.sharedListTitle}} 목록이 보입니다.

{{ui:settings.remote.methodsTitle}}에는 방식마다 한 줄이 있습니다.
- {{ui:settings.remote.viaPairing}}: {{ui:settings.remote.pairAccept}} 스위치와 릴레이 상태, {{ui:settings.remote.pairCopyLink}} 단추가 있습니다. 링크에는 휴대폰 QR과 같은 키가 들어 있어 비밀로 복사되고, 1분 뒤 클립보드에서 지워집니다. 릴레이를 쓸 수 없는 실행에서는 스위치가 흐리게 보이고 그 이유가 나옵니다. [페어링으로 연결](#pair)을 보세요.
- {{ui:settings.remote.viaTailscale}}: 이 컴퓨터에 Tailscale 주소가 없으면 그렇다는 안내와 {{ui:settings.remote.tailscaleInstall}} 단추가 보이고, 누르면 {{ui:settings.nav.tools}} 탭이 열립니다. 공유하기 전에는 {{ui:settings.remote.portLabel}} 칸이 있습니다. 보통 그대로 둡니다(43137). 공유하는 동안에는 이 컴퓨터의 Tailscale 주소와 {{ui:settings.remote.keyLabel}}가 보입니다. 키는 보기 단추로 확인하고, {{ui:settings.remote.copyButton}} 단추로 복사해 연결할 쪽에 알려 줍니다.

Tailscale이 없어도 {{ui:settings.remote.pairAccept}}가 켜져 있으면 공유를 시작할 수 있습니다. 이때는 페어링한 컴퓨터만 릴레이로 연결합니다.

공유 중에는 {{ui:settings.remote.allowFullAccess}} 스위치가 보입니다. 꺼져 있으면(기본) 연결한 컴퓨터는 원격 명령 창을 쓸 수 없고, 에이전트를 전체 권한으로 실행할 수 없습니다. 그런 요청은 실행 전에 거절되고 그 창에 이유가 보입니다. 공유를 시작할 때마다 다시 꺼집니다.

이 스위치가 꺼져 있으면 연결한 컴퓨터는 공유하는 컴퓨터가 공유 폴더 안에 가둬 두지 못하는 에이전트 창도 쓸 수 없습니다(권한 모드와 상관없이). Mac에서 공유할 때는 Antigravity·pi·opencode·Hermes가 그렇고(이들에게는 가둬 두는 샌드박스가 없습니다), Windows에서 공유할 때는 Codex까지 Claude가 아닌 모든 에이전트가 그렇습니다(Windows는 Codex를 Mac에서처럼 샌드박스 안에 가두지 못합니다). Mac의 Codex는 자체 샌드박스 안에서 실행되고, Claude 창은 어느 쪽이든 꺼져 있어도 평소처럼 권한을 물어 가며 실행됩니다. 이런 창에는 보내기뿐 아니라 권한 허용, 질문 답, 계획 승인도 스위치가 켜져 있어야 합니다.

Tailscale 공유는 이 컴퓨터의 Tailscale 주소에서만 연결을 받습니다. 앱을 다시 열면 공유는 항상 꺼진 상태로 시작합니다. {{ui:settings.remote.stopSharing}} 단추를 누르면 원격 실행이 멈추고 연결 키는 폐기됩니다. 다시 공유하면 새 키를 알려 줘야 합니다.

> [!warning]
> 연결 키나 페어링 링크를 가진 기기는 공유한 워크스페이스에서 작업과 명령을 실행할 수 있습니다. 권한 모드에 따라 호스트 사용자의 권한으로 폴더 밖에도 접근할 수 있습니다. 키와 링크는 신뢰하는 기기에만 알려 주세요.

## 다른 컴퓨터 연결 {#connect}
일을 시킬 컴퓨터에서:
1. {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → {{ui:settings.remote.connectTitle}} 탭을 엽니다. {{ui:workspace.addPane}} 메뉴의 {{ui:remote.menu.connect}} 항목도 같은 곳을 엽니다.
2. 위의 두 갈래 스위치에서 {{ui:settings.remote.pairLinkField}} 또는 Tailscale 주소를 고릅니다.
   - {{ui:settings.remote.pairLinkField}}: 호스트에서 받은 링크를 붙여 넣고, 원하면 {{ui:settings.remote.nameField}}을 정합니다.
   - Tailscale 주소: {{ui:settings.remote.addressField}}(Tailscale IP 또는 MagicDNS 이름)와 {{ui:settings.remote.keyField}}를 넣고, 원하면 이름을 정합니다. 포트를 바꾸지 않았다면 주소만 넣으면 됩니다.
3. {{ui:settings.remote.connectButton}} 단추를 누릅니다.

아래 {{ui:settings.remote.connectionsTitle}} 목록에는 연결마다 한 줄이 있습니다. 줄에는 컴퓨터 이름, 방식 칩(페어링·Tailscale), 상태({{ui:settings.remote.status.connected}}, {{ui:settings.remote.status.disconnected}}, {{ui:settings.remote.status.saved}})와 {{ui:settings.remote.refreshButton}}, {{ui:settings.remote.disconnectButton}} 단추, 지우기 아이콘이 있습니다. 연결한 컴퓨터 아래에는 공유된 워크스페이스가 보입니다. 연결하면 그 컴퓨터의 공유 워크스페이스가 거기 열린 창과 함께 사이드바에 저절로 나타나고, {{ui:remote.mirror.view}} 단추는 그 워크스페이스로 바로 갑니다([그 컴퓨터의 창 그대로](#mirror)). 탭 맨 아래에 접힌 {{ui:settings.remote.scopeTitle}} 항목이 있습니다.

원격 워크스페이스는 사이드바에서 컴퓨터 아이콘과 호스트 이름으로 구분되고, 워크스페이스 머리글과 아래 상태 막대에 연결 상태가 보입니다. 앱을 다시 열면 연결은 저장된 상태로 남아 있다가, 원격 창에서 처음 보내거나 {{ui:settings.remote.refreshButton}} 단추를 누를 때 다시 연결합니다.

{{platform:mac}} 연결 키는 macOS Keychain에만 저장하고 워크스페이스 기록에는 넣지 않습니다. 다시 연결할 때 macOS가 Keychain 접근을 물으면 "항상 허용"을 고르세요. 앱이 같은 서명으로 배포되므로 이 선택은 업데이트 뒤에도 유지됩니다.

{{platform:windows}} Windows에서는 연결 키를 워크스페이스 기록에 넣지 않고, 이 Windows 사용자만 풀 수 있게 암호화해 앱의 데이터 폴더에 저장합니다. Tailscale로 공유하려면 방화벽 규칙이 하나 필요합니다([Windows 방화벽](#firewall)).

## 페어링으로 연결 (Tailscale 없이) {#pair}
두 컴퓨터에 Tailscale이 없어도, 휴대폰처럼 한 번 페어링하면 [휴대폰 원격](phone.md)과 같은 Mighty 릴레이를 거쳐 연결할 수 있습니다. 릴레이는 앱에 들어 있어 따로 준비할 필요가 없습니다. 연결은 종단 간 암호화되어 릴레이는 내용을 볼 수 없고, 연결한 컴퓨터는 [그 컴퓨터의 창 그대로](#mirror)를 씁니다.

호스트 컴퓨터에서:
1. {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → {{ui:settings.remote.shareTitle}} 탭의 {{ui:settings.remote.methodsTitle}}에서 {{ui:settings.remote.viaPairing}} 줄의 {{ui:settings.remote.pairAccept}}를 켭니다.
2. 워크스페이스를 고르고 {{ui:settings.remote.startSharing}} 단추를 누릅니다. 공유하는 동안 이 컴퓨터가 릴레이에 연결하고, 그 상태가 같은 줄에 보입니다.
3. {{ui:settings.remote.pairCopyLink}} 단추로 링크를 복사해 연결할 컴퓨터에 전합니다.

연결할 컴퓨터에서:
1. {{ui:settings.remote.navTitle}} → {{ui:settings.remote.connectTitle}} 탭의 스위치에서 {{ui:settings.remote.pairLinkField}}를 고르고, 링크를 붙여 넣고, 원하면 이름을 정한 뒤 {{ui:settings.remote.connectButton}} 단추를 누릅니다.
2. 연결되면 그 컴퓨터가 {{ui:settings.remote.connectionsTitle}} 목록에 페어링 칩과 함께, 사이드바에 "원격 · 이름"으로 나타납니다. 연결과 연결 끊기도 Tailscale 연결과 같습니다.

- 링크에는 휴대폰 QR과 같은 페어링 키가 들어 있으니 믿는 사람에게만 전하세요. 처음 연결할 때 호스트가 이 컴퓨터만의 기기 토큰을 주고, 그 뒤에는 그 토큰으로 연결합니다. 토큰은 연결 키처럼 macOS Keychain(Windows에서는 이 사용자만 풀 수 있게 암호화한 파일)에 두고 워크스페이스 기록에는 넣지 않습니다.
- 페어링한 컴퓨터는 호스트의 {{ui:settings.remote.phoneTab}} 탭 {{ui:settings.mobileRemote.connectedDevicesTitle}} 목록에 컴퓨터 아이콘과 함께 보입니다(휴대폰 연결 스위치가 꺼져 있어도 보입니다). 그 줄의 {{ui:settings.mobileRemote.revokeRowButton}} 단추는 그 컴퓨터만 해제하고, 다른 휴대폰과 컴퓨터는 그대로 연결됩니다. 이때 페어링 키가 새로 바뀌므로 그 뒤에 연결하는 기기는 새 링크를 씁니다. {{ui:settings.mobileRemote.regenerateKeyButton}}를 누르면 휴대폰과 컴퓨터가 모두 해제되고, 다시 쓰려면 새 링크로 연결해야 합니다.
- 공유한 워크스페이스만 보이고, {{ui:settings.remote.allowFullAccess}} 스위치와 가둬 둘 수 없는 에이전트의 규칙도 Tailscale 연결과 똑같이 적용됩니다.
- 휴대폰 설정은 바뀌지 않습니다. {{ui:settings.remote.phoneTab}} 탭에서 휴대폰 연결을 허용하지 않았다면 이 스위치가 켜져 있어도 휴대폰은 연결되지 않습니다.
- 릴레이는 한 번에 1 MB까지 보냅니다. 페어링 연결로 보내는 첨부 파일은 합계 약 750 KB까지이고(파일 내용이 base64로 늘어납니다), 더 크면 보내기 전에 알려 줍니다. 큰 파일은 Tailscale로 연결해 보내세요. 긴 대화는 최근 항목부터 1 MB 안에 드는 만큼 보입니다.
- 페어링 연결에서는 [그 컴퓨터의 창 그대로](#mirror)만 씁니다. 이 컴퓨터의 창을 그 컴퓨터에서 실행하는 원격 창({{ui:settings.remote.openButton}} 단추)은 Tailscale 연결에서만 됩니다.
- 호스트가 공유를 멈추거나 스위치를 끄면 연결한 컴퓨터에 공유를 멈췄다는 말이 보입니다. 호스트가 다시 공유하면 링크를 다시 받을 필요 없이 {{ui:settings.remote.refreshButton}} 단추로 다시 연결합니다.

## Windows 방화벽 {#firewall}
{{platform:windows}} Windows에서는 {{ui:settings.remote.shareTitle}}와 {{ui:settings.remote.phoneTab}} 탭 아래에 {{ui:settings.remote.firewallTitle}} 항목이 있습니다. Tailscale 대역에서 오는 연결에만 앱의 포트를 여는 규칙 하나를 관리하고, 지금 상태가 한 줄로 보입니다.
- {{ui:remote.windows.firewall.add}} 단추로 규칙을 추가합니다. 이때 Windows가 관리자 확인 창을 한 번 띄웁니다.
- 규칙이 이미 있으면 같은 자리에 {{ui:remote.windows.firewall.update}} 단추가 나와 지금 쓰는 포트에 맞춰 고칩니다. 규칙의 포트가 지금 쓰는 포트를 모두 담지 않으면 갱신이 필요하다고 표시합니다.
- {{ui:remote.windows.firewall.remove}} 단추로 규칙을 지웁니다.

이 규칙 하나가 워크스페이스 공유 포트와 휴대폰 [직접 연결](phone.md#direct) 포트를 함께 엽니다. 그래서 두 탭 어느 쪽에서 고쳐도 같은 규칙이 바뀌고, 규칙을 지우면 두 포트가 함께 닫힙니다. 페어링 연결은 릴레이를 거치므로 이 규칙이 필요 없습니다.

## 그 컴퓨터의 창 그대로 {#mirror}
다른 컴퓨터에 연결하면, 그 컴퓨터가 공유한 워크스페이스가 거기 열려 있는 에이전트 창 그대로 사이드바에 나타납니다. 대화, {{ui:graph.view.mighty}} 보기, 진행 상황과 상태가 실시간으로 따라오고, 여기서 보내기·멈추기와 승인·질문·계획 답을 할 수 있습니다. 창은 그 컴퓨터에서 그 컴퓨터의 CLI와 계정으로 실행되고, 보낸 글은 그 창의 다음 요청이 됩니다. 실행 중이면 그 컴퓨터의 대기열에 들어갑니다.

- 사이드바에는 이 컴퓨터의 워크스페이스 아래에 서버마다 "원격 · 서버 이름" 머리글이 생기고, 주소와 연결 상태가 보입니다. 점이 초록이면 연결됨, 주황이면 다시 연결하는 중, 회색이면 끊김입니다. 머리글의 … 메뉴에서 {{ui:remote.mirror.connect}}·{{ui:remote.mirror.disconnect}}를 고릅니다.
- 원격 창 위에는 {{ui:remote.mirror.paneBadge}} 띠가 붙어 어느 컴퓨터에서 실행 중인지와 연결 상태, 그 컴퓨터의 대기열에 있는 요청 수를 보여 줍니다. 아래 상태 막대도 "원격 · 그 컴퓨터에서 실행 중 · 연결됨"처럼 표시합니다.
- {{ui:workspace.addPane}} 메뉴로 그 컴퓨터에 새 에이전트 창을 열 수 있고, 창을 닫으면 그 컴퓨터의 창도 닫힙니다.
- 연결을 끊으면 창은 마지막 모습으로 남고 보낼 수 없습니다. 연결을 지우면 그 서버의 워크스페이스가 사라집니다. 이 창들은 이 컴퓨터에 저장하지 않으므로, 앱을 다시 열면 연결해야 다시 보입니다.
- 대화는 최근 80개 항목까지 보이고 그림은 보이지 않습니다. 이름 바꾸기와 모델·권한 모드 바꾸기는 그 컴퓨터에서 합니다. 명령 창은 보이지 않고, [원격 창에서 쓸 수 없는 기능](#use)도 마찬가지입니다.
- 전체 권한으로 실행하는 창에 보내기와 "항상 허용"은 그 컴퓨터가 {{ui:settings.remote.allowFullAccess}}을 켰을 때만 됩니다. 그 컴퓨터가 가둬 두지 못하는 에이전트 창에 보내기와 답하기(권한 허용, 질문, 계획)도 이 스위치가 켜져 있어야 합니다. Mac이면 Antigravity·pi·opencode·Hermes, Windows이면 Claude가 아닌 모든 에이전트입니다.
- 그 컴퓨터의 Mighty Code가 이 기능을 지원하지 않는 이전 버전이면 창이 보이지 않고 머리글에 이유가 나옵니다. 그때는 {{ui:settings.remote.connectionsTitle}} 목록의 {{ui:settings.remote.openButton}} 단추로 그 워크스페이스를 이 컴퓨터의 원격 창으로 씁니다(Tailscale 연결).

## 원격 창 쓰기 {#use}
- 에이전트 창은 로컬 창처럼 쓰면 됩니다. 모델·권한·사고 강도·Fast 모드는 그대로 호스트로 전해지고, 호스트 CLI가 지원하지 않는 설정은 실행 전에 알려 줍니다.
- 승인·질문·계획 카드는 이 앱에 나타나고, 답하면 호스트의 실행으로 전해집니다. 실행 중에 보낸 요청은 로컬 창처럼 대기열에 들어가 지금 실행이 끝난 뒤 시작합니다.
- {{ui:graph.view.mighty}} 보기({{ui:graph.view.diagram}}·{{ui:graph.view.timeline}}·{{ui:graph.view.structure}})가 로컬 실행과 똑같이 그려집니다.
- {{ui:remote.menu.newCommand}} 창은 명령마다 호스트의 공유 폴더에서 새 셸을 엽니다. 대화형 프로그램과 비밀번호 입력은 지원하지 않습니다.
- 원격 창은 상태 막대와 세션 정보에 실행 컴퓨터의 OS(macOS·Windows)를 보여 줍니다. {{ui:remote.menu.newCommand}} 창은 Mac 호스트에서는 로그인 셸, Windows 호스트에서는 PowerShell로 명령을 실행합니다.
- 파일을 첨부하면 내용 자체가 호스트로 갑니다. 한 번에 8개, 파일당 5 MiB, 합계 8 MiB까지입니다.

원격 워크스페이스에서는 다음을 쓸 수 없습니다. 모두 이 컴퓨터의 디스크나 네트워크를 읽기 때문입니다.
- [마이티 오케스트레이션](delegation.md)(자식 작업 트리)
- 파일 창
- 프로젝트 파일을 읽는 [스타일](styles.md), 상태 줄, 이전 세션 이어가기

pi, opencode, Hermes 창은 아직 원격 워크스페이스에서 열 수 없습니다. 원격 워크스페이스는 [휴대폰 원격](phone.md)에 보이지 않습니다.

## 연결이 끊겼을 때 {#disconnect}
- 네트워크가 끊기면 실행 중인 원격 창은 오류로 끝나고 {{ui:settings.remote.connectionsTitle}} 목록에 이유가 표시됩니다. 호스트는 확인이 끊긴 실행을 약 30초 안에 스스로 멈춥니다.
- {{ui:settings.remote.disconnectButton}} 단추는 그 연결의 실행을 멈추고 연결 정보는 남깁니다. 다시 쓰려면 {{ui:settings.remote.refreshButton}} 단추를 누르거나 원격 창에서 보냅니다. 연결 정보까지 지우려면 그 줄의 지우기 아이콘을 누릅니다.
- Tailscale 호스트가 공유를 다시 시작했다면 새 키가 필요합니다. 같은 주소로 새 키를 넣어 연결하면 저장된 연결의 키가 바뀝니다.
- 호스트 컴퓨터가 잠들거나 Mighty Code가 종료되면 새 작업을 보낼 수 없습니다.
