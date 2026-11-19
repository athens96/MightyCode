---
title: 远程连接
order: 10
section: remote
---
连接两台 Mac 或 Windows 电脑上的 Mighty Code 后，可以在一台电脑上打开另一台的工作区，并在那台电脑上运行 Claude、Codex、Antigravity 和命令。文件和 CLI 登录都留在实际运行的电脑上，不会复制或同步项目文件。

连接方式有两种：
- {{ui:settings.remote.viaPairing}}：像手机一样配对一次，然后经由 Mighty 中继连接，不需要 Tailscale。请参阅[通过配对连接](#pair)。
- {{ui:settings.remote.viaTailscale}}：在同一个 [Tailscale](https://tailscale.com/kb/1017/install) 网络（tailnet）内，用地址和连接密钥直接连接。在那台电脑上运行本机窗格的远程窗格以及大附件只能通过 Tailscale。

这些都在 {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} 标签中进行。标签顶部有一行说明，下方有 {{ui:settings.remote.shareTitle}} | {{ui:settings.remote.connectTitle}} | {{ui:settings.remote.phoneTab}} 三个标签。内部标签只在应用运行期间被记住。连接手机请参阅[手机远程](phone.md)。

## 准备 {#prepare}
- 在两台电脑上安装 Mighty Code。
- 在负责运行的电脑（主机）上安装并登录要用的 CLI。远程运行使用主机的 CLI 和账号。
- 要通过 Tailscale 连接，请在两台电脑上安装 Tailscale 并登录同一个 tailnet。不需要路由器端口转发，也不需要 Tailscale Serve 或 Funnel。只需确认 tailnet 策略允许两台设备互相连接。

## 共享这台电脑 {#share}
在主机电脑上：
1. 打开 {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → {{ui:settings.remote.shareTitle}} 标签。
2. 在 {{ui:settings.remote.pickWorkspaces}} 中勾选要共享的本地工作区。
3. 在下方的 {{ui:settings.remote.methodsTitle}} 中准备要用的方式（见下文）。
4. 点按顶部状态行旁的 {{ui:settings.remote.startSharing}}。共享期间，该位置改为显示 {{ui:settings.remote.sharedListTitle}} 列表。

{{ui:settings.remote.methodsTitle}} 中每种方式占一行：
- {{ui:settings.remote.viaPairing}}：有 {{ui:settings.remote.pairAccept}} 开关、中继状态和 {{ui:settings.remote.pairCopyLink}} 按钮。链接中含有与手机二维码相同的密钥，因此会作为机密内容复制，并在一分钟后从剪贴板清除。在无法使用中继的运行中，开关会变灰并显示原因。请参阅[通过配对连接](#pair)。
- {{ui:settings.remote.viaTailscale}}：这台电脑没有 Tailscale 地址时，会显示相应说明和 {{ui:settings.remote.tailscaleInstall}} 按钮，点按后打开 {{ui:settings.nav.tools}} 标签。开始共享前有 {{ui:settings.remote.portLabel}} 输入框，一般保持不变（43137）。共享期间会显示这台电脑的 Tailscale 地址和 {{ui:settings.remote.keyLabel}}。用显示按钮查看密钥，用 {{ui:settings.remote.copyButton}} 复制后告诉要连接的一方。

即使没有 Tailscale，只要打开了 {{ui:settings.remote.pairAccept}}，也能开始共享。此时只有已配对的电脑能经由中继连接。

共享期间会显示 {{ui:settings.remote.allowFullAccess}} 开关。关闭时（默认），已连接的电脑不能使用远程命令窗格，也不能以完全权限运行代理；这类请求会在运行前被拒绝，窗格中会显示原因。每次开始共享时它都会重新关闭。

只要这个开关关闭，已连接的电脑也不能使用共享电脑无法限制在共享文件夹内的智能体窗格（无论权限模式如何）：由 Mac 共享时是 Antigravity、pi、opencode 和 Hermes（它们都没有沙箱），由 Windows 共享时是包括 Codex 在内的所有 Claude 以外的智能体（Windows 无法像 Mac 那样把 Codex 限制在沙箱内）。Mac 上的 Codex 在自己的沙箱中运行，两边的 Claude 窗格在开关关闭时也照常运行并请求权限。在这类窗格中，不只是发送内容，允许工具、回答问题和批准计划也需要打开这个开关。

Tailscale 共享只在这台电脑的 Tailscale 地址上接受连接。每次打开应用时共享都是关闭的。点按 {{ui:settings.remote.stopSharing}} 会停止远程运行并作废连接密钥；再次共享时需要告知新的密钥。

> [!warning]
> 持有连接密钥或配对链接的设备可以在共享的工作区中运行任务和命令。根据权限模式，它们可能以主机用户的权限访问文件夹以外的内容。请只把密钥和链接告诉你信任的设备。

## 连接其他电脑 {#connect}
在要下达任务的电脑上：
1. 打开 {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → {{ui:settings.remote.connectTitle}} 标签。{{ui:workspace.addPane}} 菜单中的 {{ui:remote.menu.connect}} 也会打开同一处。
2. 在上方的二选一开关中选择 {{ui:settings.remote.pairLinkField}} 或 Tailscale 地址。
   - {{ui:settings.remote.pairLinkField}}：粘贴从主机收到的链接，可按需填写 {{ui:settings.remote.nameField}}。
   - Tailscale 地址：输入 {{ui:settings.remote.addressField}}（Tailscale IP 或 MagicDNS 名称）和 {{ui:settings.remote.keyField}}，可按需填写名称。如果主机没有更改端口，只输入地址即可。
3. 点按 {{ui:settings.remote.connectButton}}。

下方的 {{ui:settings.remote.connectionsTitle}} 列表中每个连接占一行，显示电脑名称、方式标签（配对或 Tailscale）、状态（{{ui:settings.remote.status.connected}}、{{ui:settings.remote.status.disconnected}}、{{ui:settings.remote.status.saved}}），以及 {{ui:settings.remote.refreshButton}}、{{ui:settings.remote.disconnectButton}} 按钮和删除图标。已连接电脑下方会列出共享的工作区。连接后，那台电脑共享的工作区会连同其中打开的窗格自动出现在侧边栏；点按 {{ui:remote.mirror.view}} 可直接前往（[原样显示那台电脑的窗格](#mirror)）。标签底部有折叠的 {{ui:settings.remote.scopeTitle}}。

远程工作区在侧边栏中以电脑图标和主机名称区分，工作区标题和底部状态栏会显示连接状态。重新打开应用后，连接仍保持已保存状态，在远程窗格中第一次发送或点按 {{ui:settings.remote.refreshButton}} 时会重新连接。

{{platform:mac}} 连接密钥只保存在 macOS 钥匙串中，不会写入工作区记录。重新连接时如果 macOS 询问钥匙串访问权限，请选择“始终允许”；应用以相同的签名发布，因此这个选择在更新后仍然有效。

{{platform:windows}} 在 Windows 上，连接密钥同样不会写入工作区记录，而是加密保存在应用的数据文件夹中，只有当前 Windows 用户可以读取。通过 Tailscale 共享需要一条防火墙规则（[Windows 防火墙](#firewall)）。

## 通过配对连接（无需 Tailscale） {#pair}
两台电脑都没有 Tailscale 时，也可以像手机一样配对一次，然后经由与[手机远程](phone.md)相同的 Mighty 中继连接。中继内置在应用中，无需另外准备。连接采用端到端加密，中继无法读取内容；连接方使用[原样显示那台电脑的窗格](#mirror)。

在主机电脑上：
1. 打开 {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → {{ui:settings.remote.shareTitle}} 标签，在 {{ui:settings.remote.methodsTitle}} 的 {{ui:settings.remote.viaPairing}} 一行中打开 {{ui:settings.remote.pairAccept}}。
2. 选择工作区并点按 {{ui:settings.remote.startSharing}}。共享期间本机会连接中继，同一行会显示其状态。
3. 点按 {{ui:settings.remote.pairCopyLink}}，把链接交给要连接的电脑。此链接只用于电脑（手机二维码不能配对电脑，反之亦然），只能使用一次，关闭此界面 10 分钟后失效。另一台电脑请求连接时，这台电脑会弹出“新设备请求连接”窗口和通知，确认无误后点按 {{ui:settings.mobileRemote.approval.allow}}（60 秒内未回应将被拒绝）。

在要连接的电脑上：
1. 在 {{ui:settings.remote.navTitle}} → {{ui:settings.remote.connectTitle}} 标签的开关中选择 {{ui:settings.remote.pairLinkField}}，粘贴链接，可按需填写名称，然后点按 {{ui:settings.remote.connectButton}}。在主机允许之前，会提示你在那台电脑上允许。
2. 连接后，那台电脑会带着配对标签出现在 {{ui:settings.remote.connectionsTitle}} 列表中，并以“远程 · 名称”出现在侧边栏中。连接与断开的方式与 Tailscale 连接相同。

- 链接中含有电脑专用的短时配对码。即使链接落入他人手中，只要你不在主机上允许就不会连接。第一次连接时，主机会给这台电脑一个专属的设备令牌，之后都用这个令牌连接，不会再次请求允许。令牌像连接密钥一样保存在 macOS 钥匙串中（在 Windows 上保存在只有当前用户能解密的文件中），不会写入工作区记录。主机关闭 {{ui:settings.remote.pairAccept}} 或未在共享时，新的电脑无法连接。
- 已配对的电脑也会带着电脑图标出现在主机 {{ui:settings.remote.phoneTab}} 标签的 {{ui:settings.mobileRemote.connectedDevicesTitle}} 列表中（即使手机连接开关已关闭，重启后也会显示）。该行的 {{ui:settings.mobileRemote.revokeRowButton}} 按钮只解除这台电脑的配对，其他手机和电脑仍保持连接。点按 {{ui:settings.mobileRemote.regenerateKeyButton}} 会解除所有手机和电脑的配对，并重新生成主机的中继 ID 和加密密钥，之后需要用新链接重新连接。
- 只能看到共享的工作区，{{ui:settings.remote.allowFullAccess}} 开关以及无法限制的智能体的规则与 Tailscale 连接完全相同。
- 手机的设置不会改变。如果没有在 {{ui:settings.remote.phoneTab}} 标签中允许手机连接，即使打开这个开关，手机也无法连接。
- 中继一次最多传送 1 MB。通过配对连接发送的附件合计约 750 KB 以内（文件内容以 base64 传送会变大），超出时会在发送前提示。大文件请通过 Tailscale 连接后发送。较长的对话会显示能放进 1 MB 的最新条目。
- 配对连接只提供[原样显示那台电脑的窗格](#mirror)。要在那台电脑上运行本机窗格的远程窗格（{{ui:settings.remote.openButton}} 按钮）需要 Tailscale 连接。
- 主机停止共享或关闭开关后，连接方会显示共享已停止。主机重新共享后，点按 {{ui:settings.remote.refreshButton}} 即可重新连接，无需新的链接。

## Windows 防火墙 {#firewall}
{{platform:windows}} 在 Windows 上，{{ui:settings.remote.shareTitle}} 和 {{ui:settings.remote.phoneTab}} 标签下方有 {{ui:settings.remote.firewallTitle}}。它管理一条只向 Tailscale 地址段打开应用端口的规则，并用一行显示当前状态。
- 点按 {{ui:remote.windows.firewall.add}} 添加规则。Windows 会弹出一次管理员确认。
- 规则已存在时，同一位置会出现 {{ui:remote.windows.firewall.update}} 按钮，按当前使用的端口修改规则。如果规则没有包含当前使用的全部端口，会提示需要更新。
- 点按 {{ui:remote.windows.firewall.remove}} 删除规则。

这一条规则同时打开工作区共享端口和手机[直接连接](phone.md#direct)端口。因此在任一标签中修改的都是同一条规则，删除规则会同时关闭这两个端口。配对连接经由中继，不需要这条规则。

## 原样显示那台电脑的窗格 {#mirror}
连接到其他电脑后，那台电脑共享的工作区会连同其中打开的代理窗格一起出现在侧边栏。对话、{{ui:graph.view.mighty}} 视图、进度和状态实时同步，你可以在这里发送、停止，并回答批准、提问和计划。窗格在那台电脑上用它的 CLI 和账号运行；你发送的内容成为该窗格的下一个请求，正在运行时会排进那台电脑的队列。

- 侧边栏在本机工作区下方为每台服务器显示“远程 · 服务器名”标题，以及地址和连接状态。绿点表示已连接，橙点表示正在重新连接，灰点表示已断开。标题的 … 菜单中有 {{ui:remote.mirror.connect}} 和 {{ui:remote.mirror.disconnect}}。
- 远程窗格上方有 {{ui:remote.mirror.paneBadge}} 条，显示在哪台电脑上运行、连接状态以及那台电脑队列中的请求数。底部状态栏显示类似“远程 · 在那台电脑上运行 · 已连接”。
- 用 {{ui:workspace.addPane}} 菜单可在那台电脑上打开新的代理窗格；关闭窗格也会关闭那台电脑上的窗格。
- 断开连接后，窗格保持最后的样子，无法发送。删除连接会移除该服务器的工作区。这些窗格不会保存在本机，重新打开应用后需要连接才会再次出现。
- 对话最多显示最近 80 条，不显示图片。重命名窗格以及更改模型、权限模式请在那台电脑上进行。命令窗格不显示，[远程窗格不能使用的功能](#use)同样不可用。
- 向以完全权限运行的窗格发送内容以及“始终允许”，仅在那台电脑打开了 {{ui:settings.remote.allowFullAccess}} 时可用。 向那台电脑无法限制的智能体窗格发送内容和作答（允许工具、回答问题、计划）也需要打开这个开关：Mac 上是 Antigravity、pi、opencode 和 Hermes，Windows 上是 Claude 以外的所有智能体。
- 如果那台电脑的 Mighty Code 是不支持此功能的旧版本，窗格不会出现，标题会说明原因。此时请用 {{ui:settings.remote.connectionsTitle}} 列表中的 {{ui:settings.remote.openButton}} 按钮，把其工作区作为本机的远程窗格使用（Tailscale 连接）。

## 使用远程窗格 {#use}
- 代理窗格的用法与本地窗格相同。模型、权限模式、思考强度和 Fast 模式会原样传给主机；主机 CLI 不支持的设置会在运行前提示。
- 权限、提问和计划卡片会出现在本应用中，你的回答会传给主机上的运行。运行期间发送的请求会像本地窗格一样放入队列，在当前运行结束后开始。
- {{ui:graph.view.mighty}} 视图（{{ui:graph.view.diagram}}、{{ui:graph.view.timeline}}、{{ui:graph.view.structure}}）的显示与本地运行完全相同。
- {{ui:remote.menu.newCommand}} 窗格的每条命令都会在主机的共享文件夹中打开新的 shell。不支持交互式程序和密码输入。
- 远程窗格会在状态栏和会话信息中显示运行电脑的系统（macOS 或 Windows）。{{ui:remote.menu.newCommand}} 窗格在 Mac 主机上用登录 shell、在 Windows 主机上用 PowerShell 运行命令。
- 附加的文件会以内容本身发送：一次最多 8 个，每个 5 MiB，合计 8 MiB。

远程工作区中不能使用以下功能，因为它们读取的是这台电脑的磁盘或网络：
- [Mighty 编排](delegation.md)（子工作树）
- 文件窗格
- 读取项目文件的[风格](styles.md)、状态行，以及继续以前的会话

pi、opencode 和 Hermes 窗格暂时还不能在远程工作区中打开。远程工作区不会显示在[手机远程](phone.md)中。

## 连接断开时 {#disconnect}
- 网络断开时，正在运行的远程窗格会以错误结束，{{ui:settings.remote.connectionsTitle}} 列表会显示原因。主机会在约 30 秒内自行停止失去联系的运行。
- {{ui:settings.remote.disconnectButton}} 会停止该连接的运行并保留连接信息。要再次使用，请点按 {{ui:settings.remote.refreshButton}} 或在远程窗格中发送。要连同连接信息一起删除，请点按该行的删除图标。
- 如果 Tailscale 主机重新开始共享，就需要新的密钥。用新密钥连接同一个地址，会替换已保存连接的密钥。
- 主机电脑休眠或其 Mighty Code 退出时，无法发送新的任务。
