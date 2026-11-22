---
title: 界面布局
order: 2
section: layout
---
窗口分为左侧的侧边栏和右侧的主区域。在主区域中，可以把窗格叠放为标签页，也可以左右或上下分割排列。

![[overview]]

## 侧边栏 {#sidebar}
侧边栏从上到下依次是：
- {{ui:sidebar.searchPlaceholder}}：按{{kbd:⌘K}}可直接跳到搜索框。
- {{ui:phone.dashboard.title}}：在一处查看所有工作区中窗格的状态。
- {{ui:review.sidebar}}：集中查看有待审阅更改的窗格和请求（[审阅队列](#review)）。
- {{ui:sidebar.workspacesHeader}}列表：点击工作区即可切换到该工作区。点击旁边的箭头可以展开或收起其窗格列表。
- 底部：帮助按钮、主题按钮（深色、浅色）和设置按钮。

展开工作区后，最后一行是{{ui:workspace.addPane}}按钮。右键点击窗格所在的行，可以看到{{ui:menu.rename}}和{{ui:menu.closePane}}。

### 调整侧边栏宽度 {#sidebar-resize}
拖动侧边栏与主区域之间的边界线，即可调整侧边栏的宽度（{{ui:sidebar.resize}}）。
- 指针移到边界线上时，它会变为强调色。左右拖动即可调整宽度。宽度只在限定范围内变化，重新打开应用后仍会保留。
- 双击边界线可恢复默认宽度。
- 用 Tab 键选中边界线后按{{kbd:←}} / {{kbd:→}}，每次变窄或变宽 10 px。
- 将边界线一直拖到最左侧会收起侧边栏。再次展开时，会恢复拖动前的宽度。

### 收起侧边栏 {#sidebar-collapse}
想让主区域更宽时，可以收起侧边栏。
1. 点击主区域标题栏左上角的侧边栏按钮，或按{{kbd:⌃⌘S}}。{{ui:phone.dashboard.title}}页面和首页的同一位置也有这个按钮。菜单栏的“显示”菜单中也有{{ui:sidebar.collapse}}。
2. 再次展开时，同样使用该按钮或{{kbd:⌃⌘S}}（{{ui:sidebar.expand}}）。

收起状态在重新打开应用后仍会保留。在收起状态下按{{kbd:⌘K}}，侧边栏会展开并跳到搜索框。

![[sidebar-collapsed]]

## 工作状态 {#dashboard}
点击侧边栏搜索框下方的{{ui:phone.dashboard.title}}，可以按状态汇总查看所有工作区的窗格。
- 顶部的格子显示{{ui:phone.dashboard.stat.running}}、{{ui:phone.dashboard.stat.waiting}}和{{ui:phone.dashboard.stat.done}}的数量。
- {{ui:dashboard.usage.title}}卡片显示账户用量。此限额与使用同一账户的其他应用和会话共享。
- 每个工作区都有{{ui:files.pane.title}}按钮和{{ui:workspace.addPane}}菜单。
- 点击卡片即可转到该窗格。选择工作区或窗格后，会离开工作状态页面。

![[dashboard]]

## 审阅队列 {#review}
把所有待审阅的内容集中在一处。点击侧边栏{{ui:phone.dashboard.title}}下方的{{ui:review.sidebar}}，或按 {{kbd:⇧⌘R}}(Windows：{{kbd:Ctrl+Shift+R}})，即可打开{{ui:review.open}}面板。条目旁的数字是等待中的数量。
- 列出的内容：留下更改后结束的[worktree 窗格](delegation.md#pane-worktree)和[子窗格](delegation.md#states)，以及每个窗格最近结束、且保存了所改文件的请求([审阅请求更改](agent-pane.md#request-changes))。仍在工作的窗格要等结束后才会出现。 所有文件都以这种方式审阅完的请求会被移除。
- 每行显示窗格名称、代理、类型、工作区、分支及其基础分支、更改的文件数以及增删的行数，还有状态({{ui:review.status.ready}}、{{ui:review.status.conflict}}、{{ui:review.status.merged}})。冲突排在最上，已合并的排在最下。
- 按钮：{{ui:review.action.changes}}、{{ui:review.action.merge}}(有未提交文件的 worktree 窗格为{{ui:review.action.commitMerge}})、{{ui:editor.open}}、{{ui:review.action.pr}}(worktree 窗格)、{{ui:review.action.discard}}、{{ui:review.action.focus}}、{{ui:review.action.dismiss}}。合并、丢弃和 PR 与窗格栏或子窗格卡片上的操作相同，会经过同样的确认。请求的{{ui:review.action.changes}}会打开请求更改审阅。
- 点击{{ui:review.action.dismiss}}后，该项会隐藏，直到其工作再次变化。
- 列表在运行结束、打开面板和点击{{ui:review.refresh}}时重新读取 git，不会在每次重绘时读取。
- worktree 窗格和子窗格的行会附上合并前检查摘要({{ui:review.checks.ok}}或警告数量)。点击即可打开该窗格的{{ui:git.checks.tab}}标签页([检查](#checks))。摘要来自上次读取的检查结果和应用已知的信息(未提交的文件、剩余待办、窗格的 PR 状态)，列表本身不会另外调用 gh。

## 提交与 PR {#git}
在一个面板中完成工作区的 git 操作。可从侧边栏工作区菜单中的{{ui:git.workbench.open}}、工作区标题中的分支显示，或输入框中的 `/git` 打开。从 worktree 窗格打开时，它在该窗格的 worktree 中工作。
- {{ui:git.checks.changesTab}}标签页：更改的文件及其 diff、按文件或代码块暂存、切换分支、草拟提交信息、提交、push、创建 PR 以及请求修复失败的 CI。不会从默认分支创建 PR。
- {{ui:git.checks.tab}}标签页：把合并前需要确认的内容集中在一个画面([检查](#checks))。

### 检查 {#checks}
针对 worktree 窗格(其他窗格为当前分支)显示五项内容。
- {{ui:git.checks.git.title}}：未提交的更改数，以及比基础分支领先(↑)、落后(↓)的提交数。
- {{ui:git.checks.pr.title}}：PR(GitLab 为 MR)编号、状态、是否为草稿、冲突、审查结果(已批准、请求更改)。
- {{ui:git.checks.ci.title}}：每项检查的结果。有失败的检查时，可用{{ui:git.pr.fixChecks}}按钮连同失败日志交给代理处理。
- {{ui:git.checks.threads.title}}：未解决审查讨论的文件和行、作者、第一条评论、回复数和链接。
- {{ui:git.checks.checklist.title}}：该窗格最近一次请求的清单中尚未完成的项目。

- 打开标签页时以及打开期间每 30 秒重新读取，关闭时不读取。
- 各项分别读取，因此没有 gh 或 glab、未登录或没有 PR 时，只有该项显示{{ui:git.checks.unknown}}。
- 选中讨论后点击{{ui:git.checks.threads.toComposer}}按钮，会在“请处理 PR #42 的审查意见。”下方把 `文件:行 (作者): 评论` 列表放入该窗格的输入框，不会立即发送。看起来像密钥的行会被隐藏，链接中的账户信息会被去掉，最多放入 2,000 个字符。
- 合并前确认：只要有失败或进行中的检查、未解决的讨论、剩余待办、未提交的更改(“提交并合并”除外)、草稿、冲突或请求更改中的任何一项，worktree 栏、子窗格卡片和审阅队列上的合并按钮就会显示原因并再确认一次。点击{{ui:git.checks.merge.confirm}}即照常合并，不会阻止。

## 添加窗格 {#add-pane}
窗格可以是 AI 智能体（Claude、Codex、Antigravity、pi、opencode、Hermes）、终端或文件中的一种。

1. 点击侧边栏中的{{ui:workspace.addPane}}按钮。
2. 选择要创建的窗格：智能体（Claude、Codex、Antigravity、pi、opencode、Hermes）窗格、{{ui:workspace.newTerminal}}或{{ui:menu.showFiles}}（{{kbd:⇧⌘E}}）。菜单最下方一组是添加工作区的三种方式：{{ui:workspace.add.existing}}、{{ui:workspace.add.newFolder}}和{{ui:workspace.add.fromGit}}（[打开工作区](getting-started.md#open-workspace)）。
3. 新的 AI 窗格会沿用同一智能体最近使用过的窗格的模型、思考强度、权限和视图。对话则从头开始。

快捷键：{{ui:menu.newClaudePane}}为{{kbd:⌘N}}，{{ui:menu.addTerminalPane}}为{{kbd:⌘T}}，{{ui:menu.showFiles}}为{{kbd:⇧⌘E}}。所有工作区合计最多可以打开 128 个窗格（原样显示其他电脑窗格的远程窗格不计入）。

> [!note]
> 除 Claude 以外的智能体窗格（Codex、Antigravity、pi、opencode、Hermes）带有{{ui:badge.beta}}标记。

### 继续之前的会话 {#resume}
添加智能体窗格时，如果此文件夹中有之前的会话，会弹出选择窗口。Antigravity 会列出此文件夹中的对话，子代理运行的对话不会列出。
1. 要开始新对话，请点击{{ui:resume.choice.startNew}}；要继续之前的对话，请点击{{ui:resume.choice.resume}}按钮。
2. 在{{ui:resume.title}}列表中选择会话。可以用{{ui:resume.search}}查找。
3. 新窗格会继续该会话。

![[resume-choice]]

![[resume-list]]

### 搜索过去的对话 {#conversation-search}
{{ui:conversationSearch.open}}（{{kbd:⇧⌘F}}）会在此文件夹过去会话的请求和回答中查找词语。也可以从工作区标题栏的放大镜按钮、在侧边栏右键点击工作区的菜单，以及菜单栏的工作区菜单打开。
- 列出某条消息包含所有词的会话，按时间从新到旧，最多 50 个，找到的词会被标出。不搜索工具输出。
- 搜索 Claude、Codex、Antigravity 和 pi 的会话，不搜索 opencode 和 Hermes 的会话。已在窗格中打开的会话和超过 60 天的会话不包括在内。
- 用 {{kbd:↑}} {{kbd:↓}} 选择后按 {{kbd:Enter}}，或点击{{ui:conversationSearch.resume}}，新窗格会继续该会话。
- {{ui:conversationSearch.export}}会把该对话的请求和回答保存为 Markdown 文件。
- 远程工作区中不可用，因为会话记录在那台电脑上。

## 标签页与分割 {#tabs-splits}
- 将标签页拖到另一组的中间，即可合并为标签页（{{ui:layout.drop.merge}}）。
- 放到某组的左、右、上、下边缘，就会向该方向分割。
- 拖动分割线可以调整大小，双击分割线则平分两侧。
- 拖动过程中按{{kbd:Esc}}即可取消。
- 双击标签页可以重命名。右键点击标签页，可以看到{{ui:menu.rename}}、{{ui:menu.focusPane}}和{{ui:menu.closeTab}}。

布局按工作区分别保存。切换工作区后，各自的布局保持不变。

### 窗格菜单 {#pane-menu}
窗格标题栏中的 … 按钮包含以下项目：
- {{ui:menu.rename}}
- {{ui:menu.focusPane}}：只放大显示此窗格。再次点击即可{{ui:pane.menu.restoreLayout}}。
- {{ui:pane.menu.copyLog}}：复制智能体窗格的运行记录。终端窗格在这个位置是{{ui:pane.menu.terminalHistory}}（[终端窗格](terminal-files-browser.md#terminal)）。
- {{ui:pane.menu.newConversation}}：从下一个请求起不再接续之前的对话（智能体窗格）。
- {{ui:menu.closePane}}

右键点击窗格标题栏，会有{{ui:menu.rename}}、{{ui:session.menu.focus}}和{{ui:session.menu.close}}，终端窗格还有{{ui:terminal.history.title}}。

## 快捷键 {#shortcuts}
| 快捷键 | 操作 |
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
| {{kbd:Enter}} | 发送请求。在运行中的窗格里，按{{ui:composer.busySend.title}}的选择放入队列，或立即在所选代理的新 worktree 中运行（[运行时继续提出请求](agent-pane.md#queue)） |
| {{kbd:⇧Enter}} | 换行 |
| {{kbd:⌘Enter}} | 在运行中的窗格里立即在新的 worktree 中运行（开启了[Mighty 编排](delegation.md#worktree)、位于 git 仓库中的窗格）；在其他运行中的窗格里放入队列 |
| {{kbd:Esc}} | 关闭弹出面板、关闭斜杠命令列表 |
| {{kbd:⌘=}} / {{kbd:⌘-}} / {{kbd:⌘0}} | {{ui:menu.textBigger}} / {{ui:menu.textSmaller}} / {{ui:menu.textActualSize}} |
| {{kbd:⌘?}} | {{ui:menu.help}} |
