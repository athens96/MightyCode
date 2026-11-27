---
title: 手机远程
order: 9
section: phone
---
通过手机应用，你在外面也能查看和操作 Mac 或 Windows 电脑上的 Mighty Code：查看任务状态、发送请求、回答批准和提问。

电脑与手机通过中继服务器连接。默认中继内置在应用中，无需另外准备。内容全程端到端加密，中继无法查看内容。无需在路由器上开放端口，也无需使用 VPN。如果你在用 Tailscale，手机也可以[不经中继直接连接](#direct)。

## 获取手机应用 {#get-app}
从[下载页面](https://pub-b705901d18c940159d40f58683d758ad.r2.dev/mightycode/index.html)下载 Android 应用（APK）并安装。

## 连接手机 {#pair}
在电脑上：
1. 打开{{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → {{ui:settings.remote.phoneTab}}标签页。
2. 打开{{ui:settings.mobileRemote.allowToggle}}。在 Windows 上是{{ui:windows.mobile.allow}}。旁边会显示连接状态；如果一直停在正在连接中继，请点击{{ui:settings.mobileRemote.reconnectButton}}按钮。
3. {{ui:windows.mobile.pairingTitle}}中会出现二维码和三个步骤的说明。

在手机上：
1. 打开手机应用，轻点{{ui:phone.tabs.hosts}}标签页中的{{ui:phone.hosts.addHost}}。首次打开时会先显示连接指南。
2. 用{{ui:phone.pair.mode.qr}}扫描电脑上的二维码。如果无法使用相机，请在电脑上点击{{ui:settings.mobileRemote.copyLinkButton}}按钮，把链接发到手机，再粘贴到{{ui:phone.pair.mode.manual}}中。链接中含有密钥，因此会作为机密内容复制，并在一分钟后从剪贴板清除。
3. 轻点{{ui:phone.pair.connect}}后，电脑上会弹出“新设备请求连接”窗口和通知，手机上会提示你在电脑上允许。在电脑上点击{{ui:settings.mobileRemote.approval.allow}}即可连接，这部手机会出现在{{ui:settings.mobileRemote.connectedDevicesTitle}}中。60 秒内未允许或点击{{ui:settings.mobileRemote.approval.refuse}}（默认按钮）则不会连接。不是你亲自配对的手机请不要允许。

二维码旁有{{ui:settings.mobileRemote.hostIdLabel}}和{{ui:settings.mobileRemote.keyLabel}}。密钥要点击显示按钮才会显示。二维码、链接和密钥是一次性的配对码：显示在屏幕上时一直有效，关闭界面 10 分钟后失效，有一台设备连接或被拒绝后会换成新的配对码。已配对的设备用设备令牌连接，不会再次请求允许。

{{ui:windows.mobile.connectionTitle}}中有两种连接方式：
- {{ui:windows.mobile.relayTitle}}：默认方式，无需任何操作。
- {{ui:mobileRemote.direct.toggle}}：设置端口并应用，下方显示状态。请参阅[通过 Tailscale 直接连接](#direct)。

![[settings-mobile]]

已连接的手机和[已配对的电脑](remote.md#pair)会在{{ui:settings.mobileRemote.connectedDevicesTitle}}列表中各占一行，电脑以电脑图标区分（重启应用后仍保留）。只要有任何设备，即使手机连接开关已关闭，这个列表也会显示。不再使用某台设备时，请点击该行的{{ui:settings.mobileRemote.revokeRowButton}}按钮。只有这台设备会解除配对，其他手机和电脑仍保持连接。此时屏幕上的配对码也会更新。已连接的手机再次扫描二维码连接时，会像新设备一样请求允许（并提示将替换原设备），允许后旧的连接不再可用。

> [!warning]
> 点击{{ui:settings.mobileRemote.regenerateKeyButton}}后，所有已连接的手机和已配对的电脑都会解除配对，这台电脑的中继 ID 和加密密钥也会重新生成，所有设备都需要用新的二维码或链接重新连接。请在有人未经允许连接或设备丢失时使用。

## 手机可见的工作区 {#workspaces}
在 {{ui:settings.remote.phoneTab}} 标签页的 {{ui:settings.mobileRemote.workspacesTitle}} 中，选择配对的手机能看到这台电脑的哪些工作区。
- {{ui:settings.mobileRemote.workspacesAll}}：默认值。手机能看到这台电脑上的所有工作区，并打开其中的窗格、文件（包括 `.env` 这样的文件）和运行。
- {{ui:settings.mobileRemote.workspacesShared}}：手机只能看到 [{{ui:settings.remote.shareTitle}}](remote.md#share) 当前共享的工作区。共享关闭时一个工作区也看不到；不能打开其他工作区的窗格、文件或工作地图，也不能向其发送请求，也收不到这些窗格的通知。

更改从手机的下一个请求起生效，连接不会断开。

## 通过 Tailscale 直接连接 {#direct}
Mac 和手机在同一个 [Tailscale](https://tailscale.com) 网络中时，手机会不经中继直接连接 Mac。配对方式和加密都不变；直接连不上 Mac 时，手机会自动改用中继。

1. 在 Mac 和手机上打开 Tailscale，并用同一个账号登录。
2. 在电脑的 {{ui:settings.remote.phoneTab}} 标签页中确认 {{ui:mobileRemote.direct.toggle}} 已打开（默认打开）。下面一行显示 Tailscale 地址就说明准备好了。如果显示 {{ui:mobileRemote.direct.status.notFound}}，说明 Mac 上的 Tailscale 没有打开。
3. 已经配对的手机不需要重新配对。下次连接时 Mac 会告诉它自己的地址。

在手机的 {{ui:phone.tabs.hosts}} 标签页中，Mac 卡片上会显示 {{ui:phone.direct.route.direct}} 或 {{ui:phone.direct.route.relay}}。

Windows 电脑也一样，只是需要一条 Windows 防火墙规则，以免连接被拦截。在 {{ui:settings.remote.phoneTab}} 标签页下方的 {{ui:settings.remote.firewallTitle}} 中点击 {{ui:remote.windows.firewall.add}} 按钮，并允许 Windows 的管理员确认。该规则只允许来自 Tailscale 地址的连接到达本应用。规则存在后，可以用 {{ui:remote.windows.firewall.update}} 按钮按当前使用的端口调整规则，用 {{ui:remote.windows.firewall.remove}} 按钮删除规则。关闭直接连接或手机连接时会一并删除规则（如果这台电脑正在[共享工作区](remote.md#share)，还需要它的端口，规则就会保留），Windows 会再次请求管理员确认。这是同一条规则，同时打开手机直连的端口和工作区共享的端口，因此在 {{ui:settings.remote.shareTitle}} 标签页中修改的也是同一条规则（[Windows 防火墙](remote.md#firewall)）。

## 在手机上可以做的事 {#use}
手机应用有四个标签页。

| 标签页 | 内容 |
|---|---|
| {{ui:phone.tabs.dashboard}} | {{ui:phone.dashboard.title}}：所有已配对电脑的运行中、待你处理和已完成数量合计，下面依次列出每台电脑的工作区和会话。打开会话可以查看对话并发送请求。 |
| {{ui:phone.tabs.alerts}} | 需要你回复的批准和提问，以及已完成的任务 |
| {{ui:phone.tabs.bots}} | 所有已配对电脑的机器人及其状态、待审批项和机器人报告（{{ui:badge.beta}}）。参见[机器人](#bots)。 |
| {{ui:phone.tabs.hosts}} | 已连接的电脑（Mac、Windows）及其连接状态 |

在会话中，可以发送和停止请求，更改模型、权限和思考强度，使用斜杠命令，附加文件，以及查看工作区文件（只读）。

会话中的任务正在运行时，发送按钮会变成{{ui:queue.add}}，你发送的内容会作为该窗格的下一个请求进入队列，不会插入正在运行的任务。

当电脑上的应用足够新时，排队的请求会显示{{ui:queue.runNow}}按钮。点按后，由电脑决定是在电脑上的新 worktree 子窗格中运行，还是在本窗格中运行。灰色的按钮在点按时会说明原因。手机不能选择代理。

当电脑上的应用支持编辑队列时，每个请求还会显示{{ui:queue.edit}}和 ↑、↓ 按钮。{{ui:queue.edit}}可就地编辑文字，用{{ui:queue.editSave}}保存（文字为空时不会保存）。↑、↓ 调整顺序，电脑按新顺序运行。电脑为该窗格保留的行不能编辑、移动或删除。

在{{ui:phone.session.view.blocks}}视图中，已完成的请求会直接显示请求和{{ui:phone.blocks.finalResult}}。过程步骤折叠为一行，点击即可展开。仍在运行的请求会显示所有步骤。

当结果提到工作区中的文件时，{{ui:phone.blocks.finalResult}}下方会附上{{ui:graph.resultFiles.title}}列表。其中的文件与桌面应用结果旁的列表相同，文件名下方以浅色显示其所在的文件夹。文件较多时只显示前五个，其余的用下方的按钮展开。点击文件即可在工作区文件查看中打开它。该列表只随成功的结果出现；其他电脑上的工作区，或不发送此列表的旧版桌面应用，不会显示它。 如果该文件是此请求更改过的源代码或 Markdown，顶部会出现 {{ui:files.changes.show}} | {{ui:files.changes.full}} 标签，并先显示{{ui:files.changes.show}}：只显示更改的行，附有旧行号和新行号，添加的行为绿色背景，删除的行为红色背景。只有支持此功能的桌面应用才会提供这个视图。

在桌面应用（Mac 和 Windows）中通过 [Mighty 编排](delegation.md)创建的子窗格，会在会话列表中缩进一级显示在父窗格下方，右侧显示其状态（{{ui:delegation.child.state.reported}}、{{ui:delegation.child.state.merged}} 等）。不发送这些信息的旧版应用中的子窗格，在列表中与普通窗格一样显示。合并或丢弃子窗格无法在手机上进行，只能在桌面应用（Mac 或 Windows）中进行。只有对比运行的子窗格可以在下文的对比结果中合并或丢弃。使用 Antigravity 的桌面应用（Mac 0.4.23 及以上、Windows 0.4.28 及以上）中的 Gemini 窗格显示为 Antigravity（{{ui:badge.beta}}），更旧版本应用中的窗格显示为 Gemini。

在自己的 worktree 中工作的窗格，会在画面顶部显示 worktree 栏。PORT 旁边显示分配给该 worktree 的端口范围（例如 `:41010–41019`），下方显示正在运行的[运行操作](delegation.md#mighty-yaml)及其开发服务器地址。地址是电脑上的地址，点按只会复制它。电脑上的 Mighty Code 为 0.6.0 或更高版本时，可以用 {{ui:phone.worktree.runStart}}、{{ui:phone.worktree.runStop}} 按钮在手机上启动和停止这些运行操作。只显示在电脑上批准过的 `.mighty.yaml` 中的运行操作（手机只发送名称，不发送命令），SSH 和远程工作区中不会运行，每次启动或停止都会在电脑上该窗格的记录里留下一行。电脑在 [Tailscale](#direct) 上、且开发服务器也在其 Tailscale 地址上监听时（例如用 `--host 0.0.0.0` 启动的服务器），地址会显示为 Tailscale 地址，点按即在手机浏览器中打开；手机也必须在同一个 Tailscale 网络中。只在 `localhost` 上监听的服务器不会被暴露，仍像以前一样复制地址。点按 {{ui:git.checks.tab}} 按钮，可以只读查看与桌面应用检查标签相同的内容（未提交的更改、PR 状态、CI 检查、未解决的评审讨论、剩余待办）。合并时如果这些检查仍有警告，手机会列出它们并再确认一次；点按 {{ui:git.checks.merge.confirm}} 即可合并。

开始了[对比运行](delegation.md#compare)的窗格，会在栏下方显示对比运行卡片。点按后在 {{ui:compare.view.title}} 中查看每个子窗格的代理和模型、状态、更改的文件、耗时、费用和测试结果，可以{{ui:compare.action.merge}}已报告的子窗格，或{{ui:compare.action.discard}}子窗格。合并后，手机会询问是否一次丢弃其余子窗格（{{ui:compare.rest.title}}）。

桌面应用标记为在等你的窗格（终端通知或响铃、批准或提问卡片、失败的 setup），会在会话列表中标题后面显示一个橙色圆点，{{ui:phone.tabs.dashboard}} 标签的计数下方会用一行显示这类窗格的数量。点按即可打开最近被标记的窗格。[分叉的窗格](delegation.md#fork)会显示指向原窗格的链接，在沙箱中运行的窗格会显示 {{ui:sandbox.badge}} 徽章和沙箱最近拦截的内容。这些标记只在会发送它们的桌面应用（0.6.0 及以上）中显示。

## 机器人 {#bots}
{{ui:phone.tabs.bots}} 标签页是在手机上查看桌面应用的[机器人标签页](agent-pane.md#bots)的 {{ui:badge.beta}} 功能。它在一个屏幕上显示所有已配对的电脑，有多台电脑时按电脑名称分开显示。

- 顶部的 {{ui:automation.approvals.title}} 列出被[策略](agent-pane.md#bots)留待你批准的触发器操作：事件、{{ui:automation.approvals.rule}}、{{ui:automation.approvals.what}} 和 {{ui:automation.approvals.when}}。{{ui:automation.approvals.run}} 会再确认一次，然后在电脑上执行；{{ui:automation.approvals.skip}} 会立即跳过。只能在应用内批准：这条通知没有按钮，因此锁定的手机无法批准任何操作。
- 下面依次是 {{ui:bot.sidebar.otherBots}} 和 {{ui:bot.sidebar.workspaceBots}}，每组中等你处理的机器人排在最上面。每行显示状态圆点和文字（{{ui:bot.sidebar.state.needsYou}}、{{ui:bot.sidebar.state.working}}、{{ui:bot.sidebar.state.idle}}、{{ui:bot.sidebar.state.asleep}}）、@名称和工作区、正在做的事或下次运行时间，以及最近一份报告；未读报告旁有一个小圆点。
- 点按机器人会打开它的页面。{{ui:automation.jobs.runNow}} 会立即运行它的一个计划或触发器（有多个时先选择一个），{{ui:bot.menu.sleep}} 和 {{ui:bot.menu.wake}} 让它休眠或唤醒，{{ui:bot.report.open}} 打开它的窗口。下面列出它最近的报告。
- {{ui:phone.bots.view.reports}} 按从新到旧列出所有机器人的报告。打开报告会标记为已读，并显示链接、触发原因、{{ui:bot.report.rerun}} 和 {{ui:bot.report.open}}。报告只包含链接，不包含文件路径或记录。
- 如果手机只能看到共享的工作区，就只显示这些工作区的机器人、它们的报告和其触发器的待审批项。
- 标签页上的数字是待审批项和等你处理的机器人数量之和。
- 此标签页需要具备该功能的桌面应用。桌面应用较旧时，会提示你更新桌面应用。用非常旧的手机应用配对的手机会被拒绝立即执行、重新执行和批准执行，请重新配对。

## 批准与提问 {#approvals}
智能体请求批准或提问时，{{ui:phone.tabs.alerts}}标签页上会显示数量。
- 权限请求：轻点{{ui:permission.allow}}或{{ui:permission.deny}}。
- Antigravity 权限卡片：每项被拒绝的操作都会显示类型以及命令、地址或路径。可选择{{ui:permission.allowOnce}}、{{ui:antigravity.permission.allowAlways}}或{{ui:antigravity.permission.deny}}；允许后会在同一个对话中继续（[Antigravity 的权限卡片](approvals.md#antigravity-permission)）。如果卡片中只有没有规则可以允许的操作，则只有{{ui:antigravity.permission.deny}}，并提示你更改权限模式。如果命令会串联、使用管道、重定向或可能删除数据，则不显示{{ui:antigravity.permission.allowAlways}}。如果主机无法接受你的答复，卡片会保留并显示原因。不认识这种卡片的旧版手机应用会把它显示为普通的权限请求。
- 提问：选择答案后，轻点{{ui:phone.questionnaire.submit}}。
- 计划卡片：用与 Mac 上相同的按钮（{{ui:plan.card.approveAuto}}、{{ui:plan.card.revise}}等）回答。

## 通知 {#notifications}
已连接的电脑（Mac 或 Windows）上的运行完成或失败时，以及需要你处理权限请求、问题、待批准的计划或登录过期时，手机会收到通知。目前只有 Android 应用会收到。
- 第一次打开应用时（Android 13 及以上）会询问一次是否允许通知。要按类型关闭，请在 {{ui:phone.tabs.hosts}} 标签页打开 {{ui:phone.settings.title}}，修改 {{ui:phone.settings.notifications}} 下的开关。如果系统设置里该应用的通知被关闭，{{ui:phone.settings.title}} 会提示。
- 收到权限请求通知时，可以直接点 {{ui:permission.allow}} 或 {{ui:permission.deny}}。Antigravity 卡片提供 {{ui:permission.allowOnce}}、{{ui:antigravity.permission.allowAlways}} 和 {{ui:antigravity.permission.deny}}。只能允许一次的卡片（例如串联命令或可能删除数据的命令）没有 {{ui:antigravity.permission.allowAlways}}。问题和计划的通知没有按钮，点开后在应用里回答。没有任何可允许操作的 Antigravity 卡片同样不带按钮，点开后在应用里回答。
- 按钮只有在手机解锁后才起作用。锁屏时点按，通知会变为“{{ui:phone.push.outcome.locked}}”，按钮仍保留。如果已在电脑上回答，通知会变为 {{ui:phone.push.outcome.already}}；如果答复没能发出，会变为 {{ui:phone.push.outcome.failed}}。最后这种情况请点开通知，在应用里回答。
- 可以按工作区关闭通知。点按工作区名称旁的铃铛，该工作区的通知就会关闭，铃铛上会出现斜线。再点一次即可重新开启。所有工作区一开始都是开启的，这个设置只对这部手机有效，连接同一台电脑的其他手机照常收到。应用还没见过该工作区，或超过一天没有收到电脑的列表时，通知仍可能送达。
- 点按通知会打开对应的会话。手机正在显示某个会话时，不会收到关于它的通知。
- 点按“触发器操作等待批准”的通知会打开 {{ui:phone.tabs.bots}} 标签页的 {{ui:automation.approvals.title}}。没有窗口的机器人报告通知会打开 {{ui:phone.bots.view.reports}}。
- 通知在锁屏上以私密方式显示：如果你设置了在锁屏上隐藏敏感内容，标题和正文就不会显示。
- 电脑会加密每条通知，只有这部手机能读取，中继和 Google 无法读取。通知经由中继发送，因此通过 Tailscale 直接连接时，只要电脑能上网也会收到。
- 电脑应用和手机应用都必须是带有此功能的版本。没有此功能的电脑应用不会发送通知。如果你是用很旧的手机应用配对的，请重新配对。
- 0.4.58 起默认中继换到了新服务器。在此之前连接的手机如果收不到通知，请用二维码重新连接一次。
- 在电脑上关闭手机连接期间，电脑不会发送任何内容；重新打开后，也不会补发这段时间里发生的事。

应用崩溃时只会把出错位置发给开发者，不发送对话、命令或路径。可以在 {{ui:phone.settings.title}} 中关闭 {{ui:phone.settings.crashReports}}。

## 远程屏幕 {#screen}
在手机上查看电脑的屏幕，获得允许后还可以操控。这是{{ui:badge.beta}}功能。

> [!warning]
> 从 0.4.58 起，屏幕查看与控制暂时停用（Mac 和 Windows 都是）。手机上不会出现查看屏幕的按钮，电脑的设置中也没有屏幕查看这一项。为每部手机设定的允许范围和已授予的权限都会保留，重新开启后恢复。

## 桌面应用更新 {#desktop-update}

当 Mac 或 Windows 应用有新版本时，手机上该电脑的会话列表上方会出现 **{{ui:phone.appUpdate.title}}** 横条。按 **{{ui:phone.appUpdate.install}}**，电脑就会下载并安装新版本，然后重新启动应用。你不需要在电脑旁边。

- 有正在运行的请求时，会先告诉你有几个会停止，确认后才继续。
- 应用重新启动时连接会短暂断开，然后自动恢复。
- 电脑从官方地址下载更新，并在验证签名后才安装。手机只负责通知它开始。
- 此功能适用于 0.4.55 及以后版本的电脑。

## 更改中继地址 {#own-relay}
默认中继（Cloudflare）内置在应用中，通常不需要更改。如果需要使用其他中继，请展开 {{ui:settings.remote.phoneTab}} 标签页底部折叠的 {{ui:settings.advancedTitle}}。
- {{ui:settings.mobileRemote.relayLabel}}：填写中继地址（`wss://…`）并点击{{ui:settings.mobileRemote.applyButton}}。留空则使用默认中继。
- {{ui:settings.mobileRemote.legacyAppsToggle}}：默认关闭。开启后（确认警告后），没有设备令牌的很旧的手机应用仅凭下方的{{ui:settings.mobileRemote.legacyKeyLabel}}即可连接，无需允许。任何知道该密钥的人都能远程操作这台电脑，请仅在需要时开启，用完后关闭。当前应用不需要它。
