---
title: Mighty 编排
order: 6
section: delegation
---
借助 Mighty 编排，一个代理窗格在工作的同时，可以把别的工作交给旁边的子窗格一起推进。子窗格运行与父窗格相同的代理（Claude、Codex 或 Antigravity）；也可以运行三者中的另一个（[Codex 和 Antigravity](#agents)）。每个子窗格都在自己的 git worktree 和分支中工作，因此不会动到彼此的文件；完成后，在侧边栏的卡片上把结果合并到父窗格的分支。此功能以前叫作智能体委派。

打开子窗格有两种方法。

- **亲自启动**：在运行中的 Claude 窗格，或处于子窗格能更改文件的模式的 Codex、Antigravity 窗格（[Codex 和 Antigravity](#agents)）里写下下一个请求，按 {{kbd:⌘Enter}}。请求会立即在新 worktree 的子窗格中运行（[在新的 worktree 中运行](#worktree)）。
- **让 Claude 分工**：请 Claude（或 Codex）把工作拆开交给子窗格。代理会创建子窗格、阅读报告，并一直做到合并（[让 Claude 分工](#delegate)）。

> [!warning]
> 这是实验性功能。默认开启，可以在设置中关闭。请先在练习用的仓库中试用。

## 概览 {#overview}
亲自启动时的流程如下。

1. 把一个 git 仓库文件夹作为工作区打开，在 Claude 窗格中交给它一项工作。
2. Claude 工作期间，写下下一个请求并按 {{kbd:⌘Enter}}。正在运行的任务继续进行，这个窗格旁边会打开一个子窗格，立即运行你的请求。
3. 子窗格在侧边栏中缩进一格列在父窗格下方，旁边显示它的状态。
4. 子窗格完成工作、提交并留下报告后，状态变为{{ui:delegation.child.state.reported}}。
5. 点击子窗格卡片上的{{ui:delegation.card.merge}}按钮，把结果放进父窗格的分支。
6. 合并后，若 worktree 中没有未提交的文件，会立即清理子窗格及其 worktree 和分支；若有剩余文件，会列出这些文件，让你选择{{ui:worktree.mergedCleanup.confirm}}或{{ui:worktree.mergedCleanup.keep}}。已合并的内容保留在父窗格的分支中。

![[delegation-tree]]

| 想做的事 | 方法 |
|---|---|
| 在当前任务之后接着运行 | {{kbd:Enter}}（{{ui:composer.busySend.title}}为{{ui:composer.busySend.queue}}时，默认） |
| 与当前任务分开，立即运行 | {{kbd:⌘Enter}} 或{{ui:queue.worktree}}按钮，运行时在{{ui:composer.busySend.title}}中选好代理后按 {{kbd:Enter}}，或队列行的{{ui:queue.runNow}}按钮 |
| 把结果放进父窗格的分支 | 卡片上的{{ui:delegation.card.merge}}按钮 |
| 撤销合并 | 卡片上的{{ui:delegation.card.undo}}按钮 |
| 丢弃结果 | 卡片上的{{ui:delegation.card.discard}}按钮 |

## 在新的 worktree 中运行 {#worktree}
在 Claude、Codex 或 Antigravity 窗格运行时写下请求，按 {{kbd:⌘Enter}}，或点击停止按钮旁的圆形{{ui:queue.worktree}}按钮（分支图标）。正在运行的任务不受打扰，你的请求会立即在拥有自己 worktree 的新子窗格中运行。除非你在此期间已经开始在父窗格中输入，新窗格会立即显示出来。

- 在运行中的窗格里，{{kbd:Enter}} 默认会把请求放入队列，等当前任务结束后再运行。如果在{{ui:composer.busySend.title}}中选好了代理，{{kbd:Enter}} 也会立即在该代理的新 worktree 中运行（见下文）。输入框的提示文字会说明 {{kbd:Enter}} 现在的作用。
- 只有在能创建 worktree 的地方才会使用新的 worktree：必须是检出了有提交的分支的 git 仓库。在其他地方（不是 git 仓库、还没有提交、检出的是提交而不是分支），{{kbd:⌘Enter}} 会像 {{kbd:Enter}} 一样把请求放入队列。
- 子窗格从父窗格的最后一次提交开始。会创建分支 `mighty/<子窗格 id>`，worktree 位于项目文件夹之外（Mac 上为 `~/.mightycode/worktrees/<子窗格 id>`，Windows 上为 `%USERPROFILE%\.mightycode\worktrees\<子窗格 id>`），不会动到父窗格的工作。父窗格文件夹中尚未提交的更改不会带给子窗格。
- 与窗格相同代理的子窗格沿用父窗格的权限模式、模型、{{ui:composer.effort.label}}和其他运行设置（快速模式、网页搜索、最大轮数、预算）运行。在每个请求都以计划模式开始的样式中，它以计划模式开始。它以与父窗格相同的视图打开；如果父窗格在 Mighty 视图中使用某个样式，它也以同一样式开始（样式的进度从子窗格的第一个请求起单独计算）。附件也会一起带上。
- 第一条消息先是你输入的文字，随后应用会告诉子窗格在哪里工作，并要求它完成后提交并写入 REPORT.md，这样才能在卡片上合并。斜杠命令（以 `/` 开头的文字）会按你输入的样子单独运行，应用的说明则作为子窗格的下一个请求随后发送，因此不会变成命令的参数。
- 若要用另一个代理运行请求，请在窗格运行时，在输入框底部一行的{{ui:composer.busySend.title}}中选择 Claude、Codex 或 Antigravity（例如“worktree · Codex”），再按 {{kbd:Enter}} 发送。只有附件、没有文字的请求会放入队列。{{kbd:⌘Enter}}（Windows 上为 {{kbd:Ctrl+Enter}}）和{{ui:queue.worktree}}按钮始终使用窗格自己的代理。灰色的项目会说明无法运行的原因：其 CLI 未安装或未登录（请在{{ui:menu.settings}} › {{ui:settings.nav.tools}}中查看）、没有能更改文件的模式，或你启动的 3 个子窗格已经打开。另一个代理的子窗格像新增窗格时一样，沿用该代理最近使用的窗格的权限模式、模型、{{ui:composer.effort.label}}和其他运行设置（该模式不能更改文件时按下面的规则）。若还没有该代理的窗格，则以它自己的模式中能更改文件、且不做窗格模式不允许之事的最宽模式开始（[Codex 和 Antigravity](#agents)）。它以窗格的视图打开，但不带样式。如果无法创建或启动该子窗格，你的文字和附件会放回输入框，对话中会说明原因；它们绝不会发给窗格本身。如果窗格自己的代理在当前模式下不能有子窗格，而另一个代理可以，则不显示{{ui:queue.worktree}}按钮，{{ui:composer.busySend.title}}中只能选择其他代理，{{kbd:⌘Enter}} 会把请求放入队列。
- 已经在队列中的请求，也可以用该行的{{ui:queue.runNow}}按钮以同样的方式送出。在按钮前的代理选择中选择代理（默认是窗格自己的代理）。若无法创建或启动它的子窗格，请求会回到队列最前面，对话中会写明原因。
- 子窗格的用法和同一代理的普通窗格一样：可以回答批准卡片、发送后续请求、更改权限模式。
- 对于你亲自启动的子窗格，父窗格中的 Claude 不会收到通知，因为这不是父窗格交出的工作。父窗格的工具也不会列出它，也不能合并它或向它发送后续指示（`human_started`）。请在侧边栏中查看它的状态，并亲自使用卡片上的按钮。
- 一个父窗格最多可以有 3 个打开的子窗格，并且需要 10 GB 可用磁盘空间。关闭或丢弃已完成的子窗格就能腾出名额。
- 无法创建子窗格或创建后无法启动时（已有 3 个打开的子窗格、磁盘空间不足、worktree 或第一次运行失败等），窗格的对话中会写明原因。随后，你的文字会像按 {{kbd:Enter}} 那样成为该窗格的下一个请求。队列已满时会放回输入框。启动失败的子窗格会一直列出，直到你在卡片上丢弃它。
- 在子窗格中，或关闭 Mighty 编排的窗格中，{{kbd:⌘Enter}} 也会像 {{kbd:Enter}} 一样放入队列。从手机发给运行中窗格的内容同样总是进入队列。人发送的内容不会插入正在运行的任务。

## 窗格使用自己的 worktree {#pane-worktree}
为了让同一工作区的多个代理窗格同时工作,工作区的代理窗格也可以像子窗格一样在自己的 git worktree 和分支中工作。各窗格不会覆盖彼此的文件,完成后在窗格上方的栏中合并到工作区分支。

- **何时在 worktree 中打开**:当同一工作区已有在工作区文件夹中工作的代理窗格时,新代理窗格会询问:{{ui:paneWorktree.choice.worktree}}、{{ui:paneWorktree.choice.always}} 或 {{ui:paneWorktree.choice.sameFolder}}。也可以在侧边栏工作区行的右键菜单 {{ui:paneWorktree.menu.mode}} 中预先设置为 {{ui:paneWorktree.mode.ask}}、{{ui:paneWorktree.mode.further}} 或 {{ui:paneWorktree.mode.every}}。设为 {{ui:paneWorktree.mode.every}} 时第一个窗格也在 worktree 中工作,工作区文件夹只接收合并(否则在工作区文件夹中工作的窗格会阻止合并)。从手机打开的窗格遵循此设置,设置为询问时在工作区文件夹中打开。继续之前会话的窗格总是在工作区文件夹中工作。
- **创建方式**:与子窗格规则相同。从工作区分支的最新提交创建分支 `mighty/<窗格 id>`,worktree 位于 `~/.mightycode/worktrees/<窗格 id>`。无法创建时(例如不是 git 仓库)会说明原因,只能在同一文件夹中打开。
- **SSH 工作区**:worktree 建在该主机的 `~/.mightycode/worktrees/<窗格 id>`,合并、提交、撤销、丢弃以及复制和准备命令也在该主机上按相同规则进行([SSH 主机](remote.md#ssh))。
- **告知代理**:窗格的第一个请求会附上应用的说明,让代理知道自己的 worktree、分支和 PORT,并在完成后提交。合并由你在应用中进行。
- **栏**:窗格上方显示分支和基础分支、基础分支没有的提交数(↑)、之后基础分支前进的提交数(↓)、未提交的更改文件数(±,点击查看)和 PORT(点击打开 localhost)。有未提交的更改时按钮变为 {{ui:paneWorktree.bar.commitMerge}},会连同新文件一起提交后合并。看起来像密钥的新文件或超过 5 MB 的新文件不会提交,并会提示。从栏中合并完成后，若没有未提交的文件，会删除 worktree 和分支并关闭窗格；若有剩余文件，会连同列表询问是否一起删除（在手机上合并时只在没有剩余文件时清理）。
- **清理残留的 worktree**：应用启动后不久，若发现没有任何窗格或记录在使用的 worktree 文件夹或 `mighty/*` 分支，会列出它们（标出未合并的提交和未提交的文件），询问是否{{ui:worktree.leftovers.confirm}}。
- **冲突**:合并因冲突被拒绝时会出现 {{ui:paneWorktree.bar.resolve}} 按钮,点击后请代理合入基础分支、解决冲突并提交。
- **栏的菜单**:{{ui:paneWorktree.bar.changes}}(相对基础分支更改的文件及差异)、{{ui:paneWorktree.bar.terminal}}、{{ui:paneWorktree.bar.finder}} (Windows: {{ui:menu.showInExplorer}})、撤销最近一次合并(仅当工作区文件夹此后没有变化时)、{{ui:paneWorktree.bar.pr}}(确认后将分支推送到 origin,从不强制;如果是 GitHub 则打开创建拉取请求的页面)、{{ui:paneWorktree.bar.discard}}(有子窗格基于此窗格的分支时会拒绝)。
- **准备**:工作区行菜单中 {{ui:paneWorktree.menu.copy}} 列出的文件(如 `.env`)会复制到新 worktree,然后运行一次 {{ui:paneWorktree.menu.setup}}。窗格的运行会获得 `PORT`、`MIGHTY_WORKSPACE_FOLDER` 和 `MIGHTY_WORKTREE` 环境变量，以及[端口范围](#mighty-yaml)的变量。
- **关闭与列表**:没有未合并工作的窗格关闭时会一并删除其 worktree。仍有工作的会保留,可在工作区行菜单的 {{ui:paneWorktree.menu.list}} 中重新作为窗格打开、合并或删除。
- **审阅队列**：工作结束且留下更改的 worktree 窗格和子窗格会汇集到[{{ui:review.open}}](layout.md#review)。栏菜单中的{{ui:editor.openFolder}}会在外部编辑器中打开窗格工作的文件夹。

## 从 Issue 开始 {#issues}
选择 GitHub Issue 或 Linear、Jira、GitLab Issue，直接开始工作。在侧边栏工作区的右键菜单中用 {{ui:integrations.menu.browse}} 打开列表。

- **连接**：在同一菜单的 {{ui:integrations.menu.settings}} 中进行。GitHub 使用 `gh`。在 {{ui:integrations.tracker.section}} 中选择 Linear、Jira 或 GitLab，并打开 {{ui:integrations.tracker.enabled}}。
  - Linear：{{ui:integrations.tracker.linearToken}} 和 {{ui:integrations.tracker.linearTeam}}(留空为所有团队)。
  - Jira Cloud：{{ui:integrations.tracker.jiraSite}}(`https://团队.atlassian.net`)、{{ui:integrations.tracker.email}}、{{ui:integrations.tracker.jiraToken}}、{{ui:integrations.tracker.jiraProject}}。
  - GitLab：已安装并登录 `glab` 时使用它，否则填入 {{ui:integrations.tracker.gitlabToken}}。{{ui:integrations.tracker.gitlabProject}} 和地址留空时从 `origin` 读取。
  - 令牌只保存在操作系统的钥匙串(Mac 钥匙串、Windows 凭据管理器)中，不会出现在界面、日志或状态文件里。用 {{ui:integrations.tracker.test}} 检查，用 {{ui:integrations.auth.logout}} 删除。
- **列表**：每个启用的跟踪器都有一个标签页。用 {{ui:integrations.filter.mine}}、{{ui:integrations.filter.open}}、{{ui:integrations.filter.all}} 筛选，首先选中的筛选项取决于 {{ui:integrations.tracker.filter}}({{ui:integrations.tracker.filterMine}}、{{ui:integrations.tracker.filterOpen}})。列表只在打开和刷新时读取。
- **{{ui:integrations.action.startWorktree}}**：为 Issue 打开一个拥有[自己 worktree](#pane-worktree) 的窗格。Linear、Jira、GitLab Issue 的分支是 `<前缀>/<键>-<标题>`(例如 `lin/ENG-123-fix-login`、`jira/PROJ-7-…`、`gl/12-…`)，GitHub Issue 仍为 `mighty/<窗格 id>`。Issue 的标题、正文(Jira 正文转换为 Markdown)、子项和链接会放进第一个请求。worktree 栏显示关联 Issue 的标签，点击即可打开 Issue。
- **PR**：在 Git 工作台创建 PR 时，正文末尾会附上 Issue 链接(正文留空时连同提交标题列表)。
- **{{ui:integrations.tracker.writeBack}}**：每个跟踪器用开关分别启用：{{ui:integrations.tracker.writeStart}}、{{ui:integrations.tracker.writePr}}、{{ui:integrations.tracker.writeMerge}}、{{ui:integrations.tracker.commentPr}}(默认开启)。Linear 移到团队的 started 或 completed 状态，Jira 使用名称匹配的转换，GitLab 使用 `In Progress`、`In Review` 标签并关闭。写入结果会留在窗格记录中。不会写入 GitHub。

## 分叉对话 {#fork}
想保留已积累的上下文去尝试另一种做法时，可以在新的 worktree 中打开一个接续此窗格对话的新窗格。原窗格的对话和文件保持不变。

- **入口**：窗格的 ··· 菜单 {{ui:fork.menu.pane}}，或结果块标题右键菜单中的 {{ui:fork.menu.here}}（到该请求为止的对话）。运行中的窗格需等结束后再分叉。
- **worktree**：从原窗格的 HEAD 创建 `mighty/fork-<短 id>` 分支和 worktree。原窗格若在自己的 worktree 中工作，分叉会合并到同一个基准分支。复制的文件、准备命令、`.mighty.yaml` 和[端口范围](#mighty-yaml)与其他 worktree 窗格相同。
- **未提交的更改**：勾选 {{ui:fork.dialog.carry}}（默认开启）时，原窗格改动的文件和新文件会带到新 worktree（用 `git stash create` 在不改动原窗格的情况下生成，再应用到新 worktree）。gitignore 的文件不会带上。无法带上时原窗格保持不变，并在新窗格的记录中说明原因。
- **接续对话**：Claude 和 Codex 会在第一个请求时由 CLI 把原会话复制为新会话（Claude `--fork-session`，Codex `exec fork`），原会话不变。Antigravity、opencode、Hermes、pi，以及从非最后一个请求分叉时，第一个请求前会附上包含最后的请求、计划、最后的结果和改动文件列表的摘要（最多 12,000 字，看起来像密钥的行会被删除）。CLI 找不到原会话时，下一个请求会附上摘要。
- **显示**：新窗格在侧边栏中缩进显示在原窗格下方，窗格上方有原窗格名称和跳转按钮。手机的窗格信息中也会以 `forkedFrom` 带上。
- **SSH 工作区**：分叉窗格的 worktree 建在该主机上，未提交的更改也在该主机上带过去（新文件不会经过链接，也不会覆盖已有的文件）。Claude 的对话记录也会在该主机上按新文件夹名复制。连接不到该主机时不会分叉，并说明原因。

## 对比运行 {#compare}
困难的任务往往值得多试几种做法，再选出最好的。把同一个请求从同一基准提交同时交给多个代理（或同一代理多次，例如 3 个 Claude），并排对比结果，然后合并其中一个。

- **开始**：在输入框中写下请求，空闲窗格从发送按钮的右键菜单、运行中的窗格从 {{ui:composer.busySend.title}} 菜单中选择 {{ui:compare.menu}}。在选择窗口中选好代理和模型的行（{{ui:compare.picker.add}}，同一代理可以出现多次）、{{ui:compare.picker.base}}（默认 HEAD，或此分支最近的提交），需要时再选 {{ui:compare.picker.test}}（`.mighty.yaml` 中的运行操作或任务），然后按 {{ui:compare.picker.start}}。
- **可选的代理**：只显示现在能创建子任务的代理（Claude、Codex、Antigravity 中已安装、已登录且有可修改文件的模式的）。最多 4 行。比较运行的子任务与你亲自开启的 3 个、`delegate` 创建的 3 个分开计算，一个窗格最多可同时打开 4 个（该窗格所有比较运行合计，SSH 工作区也一样）。因此在第一次比较运行的子任务被合并、丢弃或关闭之前，第二次比较运行只能选剩余的名额。
- **运行**：每一行都会像 [在新 worktree 中运行](#worktree) 那样同时打开一个子窗格，使用同一请求、同一基准提交和各自的 [端口范围](#mighty-yaml)。行中选了模型时，该子任务使用该模型。某一行无法开始时，该列会写明原因，其余照常进行。
- **预算**：设置了每日预算且今天的花费加上预计费用可能超出时，开始前会先提醒。预计费用是粗略值：对每个子任务，取用量账本最近 30 天中同一代理和模型单次运行的平均费用相加（没有该模型时取代理的平均值，都没有时按每个子任务 $1）。
- **分组卡片和{{ui:compare.view.title}}**：开始的窗格下方，侧栏会出现一张卡片，点按即可打开 {{ui:compare.view.title}}。每一列显示代理和模型、状态、相对基准变更的文件数和 +/− 行数（`git diff --shortstat <基准>...<分支>`）、用时、用量账本中的费用，以及所选测试的结果（子任务报告后在其 worktree 中运行一次），并有 {{ui:compare.action.open}}、{{ui:compare.action.diff}}、{{ui:compare.action.merge}}、{{ui:compare.action.discard}} 按钮。分组保存在窗格状态中，重启应用后仍可打开。
- **选出一个**：{{ui:compare.action.merge}} 只适用于已报告的子任务，遵循卡片合并的规则和合并前检查。合并完成后会询问是否一并丢弃其余子任务。撤销合并仍在子任务卡片上进行。{{ui:compare.view.dismiss}} 只移除卡片，子任务保持不变。
- 手机的窗格信息中以 `compareRuns` 包含该分组，但手机界面暂不显示。
- **SSH 工作区**：每个子任务都是该主机上的一个 worktree 窗格，从所选提交开始，请求作为它的第一次运行。只能选择在该主机上运行的代理（Claude、Codex、Antigravity、pi）。合并与 worktree 栏的 {{ui:paneWorktree.bar.commitMerge}} 相同（包括合并前检查），丢弃与栏中的丢弃相同。测试从 `.mighty.yaml` 的运行操作和任务中选择，在该主机上运行。连接不到该主机时不会创建子任务，合并会失败，丢弃会停止。

## 端口、运行操作和清理命令 {#mighty-yaml}
仓库顶层的 `.mighty.yaml` 可以为每个 worktree 指定端口、运行操作，以及删除 worktree 时运行的清理命令。只有在 {{ui:projectTasks.title}} 面板中按下 {{ui:projectTasks.approve}} 之后，其中的命令才会运行；文件改变后需要重新批准。远程工作区不使用它。

```yaml
version: 1
worktree:
  setup: npm ci
  teardown: docker compose down -v
  teardownTimeoutSeconds: 60
  ports: 10
run:
  dev:
    command: npm run dev -- --port $MIGHTY_PORT
  e2e:
    command: npm run test:e2e
    concurrent: false
    url: off
```

- **端口范围**:每个 worktree(包括主检出)会从 41000 起分配 `ports` 个(默认 10 个)不与其他 worktree 重叠的端口，并跳过其他程序已在监听的端口。分配记录在应用数据文件夹的 `ports.json` 中，删除 worktree 时归还；应用关闭期间消失的 worktree 的端口会在下次启动时归还。`ports: 0` 表示不分配。工具栏会显示 `:41010–41019` 这样的范围。
- **环境变量**:准备命令、清理命令、运行操作、任务以及 worktree 窗格中的代理会获得 `MIGHTY_PORT`(第一个端口)、`MIGHTY_PORT_COUNT`、`MIGHTY_WORKTREE_NAME`、`MIGHTY_ROOT_PATH`(主检出)和 `MIGHTY_WORKTREE_PATH`。命令中的 `$MIGHTY_PORT` 由 shell 展开(Windows 的 cmd 中为 `%MIGHTY_PORT%`)。`worktree.env` 不能设置这些名称。
- **运行操作(`run`)**:{{ui:projectTasks.title}} 面板的 {{ui:projectTasks.runActions}} 部分会显示每个操作的状态以及 {{ui:projectTasks.start}}、{{ui:projectTasks.stop}} 按钮。在输出中找到 `http://localhost:<端口>` 这样的地址时会显示为链接(`url: auto`，默认)，端口不在该 worktree 范围内时会标出 {{ui:projectTasks.urlOutside}}。`url` 写成地址时显示该地址，写成 `off` 时不显示。`concurrent: false` 的操作带有 {{ui:projectTasks.notConcurrent}} 标记，启动时会先停止同一仓库其他 worktree 中运行的同一操作。运行中的操作也会显示在工具栏上。
- **清理命令(`teardown`)**:放弃 worktree，或关闭已合并的窗格而删除 worktree 时，会先停止该 worktree 的运行操作，然后在 worktree 文件夹中运行清理命令。超过 `teardownTimeoutSeconds`(默认 60 秒)仍未结束则停止。即使清理失败或被停止，worktree 也会删除，结果保留在窗格记录和 {{ui:projectTasks.title}} 面板中。
- **个人设置**:在 `.mighty.local.yaml` 中写相同的键时以它为准；运行操作按名称覆盖。

## 回合结束验证 {#verify}
在 `.mighty.yaml` 的 `verify` 中写上类型检查、lint、单元测试等命令后，代理窗格的请求每次正常结束都会运行它们来检查结果。失败时会把失败摘要作为下一个请求发给同一个代理让它修复，然后再次检查。和其他命令一样，只有在 {{ui:projectTasks.title}} 面板中按下 {{ui:projectTasks.approve}} 后才会运行，添加或修改 `verify` 后需要重新批准。批准前的列表会以 `verify:` 行完整显示命令。

```yaml
verify:
  commands:
    typecheck: npx tsc --noEmit
    test:
      command: npm test
      cwd: web
      timeoutSeconds: 600
  maxRounds: 2
  when: changed
  browser:
    runAction: dev
    paths: [/, /settings]
```

- **何时**：仅在请求正常结束后。已停止或失败、正在等待批准或问题、队列中还有下一个请求时都不会运行；委派的子窗格、比较运行的子项和镜像窗格也不会。`when: changed`（默认）只在请求更改了文件时检查，`always` 每次都检查。无法通过快照判断是否更改的文件夹会进行检查。
- **在哪里**：在请求工作的文件夹（worktree 窗格即其 worktree）中，有 `cwd` 时在其下的文件夹中依次运行，带有与运行动作相同的环境变量（`MIGHTY_PORT` 等）和 `worktree.env`。每个命令在 `timeoutSeconds`（默认 300 秒）内未结束即为失败。SSH 工作区在该机器上运行；开启了[沙箱](approvals.md#sandbox)的工作区在与代理运行相同的沙箱中运行（无法准备沙箱时不运行）。
- **自动修复**：收集每个失败命令输出的最后 40 行（删除看起来像密钥的行和 `worktree.env` 的值，总共不超过 6,000 个字符），以“验证失败 — 请修复”请求发送。代理结束后再次检查。
- **何时停止**：通过即结束。发送了 `maxRounds` 次（默认 2，0–5）修复请求仍失败、修复后出现相同错误（去掉时间和数字后输出相同）、再运行一次可能超出[每日预算](settings.md#schedules)、有人向该窗格发送了任何内容或停止了运行、修复请求未正常结束时都会停止。除非是人为停止，否则会通过系统通知和手机通知告知。`maxRounds: 0` 时只检查，不请求修复。
- **浏览器检查（`browser`）**：用此电脑上已安装的 Chrome、Edge、Chromium、Brave 之一，以无窗口模式和独立配置文件启动，打开开发服务器的 `paths`（默认 `/`），记录控制台错误、未捕获的异常、失败的请求（4xx、5xx、网络错误）、无障碍树摘要和截图。地址为 `url`（仅限此电脑上的地址）或 `runAction` 所选运行动作输出的地址；不在此 worktree 的[端口范围](#mighty-yaml)或该运行动作端口内的地址一律不打开。不会下载浏览器；未安装时会如实说明，其余检查照常进行。代理也可以用 `mighty_browser_check` 工具自行进行同样的检查。
- 结果会留在结果卡片的[验证凭据](agent-pane.md#verify-receipt)中。

## 让 Claude 分工 {#delegate}
Claude 或 Codex 窗格（父窗格）也可以把工作拆开，交给其他窗格（子窗格）；除非它指定别的代理，子窗格运行它自己的代理。子窗格报告后，父窗格收到通知，阅读报告，再决定是否合并结果。Claude 和 Codex 父窗格在 Mac 应用和 Windows 应用中都可使用。

1. 把一个 git 仓库文件夹作为工作区打开，再打开一个 Claude 窗格。这个窗格就是父窗格。
2. 在请求中写明工具名称，例如“用 delegate 工具打开两个 Mighty 子窗格，把这件事分给它们”。如果只说“分开做”，Claude 可能会改用它自己内置的子智能体。
3. 子窗格在父窗格旁边打开，并在侧边栏中缩进一格列在父窗格下方。

- 父窗格使用的工具有 `delegate`（交出任务；可以用 `agent` 指定子窗格的代理：`claude`、`codex` 或 `antigravity`）、`list_children`（子窗格列表）、`child_status`（读取状态和报告）、`merge`（合并）和 `follow_up`（后续指示）。父窗格不能调用 `discard`（丢弃），只有你能在卡片上丢弃。
- 每个子窗格都有一个分支 `mighty/<子窗格 id>` 和一个 worktree（Mac 上为 `~/.mightycode/worktrees/<子窗格 id>`，Windows 上为 `%USERPROFILE%\.mightycode\worktrees\<子窗格 id>`）。子窗格从父窗格的最后一次提交开始。
- 交出的任务写在 TASK.md 中，结果写在 REPORT.md 中。这两个文件位于 worktree 内被 git 忽略的文件夹里，不会被提交。
- 子窗格的权限模式是父窗格选择的模式，不能比父窗格自己的模式更宽。另一个代理的子窗格不能做父窗格模式不做的事：只有父窗格的模式不经询问就更改文件时（接受编辑或 auto 模式的 Claude、{{ui:permission.label.onRequest}} 或 {{ui:permission.label.acceptEditsCodex}} 模式的 Codex、{{ui:permission.label.acceptEdits}} 模式的 Antigravity、{{ui:permission.label.fullAccess}} 模式的任何代理）才不经询问更改文件，只有父窗格的模式能更改文件时才在询问后更改文件，只有父窗格的模式不经询问就运行 shell 命令时（auto 模式的 Claude、{{ui:permission.label.defaultCodex}} 或 {{ui:permission.label.acceptEditsCodex}} 模式的 Codex、{{ui:permission.label.fullAccess}} 模式的任何代理）才不经询问运行，只有在 {{ui:permission.label.fullAccess}} 下才有完全访问权限。因此在运行 shell 命令前会询问的 Claude 接受编辑模式下，Codex 子窗格得到的是 {{ui:permission.label.onRequest}}，而不是 {{ui:permission.label.acceptEditsCodex}}；计划模式或手动模式的 Claude 窗格完全不能有 Codex 或 Antigravity 子窗格。只有你能在两次运行之间，从子窗格输入框的权限菜单或手机设置中放宽它。
- 子窗格以与父窗格相同的视图打开（{{ui:graph.view.diagram}}, {{ui:graph.view.timeline}} 中相同的那一个）。
- 父窗格不能回答子窗格的批准卡片。子窗格中出现卡片时，请你在那个窗格中亲自回答。
- 一个父窗格最多可以有 3 个由 `delegate` 工具创建的打开的子窗格，与亲自启动的 3 个分开计算，两者互不阻挡。子窗格不能再委派。已报告或已合并的子窗格只要窗格还开着也会计入这 3 个，关闭已完成的子窗格就能腾出名额。磁盘剩余空间不足 10 GB 时，交出任务会被拒绝。
- 父窗格通过通知得知子窗格的报告（[通知和保留](#notices)），用 `follow_up` 发送后续指示（[后续指示](#follow-up)），用 `merge` 合并（[合并](#merge)）。

## Codex 和 Antigravity {#agents}
Mighty 编排也适用于 Codex 和 Antigravity 窗格。默认情况下，子窗格运行父窗格的代理：Codex 窗格的子窗格是 Codex 窗格，Antigravity 窗格的子窗格是 Antigravity 窗格。

任何代理都可以有三者中任意一个代理的子窗格。可以在运行时的{{ui:composer.busySend.title}}或队列行的代理选择中选择（[在新 worktree 中运行](#worktree)），也可以让 Claude 或 Codex 父窗格用 `delegate` 的 `agent`（`claude`、`codex` 或 `antigravity`）指定。子窗格按下面所写的它自己代理的规则工作：它的模式，以及由谁提交它的工作（Claude 子窗格自己提交；Codex 或 Antigravity 子窗格的工作由应用提交）。通知和合并跟随父窗格的代理。`delegate` 会拒绝它不认识的代理（`agent_unknown`）、未安装或未登录的代理（`agent_unavailable`），以及不是子代理的模式、在其中无法更改文件的模式，或能比父窗格的模式做更多事的模式（模式的比较方法见[让 Claude 分工](#delegate)）。

- **子窗格能工作的模式**：子窗格以父窗格的权限模式启动，因此 Codex 和 Antigravity 窗格只有在子窗格能更改文件的模式下才会用 {{kbd:⌘Enter}} 启动子窗格：Codex 为{{ui:permission.label.acceptEditsCodex}}、{{ui:permission.label.onRequest}}或{{ui:permission.label.fullAccess}}，Antigravity 为{{ui:permission.label.acceptEdits}}或{{ui:permission.label.fullAccess}}。在其他模式下，{{kbd:⌘Enter}} 会像以前一样把请求放入队列。
- **Codex** 窗格只有在{{ui:permission.label.acceptEditsCodex}}或{{ui:permission.label.fullAccess}}模式下才会获得委派工具：在{{ui:permission.label.defaultCodex}}模式下 Codex 会拒绝这些工具，在{{ui:permission.label.onRequest}}模式下它会以应用无法显示的方式请求批准。`delegate` 会拒绝无法更改文件的子窗格模式。只有当运行以{{ui:permission.label.fullAccess}}模式开始时，Codex 才能调用 `merge`，否则会被拒绝（`merge_human_only`），请在卡片上合并。运行中的 Codex 回合无法接收文字，因此发给运行中 Codex 父窗格的通知会在这次运行正常结束后作为下一次运行送达；如果因停止或错误而结束，则会被保留。
- **Antigravity** 窗格没有委派工具（Antigravity 只从其全局设置获取工具，而应用从不更改该设置），只能用 {{kbd:⌘Enter}}、{{ui:queue.worktree}} 按钮或{{ui:composer.busySend.title}}启动子窗格。
- 除非处于{{ui:permission.label.fullAccess}}模式，Codex 和 Antigravity 子窗格无法在自己的 worktree 中提交，因此会被告知保留更改。子窗格的运行在 REPORT.md 有变化时正常结束后，应用会把更改提交到子窗格的分支（`mighty: <子窗格 id> report r<n>`），并以该提交记录报告；因停止或错误结束时，会等到下一次正常结束。被 .gitignore 排除的新文件、看起来像机密信息的新文件（`.env*`、`*.pem`、`*.key`、`*.pfx`、`*.p12`、`*.ppk`、`id_rsa*`、`id_ed25519*`、`id_ecdsa*`、`id_dsa*`、`credentials*`、`secrets.*`、`.npmrc`、`.pypirc`）以及超过 5 MB 的新文件不会提交，后两类会在子窗格中列出名称。提交使用你在 git 中设置的姓名和邮箱（没有时提交失败，并在窗格中说明），并且不签名。worktree 中有未解决的合并冲突时，应用不会提交任何内容，并在窗格中说明。
- 卡片的合并因冲突被拒绝时，只会请能够提交的子窗格（{{ui:permission.label.fullAccess}}）解决冲突。否则请把子窗格切换到{{ui:permission.label.fullAccess}}后在子窗格中请它解决，或在该 worktree 的终端中亲自解决冲突并提交。
- 把仓库的子文件夹作为工作区打开时，Codex 子窗格也可以写入 REPORT.md 所在的 worktree 顶层文件夹，Antigravity 子窗格会添加该文件夹。

## 子窗格的状态 {#states}
侧边栏会显示每个子窗格的状态。

| 状态 | 含义 |
|---|---|
| {{ui:delegation.child.state.creating}} | 正在创建 worktree 和窗格。 |
| {{ui:delegation.child.state.running}} | 正在工作。 |
| {{ui:delegation.child.state.waiting}} | 正在等你回答批准或提问。 |
| {{ui:delegation.child.state.reported}} | 已留下 REPORT.md 并结束运行，可以合并。报告之后的运行若没有任何改动就结束，也保持此状态。 |
| {{ui:delegation.child.state.merged}} | 已合并到父窗格的分支。 |
| {{ui:delegation.child.state.ended}} | 运行结束，但没有新的报告。 |
| {{ui:delegation.child.state.interrupted}} | 因为退出应用，运行被中断。 |
| {{ui:delegation.child.state.failed}} | 子窗格未能启动。 |
| {{ui:delegation.child.state.closed}} | 窗格已关闭。在丢弃之前会一直留在列表中。 |

子窗格开始新的运行时，{{ui:delegation.child.state.reported}}状态会被清除；再次报告时会使用下一个报告编号。如果子窗格没有报告就结束了，请在子窗格中再次要求它提交并写入 REPORT.md。

## 合并 {#merge}
把子窗格的结果放进父窗格的分支有两种方法。

1. **卡片上的{{ui:delegation.card.merge}}按钮**：由你来点。亲自启动的子窗格只能用这种方法合并。能快进（fast-forward）时就快进，否则只创建一个合并提交。它会先检查冲突，拒绝时会在卡片上显示原因。
   子窗格仍在工作、父窗格正在运行或父文件夹有未提交的更改时，不会拒绝，而是排队合并：卡片上显示它在等待什么，等这些结束后会自动合并。子窗格没有新的报告就结束时，排队会取消。如果创建子窗格时的父分支之后已合并到当前检出的分支（例如 main）或已删除，就合并到当前分支；父窗格的 `merge` 工具也会这样跟随分支。
   合并成功后，若 worktree 中没有未提交的文件，会立即关闭子窗格并删除其 worktree 和分支；若有剩余文件，会连同列表询问是否一起删除。子 worktree 中还有其他 worktree 或 git 无法判断时，会像以前一样询问是否立即丢弃：选择{{ui:delegation.mergedDiscard.confirm}}会关闭子窗格并删除其 worktree 和分支；已合并的内容仍保留在父窗格的分支中，但这次合并将无法撤销。选择{{ui:delegation.mergedDiscard.later}}则保留卡片。
   因冲突被拒绝时，应用会请子窗格并入父分支、解决冲突、提交并重新报告。卡片上会这样提示，子窗格变为{{ui:delegation.child.state.running}}。子窗格重新报告后会自动合并。这个请求每次报告只发送一次。子窗格现在无法开始运行时不会发送，卡片上只留下拒绝原因，下次合并时会再试一次。如果子窗格收到请求却没能开始，或者子窗格处于计划模式、只做了计划就结束了，请直接让子窗格解决冲突并重新报告。对于 Codex 和 Antigravity 子窗格，只有在{{ui:permission.label.fullAccess}}模式下才会发送此请求（[Codex 和 Antigravity](#agents)）。
2. **父窗格的 `merge` 工具**：只适用于交给 Claude 的子窗格，只做快进。父窗格要一起传入 `child_status` 给出的子窗格 head。子窗格尚未报告、子窗格或父窗格文件夹中有未提交的更改、子窗格的 head 已移动、父窗格的分支在此期间已经前进（分叉），或者父窗格的分支没有被检出时，合并会被拒绝，并且不做任何更改。这个工具是否会弹出批准卡片，取决于父窗格的权限模式。只有当运行以{{ui:permission.label.fullAccess}}模式开始时，Codex 父窗格才能使用它，在其他模式下会以 `merge_human_only` 被拒绝，请在卡片上合并。

> [!tip]
> 合并一个子窗格后，父窗格的分支会向前移动，所以其余子窗格通常会被 `merge` 工具以分叉为由拒绝。这时请用卡片上的{{ui:delegation.card.merge}}按钮合并。

合并之后，在子窗格开始新的运行之前，即使关闭了窗格，卡片上也会保留{{ui:delegation.card.undo}}按钮。只有当父窗格的分支仍停在这次合并、父窗格空闲且没有未提交的更改时才能使用。撤销后，文件会回到合并之前的样子，窗格还开着的子窗格会重新变为{{ui:delegation.child.state.reported}}。

> [!note]
> 如果合并已完成，但委派记录没能保存，卡片会告诉你。合并保持不变，记录会在下次保存时重新写入。

## 丢弃 {#discard}
卡片上的{{ui:delegation.card.discard}}按钮会删除子窗格的 worktree 和分支，连同尚未合并的工作一起删除。只有你能丢弃，父窗格不能。已合并的子窗格也可以在合并后立即弹出的询问窗口中丢弃（[合并](#merge)）。

- 确认窗口（{{ui:delegation.discard.title}}）会列出嵌套在子窗格 worktree 中、将被一起删除的所有其他 worktree。
- 无法确认这份列表时，确认窗口不会出现丢弃按钮，也不会停止或删除任何东西。

> [!tip]
> 应用绝不会自行删除尚未合并的工作。只有你点击的丢弃才会让未合并的工作消失。

## 关闭窗格和清理 {#close}
- **关闭已合并的子窗格时**，如果它的工作已全部合并且没有留下更改，应用会清理它的 worktree 和分支（`git branch -d`，从不强制删除）。否则只会关闭窗格，worktree 保留。
- **在合并之前关闭子窗格时**，应用就不能再合并它了。卡片上只剩丢弃，分支和 worktree 会一直保留到你丢弃为止。
- **关闭还有打开子窗格的父窗格时**，桌面应用会先询问（{{ui:delegation.parentClose.title}}）。只关闭父窗格，子窗格继续工作。
- 父窗格已关闭的子窗格会作为卡片留在侧边栏的{{ui:delegation.tree.parentClosed}}项目下面。它们的通知保留在卡片上并显示数量，也不再接收后续指示。
- 还有打开的子窗格，或者还有尚未清理的子窗格 worktree 的工作区，无法删除。
- {{platform:windows}} 如果子窗格的 worktree 中有目录联接（junction）或符号链接（例如用链接创建的 `node_modules`），关闭已合并的子窗格时应用不会清理，而是保留 worktree，以免顺着链接把它指向的文件夹也删掉。这时请用卡片上的{{ui:delegation.card.discard}}按钮删除：丢弃会先只移除链接本身，不会动链接指向的文件夹。

![[delegation-parent-closed]]

## 通知和保留 {#notices}
交给 Claude 的子窗格报告或未能启动时，父窗格会收到一条通知。父窗格启动的运行（最初交出的任务或后续指示）没有报告就结束时，也会收到通知。通知只是一个以 `[Mighty Code notice …]` 开头的简短指引，报告内容由父窗格用 `child_status` 读取。亲自启动的子窗格不会发送通知。

- 父窗格正在运行 Claude 时，通知会直接送进这次运行，并在父窗格的 Mighty 视图中显示为橙色的{{ui:graph.block.delegationNotice}}块。在父窗格答完之后才到达的通知同样显示为这个块，Claude 对它的回答不会接在父窗格请求的结果后面，而是作为单独的{{ui:graph.block.delegationNotice}}图表打开。正在运行的 Codex 父窗格会在这次运行正常结束后，把通知作为下一次运行接收。
- 父窗格在打开应用后正常结束了上一次运行、现在处于空闲状态时，通知会开始它的下一次运行。
- 其他情况下通知会被保留：停止之后、出错之后、重新打开应用之后，以及 CLI 更新期间。一旦已有保留的项目，新通知也会排在其后保留，顺序不会乱。

保留的通知显示在父窗格输入框上方队列的最上面，是带图钉标记的行。这些行没有删除按钮，而且与普通的排队请求不同，停止或重新打开应用后也会留下。

![[delegation-held]]

- 你下一次发送内容时，所有保留的项目会按从旧到新的顺序，排在你的文字前面，作为一次运行一起送出。
- 点击最上面一条保留行的{{ui:queue.runNow}}按钮，会把所有保留的项目作为一次运行立即送到本窗格。其余保留的行没有这个按钮。打开新 worktree 的{{ui:queue.runNow}}不会动这些保留的项目，它们仍为本窗格保留。有项目被保留时，本来可以直接送进正在运行的 Claude 的请求会保持灰色，直到保留的项目送出。
- 每条通知只会送达一次，重新打开应用也一样。保留的行也会显示在手机的队列中，从手机发送也会一起送出。

## 后续指示 {#follow-up}
父窗格可以用 `follow_up` 工具给交给 Claude 的子窗格发送最多两条后续指示。后续指示按照与通知相同的规则送到子窗格，每条最多 48 KiB。你可以直接在子窗格的输入框中输入，没有次数限制。

## 开启和关闭 {#enable}
{{platform:mac}} 除非你关闭，否则它一直开启。在设置的{{ui:settings.nav.general}}标签页{{ui:settings.agents.sectionTitle}}中切换{{ui:settings.delegation.toggle}}；无需重新打开应用，从下一次开始的运行起生效。同一个开关就是 defaults 键 `delegation.enabled`，因此在终端中运行下面的命令也能关闭它，运行 `defaults delete dev.mightycode.native delegation.enabled` 则重新开启。

```sh
defaults write dev.mightycode.native delegation.enabled -bool false
```

{{platform:mac}} 以前的版本只有设置了该键才会开启，关闭的方法是 `defaults delete …`。现在这样做会让它保持开启，所以要继续关闭，请在设置中关闭{{ui:settings.delegation.toggle}}。

{{platform:windows}} 除非你关闭，否则它一直开启。在设置的{{ui:settings.nav.general}}标签页{{ui:settings.agents.sectionTitle}}中切换{{ui:settings.delegation.toggle}}；无需重新打开应用，从下一次开始的运行起生效。同一个开关就是 `HKEY_CURRENT_USER\Software\MightyCode` 下的 `delegation.enabled` 值（REG_DWORD）：为 0 时关闭，没有这个值或为其他值时开启。因此在命令提示符或 PowerShell 中运行下面的命令也能关闭它，运行 `reg delete "HKCU\Software\MightyCode" /v delegation.enabled /f` 则重新开启。

```
reg add "HKCU\Software\MightyCode" /v delegation.enabled /t REG_DWORD /d 0 /f
```

{{platform:windows}} 早期版本只有把这个值设为 1 时才开启。那时留下的 1 会让它保持开启。

- 开启后，Claude 或 Codex 窗格每次开始新的运行时都会带上委派工具。已经在进行中的运行不会带上。
- 关闭后只会去掉工具，{{kbd:⌘Enter}} 会像 {{kbd:Enter}} 一样把请求放入队列。已有的子窗格、卡片和清理都照常工作。

## 限制和支持范围 {#limits}
| 项目 | 限制 |
|---|---|
| 一个父窗格的打开子窗格 | 亲自启动的 3 个，`delegate` 创建的 3 个，比较运行的 4 个 |
| 委派深度 | 一层（子窗格不能再创建子窗格） |
| 一个子窗格的后续指示（`follow_up`） | 2 条，每条最多 48 KiB |
| 交出的任务（TASK.md、亲自启动的请求） | 64 KiB |
| 所需磁盘剩余空间 | 10 GB |

- 子窗格运行与父窗格相同的代理，也可以运行另一个代理（[Codex 和 Antigravity](#agents)）。Claude 和 Codex 窗格还可以用 delegate 工具创建子窗格；Antigravity 窗格没有工具，只能通过 {{kbd:⌘Enter}}、{{ui:queue.worktree}}按钮、{{ui:composer.busySend.title}}和{{ui:queue.runNow}}创建（[Codex 和 Antigravity](#agents)）。
- Mac 应用和 Windows 应用都默认开启，并在设置中提供开关（[开启和关闭](#enable)）。手机没有新的界面，保留的行也会显示在手机的队列中。
