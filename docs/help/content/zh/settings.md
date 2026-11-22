---
title: 设置
order: 8
section: settings
---
通过{{ui:menu.settings}}（{{kbd:⌘,}}）或侧边栏底部的设置按钮打开。在左侧选择标签页，用{{ui:settings.closeButton}}按钮或{{kbd:Esc}}关闭。设置会记住你上次查看的标签页。标签页的顺序是{{ui:settings.nav.general}}、{{ui:settings.nav.models}}、{{ui:settings.nav.styles}}、{{ui:settings.remote.navTitle}}、{{ui:settings.nav.companion}}、{{ui:settings.nav.tools}}、{{ui:settings.nav.about}}。

## 通用 {#general}
在{{ui:settings.nav.general}}标签页的{{ui:settings.display.sectionTitle}}中更改应用的外观。
- {{ui:settings.display.themeLabel}}：{{ui:settings.display.themeDarkWindows}}或{{ui:settings.display.themeLightWindows}}。也可以用侧边栏底部的按钮切换。
- {{ui:settings.display.languageLabel}}：应用的语言。请参阅[更改语言](getting-started.md#language)。

![[settings-general]]

### 文字大小与字体 {#text}
在同一标签页的 {{ui:settings.text.sectionTitle}} 中更改整个应用的文字。
- {{ui:settings.text.sizeLabel}}：在 85% 到 150% 之间选择。对话、输入框、侧边栏、设置和终端会一起放大或缩小。
- {{ui:settings.text.uiFontLabel}}：应用界面的字体，例如侧边栏、菜单、按钮和设置。从这台电脑上安装的字体中选择；{{ui:settings.text.fontSystem}} 是操作系统的默认字体（Mac 上为 macOS 字体，Windows 上为 Segoe UI）。
- {{ui:settings.text.transcriptFontLabel}}：代理的回复、你发送的消息、输入框以及 Mighty 卡片和看板正文的字体。选择 {{ui:settings.text.sameAsInterface}} 时跟随界面字体。
- {{ui:settings.text.codeFontLabel}}：代码块、行内代码、工具输出和命令行以及终端窗格的字体。只列出等宽字体：已安装的等宽字体，以及已安装的 JetBrains Mono、D2Coding、Fira Code、Cascadia Code、Cascadia Mono、Consolas 等编程字体。最上面的默认值在 Mac 上代码使用 SF Mono、终端使用 Menlo，在 Windows 上使用 Cascadia Mono（没有时用 Consolas）。在 Mac 上，所选字体被删除时，终端会改回 Menlo。已打开的终端也会立即更改。路径、版本、数量等应用界面中的等宽文字仍使用系统等宽字体。
- 每种字体下方都会用该字体显示一行预览。
- {{ui:settings.text.reset}} 按钮把大小和三种字体恢复到最初的状态。

在 Mac 上也可以用“显示”菜单中的 {{ui:menu.textBigger}}（{{kbd:⌘=}}）、{{ui:menu.textSmaller}}（{{kbd:⌘-}}）和 {{ui:menu.textActualSize}}（{{kbd:⌘0}}），在 Windows 上用 {{kbd:Ctrl+=}} / {{kbd:Ctrl+-}} / {{kbd:Ctrl+0}} 更改大小（在 Windows 上光标位于终端窗格时，这些键由终端使用）。更改会立即生效，重新打开应用后仍然保留。

### 代理窗格 {#agents}
同一标签页的{{ui:settings.agents.sectionTitle}}中是与代理窗格有关的设置和按键说明。
- {{ui:settings.display.statusLineToggle}}：运行 Claude 设置中的状态栏命令，并将结果显示在输入框下方。
- {{ui:settings.delegation.toggle}}：默认开启。Claude 窗格可以在各自的 worktree 中打开子窗格，在运行中的 Claude 窗格里按 {{kbd:⌘Enter}} 会在新的子窗格中运行你的请求。请参阅[Mighty 编排](delegation.md#enable)。
- {{ui:settings.agents.checklistToggle}}：默认开启。开启时会附上一条简短指示，请 Gemini、Codex、opencode 和 Hermes 在工作时留下计划（清单），进度面板就会显示真正的计划步骤。窗格中的请求只保留你输入的内容。关闭时或没有收到计划时，面板把智能体正在做的事显示为步骤（[Mighty 视图](agent-pane.md#diagram)）。
- {{ui:settings.editor.label}}：{{ui:editor.open}}启动的编辑器。可选择自动(找到的第一个)、此电脑上找到的编辑器或{{ui:settings.editor.system}}。每次打开设置都会重新查找([在编辑器中打开](terminal-files-browser.md#open-in-editor))。
- 下方是按键说明：按 {{kbd:Enter}} 发送，按 {{kbd:Shift+Enter}} 换行。拖动标签页可与其他标签页合并，拖到边缘可拆分。命令运行窗格是交互式终端，在 Mac 上打开登录 shell，在 Windows 上打开 PowerShell（没有时为 cmd）。

## 模型 {#models}
在{{ui:settings.nav.models}}标签页的{{ui:settings.phaseModels.sectionTitle}}中，为整台电脑设置各工作阶段使用的模型。阶段包括{{ui:settings.phaseModels.phase.planning}}、{{ui:settings.phaseModels.phase.execution}}、{{ui:settings.phaseModels.phase.review}}和{{ui:settings.phaseModels.phase.subagents}}。
- 保持{{ui:settings.phaseModels.defaultOption}}时，由 CLI 决定。
- {{ui:settings.phaseModels.paneModel}}会使用窗格模型按钮中选择的模型。
- 列表中没有的模型名称，可以在{{ui:settings.phaseModels.registeredTitle}}中{{ui:settings.phaseModels.addButton}}。如果该模型支持思考强度，请打开{{ui:settings.phaseModels.supportsEffortLabel}}。

![[settings-models]]

## 风格 {#styles}
在{{ui:settings.nav.styles}}标签页中管理风格列表：从文件注册风格、检查内容后允许或阻止。列表不对时，请点击{{ui:settings.styles.rescanButton}}按钮。把指针放在风格名称上，可以看到它的 ID、哈希和数量。详情请参阅[风格](styles.md#custom)。

![[settings-styles]]

## 远程连接 {#remote}
在{{ui:settings.remote.navTitle}}标签页中，让其他电脑（Mac、Windows）或手机使用这台电脑的工作区，或打开其他电脑的工作区。顶部的说明下方有三个标签页。
- {{ui:settings.remote.shareTitle}}：用顶部状态行旁的{{ui:settings.remote.startSharing}} / {{ui:settings.remote.stopSharing}}按钮开关共享。选择{{ui:settings.remote.pickWorkspaces}}，并在下方的{{ui:settings.remote.methodsTitle}}中查看{{ui:settings.remote.viaPairing}}和{{ui:settings.remote.viaTailscale}}两行。请参阅[共享这台电脑](remote.md#share)。
- {{ui:settings.remote.connectTitle}}：用配对链接或 Tailscale 地址连接其他电脑，并在下方的{{ui:settings.remote.connectionsTitle}}列表中管理连接。请参阅[连接其他电脑](remote.md#connect)。
- {{ui:settings.remote.phoneTab}}：连接手机。请参阅下方的[手机](#mobile)。

{{platform:windows}} {{ui:settings.remote.shareTitle}}和{{ui:settings.remote.phoneTab}}标签页下方有{{ui:settings.remote.firewallTitle}}。请参阅[Windows 防火墙](remote.md#firewall)。

### 手机 {#mobile}
在{{ui:settings.remote.navTitle}} › {{ui:settings.remote.phoneTab}}标签页中允许手机连接、用二维码连接手机，并管理已连接的设备。不需要另外准备中继。请参阅[手机远程](phone.md#pair)。折叠的{{ui:settings.advancedTitle}}中有使用自建中继时的{{ui:settings.mobileRemote.relayLabel}}栏（带{{ui:settings.mobileRemote.applyButton}}按钮，下方显示默认地址）和{{ui:settings.mobileRemote.legacyAppsToggle}}开关。只要其中任一项改过默认值，它一开始就是展开的。

屏幕查看与控制目前暂时停用，因此设置中没有这一项（[远程屏幕](phone.md#screen)）。

## Mighty Pet 与提醒 {#companion}
在{{ui:settings.nav.companion}}标签页中设置 Mighty Pet 和完成通知。
- {{ui:companion.settings.enabled}}：显示在屏幕上走动的浣熊 Mighty Pet。可以拖动 Mighty Pet 移动位置，点击它可以打开或关闭气泡。
- {{ui:companion.settings.task}}：在气泡中显示请求、当前任务和用时。也可以在气泡中回答批准请求（{{ui:permission.allowOnce}} / {{ui:permission.deny}}）。批准、提问和计划气泡上的图标按钮会打开对应的窗格，以便查看完整请求或自己输入答案。
- 气泡上方附有该窗格最近一个请求的{{ui:graph.progress.title}}卡片，内容与窗格的进度面板相同：请求编号、已完成的步骤数、状态、所有步骤以及后台任务，各自附带用时。超过八行时可在卡片内滚动查看，打开时先显示当前步骤。点击卡片标题可折叠为只剩标题，再次点击即可展开；折叠状态在下次打开应用时依然保留。点击步骤行会打开对应的窗格。没有步骤也没有后台任务的请求、气泡显示批准、提问或计划时，以及气泡关闭时，不显示卡片。
- {{ui:companion.settings.pet}}：选择宠物。可以用{{ui:companion.settings.import}}添加其他宠物，用{{ui:companion.settings.reload}}重新读取宠物列表，用{{ui:companion.settings.resetPosition}}把 Mighty Pet 位置和气泡大小恢复为初始状态。
- {{ui:companion.settings.motion}}：减少动作。
- {{ui:companion.settings.macNotifications}}（Windows 上为{{ui:windows.notifications.toggleLabel}}）：请求成功完成时发送 Mac 通知。点击通知会打开对应的窗格。通知中不包含请求或结果的内容。
- {{ui:terminalNotifications.toggle}}：在窗格上标记其终端或 CLI 发送的通知与响铃。仅当应用在后台或你正在看其他窗格时才发送系统通知。默认开启（[通知与响铃](terminal-files-browser.md#terminal-signals)）。
- 下方显示操作系统的通知权限状态，{{ui:windows.notifications.settingsButton}}按钮会打开系统的通知设置。

![[settings-companion]]

![[companion-pet]]

## 组件 {#components}
{{ui:settings.nav.tools}}标签页显示应用所需的内容是否已准备就绪，智能体 CLI 的安装、登录和更新也都在这里管理。只显示适用于这台电脑系统的项目。

最上方一行是显示切换和两个按钮。
- {{ui:settings.components.filterAll}} / 需处理：选择需处理后，只显示需要处理的行，即未安装、未登录、有可用更新、失败或需要处理的项目。旁边的数字是这类项目的数量。
- 刷新（↻，{{ui:setup.button.recheck}}）：重新读取 PATH，并重新检查所有项目。
- {{ui:settings.components.updateAllButton}}：立即更新所有已安装的智能体 CLI，每个 CLI 都沿用最初的安装方式（npm、Homebrew 等）。

列表分为三组：{{ui:setup.group.agent}}（Claude、Codex、Antigravity、pi、opencode、Hermes）、{{ui:settings.components.groupTools}}（Git（Mac 上为 Xcode 命令行工具）、Homebrew（Mac）、Node.js、Bun（Mac）、uv、Tailscale、gh、glab）和{{ui:settings.components.groupPlugins}}（ouroboros、omc、gstack（Mac））。每个项目占一行，行内有展开标记（▸）、名称、版本，智能体 CLI 还有已登录的账户。右侧是状态，以及当前最需要的一个按钮。
- 状态：{{ui:setup.status.installed}}、{{ui:settings.components.statusInstalled}}（智能体 CLI 已安装并已登录）、{{ui:settings.components.statusUpdateAvailable}}、{{ui:settings.cliAccounts.summarySignedOut}}、{{ui:settings.cliAccounts.statusNotInstalled}}、{{ui:setup.status.attention}}、{{ui:setup.status.installing}}、{{ui:setup.status.failed}}。
- 按钮：缺少时为{{ui:settings.components.installButton}}（只能通过下载页面安装的项目为{{ui:setup.button.openPage}}），未登录时为{{ui:settings.cliAccounts.buttonLogin}}，过旧时为{{ui:setup.button.update}}。没有问题时不显示按钮。

点击某一行会就地展开（▾）。展开后显示详细信息（需要哪个版本、上次更新的结果、账户信息、需要处理的原因、该项目的用途），以及该项目的所有按钮，智能体 CLI 有{{ui:settings.cliAccounts.buttonChange}}、{{ui:settings.cliAccounts.buttonLogout}}、{{ui:settings.cliAccounts.refreshTooltip}}等。再次点击即可收起。展开过的行会一直保持展开，直到退出应用。

列表下方的{{ui:settings.components.groupAutoUpdate}}框中有三个开关和上次运行的时间。把指针移到开关上即可看到说明。
- {{ui:settings.components.autoUpdateClis}}：在打开应用时以及每 6 小时检查一次智能体 CLI 更新。
- {{ui:settings.components.autoUpdatePlugins}}：同时更新 Claude 和 Codex 插件。
- {{ui:settings.components.autoLogin}}：登录过期时重新登录，并重新发送失败的请求。

点击{{ui:settings.components.installButton}}，只会安装该项目以及它缺少的前置工具。所有要运行的命令会先在一个确认框中全部列出。
- {{platform:mac}} 在应用内的终端窗格中运行；没有打开本地工作区时，在终端（Terminal.app）中运行。
- {{platform:windows}} 在单独的 PowerShell 窗口中运行。

是否安装成功，由完成后的重新检查来判断。首次运行时打开的[{{ui:setup.title}}](getting-started.md#setup)窗口是一个清单：选择缺少的项目并点击{{ui:setup.button.install}}。

同一标签页的最下方是{{ui:settings.toolkit.sectionTitle}}框，用来自己收集常用工具，一开始是空的。用{{ui:settings.toolkit.addButton}}添加工具，每一项都有{{ui:settings.toolkit.approveButton}}和{{ui:settings.toolkit.removeButton}}按钮。只有批准过的工具才能安装；安装前会显示所有要运行的命令，并请你{{ui:settings.toolkit.confirmTitle}}。可以{{ui:settings.toolkit.exportButton}}或{{ui:settings.toolkit.importButton}}该列表。

![[settings-tools]]

### 登录 {#cli-accounts}
在每个智能体 CLI 的那一行登录。未登录的 CLI 会在行内直接显示{{ui:settings.cliAccounts.buttonLogin}}按钮，其他账户按钮在展开该行后显示。
1. 点击{{ui:settings.cliAccounts.buttonLogin}}。Claude 需要在{{ui:settings.cliAccounts.buttonLoginClaude}}和{{ui:settings.cliAccounts.buttonLoginConsole}}之间选择。
2. CLI 登录会在终端中开始。请在浏览器中完成授权。
   - {{platform:mac}} 应用内会打开终端窗格。它在本地工作区中打开，因此需要至少打开一个工作区。
   - {{platform:windows}} 会打开单独的控制台窗口，不需要工作区。
3. 登录完成后，该行的状态会随之更新。如果没有更新，请展开该行并点击{{ui:settings.cliAccounts.refreshTooltip}}。更换后的账户从下一个请求起生效。

要更换账户，请展开该行并点击{{ui:settings.cliAccounts.buttonChange}}；要退出登录，请点击{{ui:settings.cliAccounts.buttonLogout}}。打开{{ui:settings.components.groupAutoUpdate}}框中的{{ui:settings.components.autoLogin}}后，登录断开时会自动重新登录，并重新发送失败的请求。

展开 Claude 一行后，还有{{ui:settings.cliAccounts.resetModelsButton}}和{{ui:settings.cliAccounts.bedrockButton}}按钮。{{ui:settings.cliAccounts.bedrockButton}}以与登录相同的方式在终端中打开 Claude CLI 的 AWS Bedrock 设置向导，完成后请刷新状态。{{platform:windows}} Windows 上还有一个{{ui:settings.cliAccounts.resetBedrockButton}}按钮，会先备份 Claude 用户设置，再删除 Bedrock 密钥和 Bedrock 模型设置。

### 更新 {#cli-update}
智能体 CLI 与应用分开更新。
- {{ui:settings.components.updateAllButton}}按钮：立即更新所有已安装的智能体 CLI。过旧 CLI 一行中的{{ui:setup.button.update}}按钮也会运行 CLI 更新。
- {{ui:settings.components.groupAutoUpdate}}框中的{{ui:settings.components.autoUpdateClis}}：在打开应用时以及每 6 小时检查一次。
- {{ui:settings.components.autoUpdatePlugins}}：同时更新 Claude 和 Codex 插件。

更新时沿用 CLI 最初的安装方式（npm、Homebrew 等）。每个 CLI 上次更新的结果在展开该 CLI 一行后显示，插件更新的结果显示在{{ui:settings.components.groupPlugins}}组下方。

## 关于 {#about}
在{{ui:settings.nav.about}}标签页中可以更新应用，并查看数据存储位置。
- {{ui:settings.appUpdate.sectionTitle}}：包括当前版本、自动更新开关、状态和按钮以及发行说明。请参阅[更新](getting-started.md#update)。折叠的{{ui:settings.advancedTitle}}中有{{ui:settings.appUpdate.manifestUrlAddressLabel}}输入框和签名说明；填写过地址时会自动展开。
- {{ui:settings.appInfo.sectionTitle}}：有{{ui:settings.appInfo.openHelpButton}}按钮和{{ui:settings.appInfo.stateLocationLabel}}。{{ui:settings.appInfo.stateLocationLabel}}是存放应用设置和记录的文件夹，在 Mac 上用{{ui:menu.showInFinder}}按钮打开，在 Windows 上用{{ui:menu.showInExplorer}}按钮打开。

![[settings-about]]
