---
title: 智能体窗格
order: 3
section: agent-pane
---
智能体窗格是与 Claude、Codex、Antigravity、pi、opencode 或 Hermes 之一对话的窗格。在下方的输入框中写请求，在上方查看进度和结果。

![[agent-basic]]

## 发送请求 {#send}
1. 在输入框中写下要做的事。
2. 按{{kbd:Enter}}或点击发送按钮。要换行，请按{{kbd:⇧Enter}}。
3. 运行期间，发送按钮的位置会变成停止按钮。要停止，请点击该按钮。

使用输入法组字时（例如输入中文或韩文）按下的{{kbd:Enter}}只会完成组字，不会发送。

在输入框第一行按 {{kbd:↑}}，会填入此窗格中上一次发送的请求；继续按会回到更早的请求。在最后一行按 {{kbd:↓}}，会前往下一条（更新的）请求；越过最新的请求后，正在输入的文字会原样恢复。修改调出的请求后，它就成为正在输入的文字。

![[composer]]

### 运行时继续提出请求 {#queue}
任务运行期间也可以写下一个请求。输入框中有文字时，停止按钮旁会出现与发送按钮相同的圆形按钮。
- {{kbd:Enter}}或{{ui:queue.add}}按钮：作为下一个请求放入队列。当前任务结束后按顺序执行。每个窗格最多可放 16 个。如果在下面的{{ui:composer.busySend.title}}中选了代理，则改为立即用该代理在新的 worktree 中运行。
- {{ui:composer.busySend.title}}：只在运行时显示的选项；输入文字后它与发送按钮合为一个控件，呈[目标 ▾ | ↑]的样子（没有输入时单独显示为小胶囊）。只有目标为{{ui:composer.busySend.queue}}时，其左侧才另外显示{{ui:queue.worktree}}按钮；选择了 worktree 时发送按钮做同样的事，因此隐藏它。选择{{ui:composer.busySend.queue}}时显示列表图标，选择了代理时显示该代理的图标；悬停时显示当前选择。它提供{{ui:composer.busySend.queue}}（默认），以及此窗格可作为子窗格启动的每个代理的一项，例如“worktree · Codex”（Claude、Codex、Antigravity）。CLI 未安装或未登录、没有合适模式，或你直接打开的子窗格已有 3 个的项目显示为灰色并说明原因。所选的值决定{{kbd:Enter}}和圆形按钮的作用：把请求放入队列，或在此窗格旁打开该代理的子窗格，立即在新的 worktree 中运行。输入框的提示和按钮名称也随之改变。每个窗格会记住所选的值，直到退出应用。只有附件、没有文字的请求会放入队列。
- {{kbd:⌘Enter}}（Windows 上为{{kbd:Ctrl+Enter}}）或{{ui:queue.worktree}}按钮：无论{{ui:composer.busySend.title}}选了什么，始终使用此窗格自己的代理。在开启了 Mighty 编排、位于有提交的分支上的 git 仓库中的 Claude 窗格，或处于子窗格能更改文件的模式的 Codex、Antigravity 窗格里，会在此窗格旁打开一个拥有自己 git worktree 的新子窗格并立即运行，正在运行的任务照常继续；子窗格完成后，在侧边栏的卡片上合并它的结果（[在新的 worktree 中运行](delegation.md#worktree)）。
- 在子窗格中、在这样的仓库之外，或关闭 Mighty 编排时，{{kbd:⌘Enter}}也会像{{kbd:Enter}}一样放入队列。如果此窗格的代理在当前模式下无法创建子窗格、但其他代理可以，{{kbd:⌘Enter}}会放入队列，不过可以通过{{ui:composer.busySend.title}}和下面{{ui:queue.runNow}}的代理选择，在该代理的子窗格中运行。无法创建或启动子窗格时，文字同样会放入队列（[在新的 worktree 中运行](delegation.md#worktree)）。你发送的内容不再插入正在运行的任务；在较早的记录中，Mighty 视图仍会把这类文字显示为橙色的{{ui:graph.block.steer}}块。直接送进正在运行的 Claude 父窗格的委派通知，显示为同样颜色的{{ui:graph.block.delegationNotice}}块（[通知和保留](delegation.md#notices)）。
- 队列中的每个请求，在 ✕ 旁都有一个{{ui:queue.runNow}}按钮。在能创建新 worktree 的地方（满足上面 {{kbd:⌘Enter}} 的条件，或换用其他代理就能创建子窗格的窗格），它会立即在新的子窗格中运行该请求，正在运行的任务照常继续。在按钮前的代理选择（默认是此窗格的代理）中选择其他代理，即可在该代理的子窗格中运行。按钮的提示会说明将会发生哪一种。只有附件、没有文字的请求不会进入 worktree。
- 无法创建 worktree 时，请求在本窗格中处理。窗格处于空闲状态（例如因出错而停止）时，会立即开始该请求。Claude 的回答已结束、只剩后台任务的窗格，会把它直接送进正在运行的 Claude，并在 Mighty 视图中显示为{{ui:graph.block.steer}}块（带附件的请求要等到任务结束）。其他情况下按钮显示为灰色，提示会说明原因：当前任务必须先结束、为该窗格保留的项目要先送出，或 CLI 正在更新。此时请求依次等待。Antigravity 卡片的后续请求只会在它自己的窗格中运行。
- 用铅笔按钮（{{ui:queue.edit}}）可就地编辑排队请求的文字：{{kbd:Enter}} 保存，{{kbd:Esc}} 取消（{{kbd:Shift+Enter}} 换行）。附件和模式保持不变，文字清空时不会保存。已允许卡片的后续请求不能编辑。
- 用 ↑、↓ 按钮（{{ui:queue.moveUp}}、{{ui:queue.moveDown}}），或选中某行后按 {{kbd:↑}}、{{kbd:↓}} 键调整顺序。最上面的请求下一个运行，队列按新顺序运行。保留的行始终在最上面，不能编辑或移动。

停止任务时，排队中的请求也会被取消。但在只等待后台工作的窗格中停止时，队列会保留，并立即开始第一个请求。重新打开应用后，队列会被清空。

## 输入框工具 {#composer}
输入框下方一行从左到右依次是附件、模型、思考强度、权限和更多运行设置（…）。窗格较窄时，部分项目会收进一个菜单。运行期间无法更改，更改后的值从下一个请求起生效。

{{ui:compare.menu}}：空闲窗格在发送按钮的右键菜单中，运行中的窗格在 {{ui:composer.busySend.title}} 菜单中。它把输入框中的请求从同一基准提交同时交给多个代理，并排对比结果（[对比运行](delegation.md#compare)）。

### 模型 {#model}
点击模型按钮，可以选择{{ui:composer.label.runner}}（Claude、Codex、Antigravity、pi、opencode、Hermes）和{{ui:composer.label.model}}。如果列表不对，请点击{{ui:composer.model.refresh}}。切换智能体会开始新的对话。

### 思考强度 {#effort}
{{ui:composer.effort.label}}决定模型思考的深度，可以在 Auto、Low、Medium、High、XHigh、Max 中选择。只显示模型支持的级别。Codex 模型支持时还有 {{ui:composer.effort.ultra}}：思考最深，并可能把部分工作交给 Codex 自己的子代理，因此用量可能更多。Antigravity 没有此项；pi 有 Off、Minimal、Low、Medium、High、XHigh；opencode 有 Low、Medium、High、Max，即模型提供的变体；Hermes 有 Low、Medium、High、XHigh、Max。

### 权限 {#permission}
权限决定智能体无需询问即可做哪些事。

| 智能体 | 可选权限 |
|---|---|
| Claude | {{ui:permission.label.claude.plan}} · {{ui:permission.label.claude.default}} · {{ui:permission.label.claude.acceptEdits}} · {{ui:permission.label.claude.auto}} · {{ui:permission.label.claude.fullAccess}} |
| Codex | {{ui:permission.label.defaultCodex}} · {{ui:permission.label.onRequest}} · {{ui:permission.label.acceptEditsCodex}} · {{ui:permission.label.fullAccess}} |
| Antigravity | {{ui:permission.label.default}} · {{ui:permission.label.plan}} · {{ui:permission.label.acceptEdits}} · {{ui:permission.label.fullAccess}} |
| pi | {{ui:permission.label.defaultCodex}} · {{ui:permission.label.fullAccess}} |
| opencode | {{ui:permission.label.default}} · {{ui:permission.label.plan}} · {{ui:permission.label.acceptEdits}} · {{ui:permission.label.fullAccess}} |
| Hermes | {{ui:permission.label.default}} · {{ui:permission.label.acceptEdits}} · {{ui:permission.label.fullAccess}} |

- Claude 的 {{ui:permission.label.claude.default}}、Codex 的{{ui:permission.label.onRequest}}除{{ui:permission.label.fullAccess}}外的所有 opencode 权限，以及 Hermes 的文件修改和危险命令在需要额外权限时，会在窗格中请求批准。请参阅[批准与提问](approvals.md#permission)。
- 只有已安装的 Claude CLI 支持时，才会显示 {{ui:permission.label.claude.auto}}。Codex 的{{ui:permission.label.onRequest}}在不支持它的旧版 Codex 中不显示。
- {{ui:permission.label.claude.fullAccess}} 和{{ui:permission.label.fullAccess}}权限也能访问项目以外的文件和命令。请仅在确有必要时使用。

### Antigravity {#antigravity}
Google 的智能体通过 Antigravity CLI（`agy`）运行，它也可以运行 Claude 和 GPT-OSS 模型。

- 安装：在 Mac 上运行 `curl -fsSL https://antigravity.google/cli/install.sh | bash`，`agy` 会安装到 `~/.local/bin`。在 Windows 上，在 PowerShell 中运行 `irm https://antigravity.google/cli/install.ps1 | iex`，`agy.exe` 会安装到 `%LOCALAPPDATA%\agy\bin`。应用通过 `agy update` 更新它。
- 登录：在终端窗格中运行一次 `agy`，并在浏览器中用 Google 账户登录。点击{{ui:settings.nav.tools}}标签页中的{{ui:settings.cliAccounts.buttonLogin}}，应用会替你运行。Antigravity 没有退出登录命令；要切换账户，请在 `agy` 中使用 `/logout`。
- 权限：Antigravity 无法在应用中询问你。{{ui:permission.label.default}}遵循 agy 中保存的 Agent Mode（如果没有在 agy 的 `/config` 中更改，则为审核）：它可以读取工作区的文件，但会拒绝需要询问的文件编辑和 shell 命令，结束这一轮后，再用权限卡片询问被拒绝的操作。点{{ui:permission.allowOnce}}或{{ui:antigravity.permission.allowAlways}}后会在同一个对话中继续（[Antigravity 的权限卡片](approvals.md#antigravity-permission)）。{{ui:permission.label.plan}}会在 agy 自己的文件夹中写下计划，然后停止，不会修改项目；要执行计划，请用其他模式发送下一个请求。如果想提前允许某些操作，请自行在 `~/.gemini/antigravity-cli/settings.json` 的 `permissions.allow` 中添加允许规则（例如 `command(git status)`；规则会允许以该文本开头的所有命令，请尽量写得窄一些），或选择{{ui:permission.label.acceptEdits}}（文件编辑）或{{ui:permission.label.fullAccess}}（全部允许）。
- Antigravity 的每个模型名称中已包含思考深度，因此没有单独的{{ui:composer.effort.label}}。
- {{ui:graph.view.mighty}}视图与 Claude、Codex 相同：Antigravity 启动的每个子代理都会在请求下方成为单独的块，其内容（收到的指示、消息、工具调用和发回的答复）从 `~/.gemini/antigravity-cli`（Windows 上为 `%USERPROFILE%\.gemini\antigravity-cli`）中子代理自己的记录读取；读取过的文件只显示名称，不会复制内容。对话中较早的请求也从同样的记录加载，{{ui:resume.choice.resume}}会列出此文件夹中的 Antigravity 对话。
- 以前运行 Gemini CLI 的窗格会保留记录，但下一个请求会开始新的对话；如果选过 Gemini CLI 的模型，会改回默认。

### pi {#pi}
pi（`pi`）是支持多家模型提供商的开源编码智能体 CLI。

- 安装：在{{ui:setup.title}}界面中安装（缺少 Homebrew 和 Node.js 时会先安装它们），也可以自己运行 `npm install -g @mariozechner/pi-coding-agent`。应用用 `pi update self` 更新它。
- 登录：点击{{ui:settings.nav.tools}}标签页中的{{ui:settings.cliAccounts.buttonLogin}}，会打开运行 `pi` 的终端窗格。在其中输入 `/login`，选择订阅（Claude Pro/Max、ChatGPT、GitHub Copilot）或 API 密钥。环境变量中的 API 密钥（如 `ANTHROPIC_API_KEY`）或 `~/.pi/agent/auth.json` 中保存的凭据也视为已登录。退出登录请在 pi 中输入 `/logout`。
- 模型：登录后，列表来自 `pi --list-models`，以“提供商/模型”显示，第一项遵循 pi 自身的设置。不支持思考的模型不显示{{ui:composer.effort.label}}。级别为 Off、Minimal、Low、Medium、High、XHigh。
- 权限：pi 没有审批窗口，所以只有两种模式。{{ui:permission.label.defaultCodex}}模式只启用读取工具（read、grep、find、ls）。{{ui:permission.label.fullAccess}}模式不经询问使用所有工具，也可能触及项目之外。
- 每次请求都会在窗格的工作文件夹中运行 `pi --mode rpc`，并延续窗格自己的会话，会话保存在 `~/.pi/agent/sessions`。{{ui:resume.choice.resume}}中也会列出该文件夹的 pi 会话。
- 提问：pi 没有提问工具，因此应用在每次运行时以扩展的形式加入 `ask_user` 工具（不会更改你的 `~/.pi` 设置）。pi 通过它提问时会弹出[提问卡片](approvals.md#questions)，回答后同一请求继续进行。其他扩展打开的选择、确认和输入对话框也以同样的卡片显示。
- 暂不支持：pi 窗格不参与[Mighty 编排](delegation.md)，不能在[远程工作区](remote.md)中打开，也没有审批卡。

### opencode {#opencode}
opencode（`opencode`）是支持多家模型提供商的开源编码智能体 CLI，其自有服务 opencode Zen 的免费模型无需登录即可使用。

- 安装：在{{ui:setup.title}}界面中安装（缺少 Homebrew 和 Node.js 时会先安装它们），也可以自己运行 `npm install -g @opencode/cli`。应用使用 OpenCode 2；Mac 上若有 OpenCode 1 会显示为过旧，从设置界面安装时会先删除 npm 的 `opencode-ai`。应用用 `opencode upgrade` 更新它。
- 登录：点击{{ui:settings.nav.tools}}标签页中的{{ui:settings.cliAccounts.buttonLogin}}，会打开运行 `opencode auth login` 的终端窗格。在其中选择提供商，然后用其订阅登录或输入 API 密钥。opencode 保存的登录（`opencode auth list`）或环境变量中的 API 密钥（如 `ANTHROPIC_API_KEY`）也视为已登录；两者都没有时只提供 opencode Zen 的免费模型。退出登录请在终端中运行 `opencode auth logout`。
- 模型：列表来自 `opencode models`，以“提供商/模型”显示，第一项遵循 opencode 自身的设置。
- 权限：所选模式会成为该次请求专用的 opencode 规则，规则留待询问的操作会像 Claude 一样以审批卡出现在窗格中。{{ui:permission.label.default}}在修改文件、执行命令、访问网络和访问项目外内容前询问；{{ui:permission.label.acceptEdits}}修改文件不询问；{{ui:permission.label.plan}}使用不修改文件的 opencode plan 智能体运行；{{ui:permission.label.fullAccess}}从不询问。
- 每次请求都会在窗格的工作文件夹中运行 `opencode acp`（OpenCode 2，Agent Client Protocol），并延续窗格自己的会话，会话保存在 opencode 的 `opencode.db`。{{ui:resume.choice.resume}}中也会列出该文件夹的 opencode 会话。用量中显示会话费用、正在使用的上下文和该次请求的令牌。
- 提问：opencode 通过 `question` 工具提问时会弹出[提问卡片](approvals.md#questions)，回答后同一请求继续进行。在卡片上点击{{ui:phone.questionnaire.cancel}}后，opencode 会结束该请求。
- 暂不支持：opencode 窗格不参与[Mighty 编排](delegation.md)，不能在[远程工作区](remote.md)中打开。

### Hermes {#hermes}
Hermes（`hermes`）是 Nous Research 的开源智能体 CLI（Hermes Agent），可使用 Nous Portal、OpenRouter、Anthropic、OpenAI 等多家模型提供商。

- 安装：在{{ui:setup.title}}界面安装（没有 Git 时会先安装）。官方安装脚本会连同自带的 Python 环境一起装到 `~/.hermes`，并下载浏览器工具，需要几分钟。也可以自行安装：在 Mac 上运行 `curl -fsSL https://hermes-agent.nousresearch.com/install.sh | bash`，在 Windows 上于 PowerShell 中运行 `irm https://hermes-agent.nousresearch.com/install.ps1 | iex`。更新由应用通过 `hermes update` 完成。
- 登录：点击{{ui:settings.nav.tools}}标签页中的{{ui:settings.cliAccounts.buttonLogin}}按钮，会打开运行 `hermes model` 的终端窗格。在其中选择模型提供商，登录或输入 API 密钥，然后选择默认模型。应用只读取 Hermes 找到凭据的提供商名称。要删除凭据，请在终端使用 `hermes auth`。
- 模型：第一项跟随 Hermes 自身的设置（通过 `hermes model` 设定的默认模型）。Hermes 只有打开会话后才给出模型列表，因此发送第一个请求后才会出现 `提供商:模型` 列表。
- 权限：{{ui:permission.label.default}}在每次修改文件前用窗格中的批准卡片询问；{{ui:permission.label.acceptEdits}}不询问工作文件夹内的修改；{{ui:permission.label.fullAccess}}即 Hermes 的 `--yolo`，危险命令也不询问。危险命令（如 `rm -rf`）按 Hermes 的审批设置（`approvals.mode`）用卡片询问，其他命令和读取不询问。没有仅规划的模式。{{ui:permission.label.fullAccess}}在会话开始时确定，已开始的会话要从新对话起才生效。
- 每个请求都会在窗格的工作文件夹中运行 `hermes acp`（Agent Client Protocol），并继续该窗格的会话。实际工作由 Hermes 的网关（后台进程）完成，它按需启动，空闲时退出。会话保存在 `~/.hermes/state.db` 中，{{ui:resume.choice.resume}}中也会列出此文件夹的 Hermes 会话。用量显示会话的累计令牌数和正在使用的上下文；Hermes 不报告费用。
- 提问：Hermes 通过 clarify 工具提问时会弹出[提问卡片](approvals.md#questions)，回答后同一请求继续进行。Hermes 默认在编辑器连接（ACP）中关闭此工具，因此应用会在第一次 Hermes 请求之前，用 `hermes config set` 设置 Hermes 配置中的 `platform_toolsets.acp` 这一项来开启它，并在记录中说明。如果你自己保存过该列表，应用不会改动。从新对话开始生效；要撤销，请在终端运行 `hermes config unset platform_toolsets.acp`。Hermes 只等待回答一段时间（默认 90 秒），之后卡片会关闭，Hermes 在没有回答的情况下继续。
- 暂不支持：Hermes 窗格不参与[Mighty 编排](delegation.md)，也无法在[远程工作区](remote.md)中打开。

### 更多运行设置 {#run-settings}
点击 … 按钮会打开{{ui:settings.run.title}}。
- Claude：{{ui:settings.run.maxTurnsLabel}}和{{ui:settings.run.maxBudgetLabel}}。留空则不设限制。
- Codex：{{ui:settings.run.webSearchLabel}}设置和{{ui:settings.run.shellNetworkToggle}}。输入框中的 Fast 按钮在支持的模型和账户上回答更快，但会消耗更多用量。

更改值后，点击{{ui:settings.run.applyButton}}。

![[composer-settings]]

### 附加文件和图片 {#attachments}
点击回形针按钮、将文件拖到输入框中，或用{{kbd:⌘V}}粘贴图片。
- 最多 8 个文件，每个文件 5 MiB，总计 8 MiB。图片（PNG、JPEG、GIF、WebP）、PDF 和文本文件按原格式传递，其他文件作为二进制文件传递。
- 发送前可以预览，并逐个移除。也可以只发送附件。
- 终端窗格不能添加附件。

### 发送对象 {#ask-target}
在手机应用中，安装了 oh-my-claudecode 的 Claude 窗格，输入框里会出现{{ui:composer.ask.menuLabel}}开关。它列出已安装的其他 CLI（Codex、Antigravity 等）；选定后，之后的请求会通过 oh-my-claudecode 的 `ask` 发给它，答案会出现在 Claude 的回复中。选择{{ui:composer.ask.claude}}则像以前一样发给 Claude。Mac 和 Windows 应用的输入框没有这个开关，从那里发送的请求总是发给该窗格自己的代理。若要交给其他代理，请使用[{{ui:composer.busySend.title}}](#queue)或队列行的代理选择。
- 向其他 CLI 提问时无法发送附件。
- oh-my-claudecode 启动所选 CLI 时会关闭它的审批确认和沙盒，因此它可以不经询问就修改此文件夹中的文件并运行命令。Mighty Code 的权限模式和审批卡片只管 Claude 启动它的那一步。选择了某个 CLI 期间，输入框会用一行显示此警告。
- 在 Windows 上，旧版 oh-my-claudecode 会拒绝向 Antigravity 提问。如果收到这样的回答，请更新 oh-my-claudecode 或改用 Gemini CLI。
- 以 `/` 开头的文字仍原样发给 Claude。

### 斜杠命令 {#slash}
在输入框中输入 `/` 会显示命令列表。用{{kbd:↑}}{{kbd:↓}}选择，按{{kbd:Enter}}或{{kbd:Tab}}插入，按{{kbd:Esc}}关闭。

| 命令（以 Claude 为准） | 作用 |
|---|---|
| `/model` | {{ui:slash.builtin.model}} |
| `/permissions` | {{ui:slash.builtin.permission}} |
| `/clear` | {{ui:slash.builtin.newConversation}} |
| `/cost`、`/usage` | {{ui:slash.builtin.usage}} |
| `/plugin` | {{ui:slash.builtin.plugin}} |
| `/config` | {{ui:slash.builtin.settings}} |
| `/rename` | {{ui:slash.builtin.rename}} |
| `/help` | {{ui:slash.builtin.help}} |
| `/advisor` | {{ui:slash.builtin.advisor}} |
| `/committee` | {{ui:slash.builtin.committee}} |

Codex 和 Antigravity 的命令名称略有不同（例如 Codex 的 `/new`、`/approvals`、`/status`，Antigravity 的 `/usage`、`/config`）。列表中没有的 `/名称` 会作为 CLI 的技能、用户命令或插件命令原样传递。Antigravity 的技能也是这样运行的，列表中会显示 agy 自带的技能、`~/.gemini/antigravity-cli/skills` 中你的技能以及文件夹的 `.agents/skills`；pi 使用 `/new`、`/session`、`/settings`，opencode 和 Hermes 使用 `/new`、`/usage`、`/settings`。在任何智能体窗格中，`/help` 都由应用直接显示此窗格可用的命令。`/skills` 等 Antigravity 自身的只读命令只有在终端中以 `agy -p "/skills"` 单独运行时才会回答，因此在应用中会以 agy 的错误结束。

### 其他代理的意见 {#advisor}
`/advisor` 会把此窗格最后的计划（如有）、最后的结果以及最后一个请求改动的文件摘要发给另一个代理并取回意见。可以像 `/advisor codex 这个计划可行吗？` 这样附上代理和问题；不写代理时，会询问与此窗格不同、已安装并登录的第一个代理（按 Claude、Codex、Antigravity、pi 的顺序）。`/committee` 向两个代理提出同一个问题，并排显示两份意见，不汇总异同。

- 给出意见的代理只运行一次，且为只读（Claude 用计划模式且只有读取工具，Codex 用只读沙箱，Antigravity 用计划模式和沙箱，pi 只有读取工具）。它不会改文件、不运行命令，也不保留会话。opencode 和 Hermes 在单次运行中会自行批准，因此不能选择。
- 发送的内容会缩减大小并删除看起来像密钥的行。回答以意见卡片的形式附在窗格下方（代理、模型、已知时的费用），点 {{ui:advisor.card.insert}} 可放入输入框，请此窗格的代理采纳。费用计入用量账本中此窗格的工作区。

### 通知 {#notices}
点击窗口底部状态栏的铃铛，会在窗口右侧打开{{ui:notice.title}}，铃铛上显示未读数。顶部有三个标签。

- **{{ui:notice.tab.general}}**：工作区中的代理每完成一次任务（无论完成还是失败）都会留下一条：窗格名称和工作区、结束时间和用时、状态、代理最后所说内容的第一句，以及收到的请求。你亲自停止的运行和机器人的计划、触发运行不会留下（机器人的运行会在{{ui:notice.tab.bots}}中留下报告）。可按完成、失败筛选，按日期从新到旧排列。
- **{{ui:notice.tab.bots}}**：机器人留下的报告，与下文机器人一节的报告说明相同。
- **{{ui:notice.tab.events}}**：进入[触发器](settings.md#schedules)的最近 200 个事件。卡片上显示事件类型（type，如 `github.pr.opened`）、关于什么（subject，如 `PR#412`）、来自哪里（source）、时间和标题；有触发器运行时显示绿点，全部跳过时显示黄点。展开后，每个收到该事件的触发器显示 ✓ 已运行或 ✗ 原因（不满足的条件和实际值、机器人休眠、超出预算），并列出条件可用的字段。{{ui:automation.events.replay}}会用该事件再次运行该触发器：重新检查条件，审批和权限照常适用，已删除的触发器不会运行。顶部的{{ui:automation.events.clear}}清空列表。MCP 轮询触发器读不到服务器时（未配置、需要登录、没有该工具），原因每次变化时也会记录一次，这类记录没有{{ui:automation.events.replay}}按钮。触发器的[策略](settings.md#schedules)要求批准时，顶部的{{ui:automation.approvals.title}}一行会出现卡片（事件、规则、将运行的内容），用{{ui:automation.approvals.run}}或{{ui:automation.approvals.skip}}决定。等待中的批准数会显示在标签和铃铛上，重启后仍保留，7 天后记为过期。
- **前往**：双击卡片或点击{{ui:notice.go}}，会打开该窗格，并让图表定位到那个请求的块。窗格已关闭时会提示。机器人报告卡片也一样。
- 打开通知后，当前标签中的通知会立即标为已读；切换标签或打开期间新到的通知也一样。本次首次看到的通知会保留圆点直到关闭，方便看出哪些是新的。顶部的按钮可清除已读的通知。

### 机器人 {#bots}
机器人功能仍处于{{ui:badge.beta}}阶段：机器人标签、添加机器人向导和通知中的机器人标签带有测试版标记，其行为和界面可能会变化。这台电脑上的代理窗格都是机器人。新建窗格时会自动起一个应用语言里常见的可爱名字（如 `@豆豆`、`@团团`），其他代理、计划任务和 `mighty` 命令都可以用这个名字向它发送请求。名字和说明可在窗格标题栏的 ··· 菜单或侧边栏右键菜单的 {{ui:bot.menu.edit}} 中修改。侧边栏会在窗格标题旁显示 `@名字`，点击它会切换到 {{ui:bot.sidebar.title}} 标签并显示该机器人。机器人仍是普通窗格：批准和问题卡片、队列、手机和远程镜像照常工作。

- **侧边栏标签**：侧边栏顶部有 {{ui:sidebar.workspacesHeader}} 和 {{ui:bot.sidebar.title}} 两个标签。用 {{kbd:⌘1}}、{{kbd:⌘2}}（Windows 为 {{kbd:Ctrl+1}}、{{kbd:Ctrl+2}}）切换。应用启动时总是显示 {{ui:sidebar.workspacesHeader}} 标签。有机器人在等你时，{{ui:bot.sidebar.title}} 标签上会显示黄色数字；有机器人正在工作时，标签上的机器人图标会变成与运行中窗格相同的旋转标记。搜索会同时查找两个标签，另一个标签中有结果时会在列表顶部提示。
- **机器人标签**：这台电脑上的所有机器人（所有代理窗格）都列在 {{ui:bot.sidebar.allPanesTitle}} 列表中。有说明、计划或触发器的工作区机器人不会出现在 {{ui:sidebar.workspacesHeader}} 标签中其工作区的列表里，只在此标签中显示（下面挂着委派窗格的机器人会留在那些窗格上方）。机器人分为上方的 {{ui:bot.sidebar.otherBots}} 和下方的 {{ui:bot.sidebar.workspaceBots}} 两组，组内按 {{ui:bot.sidebar.state.needsYou}}、{{ui:bot.sidebar.state.working}}、{{ui:bot.sidebar.state.idle}}、{{ui:bot.sidebar.state.asleep}} 的顺序排列（可在 ··· 菜单中改为按状态分组），每组可单独折叠；行名前的小图标也会表明类型（独立机器人为机器人图标，工作区机器人为文件夹）。每行显示名字、工作区机器人所连接的工作区，下方一行说明它正在做什么：等待的原因、正在做的事、下次运行时间或正在休眠。点击打开它的窗格；双击或右键菜单中的 {{ui:bot.menu.showWorkspace}} 会在 {{ui:sidebar.workspacesHeader}} 标签中展开它的工作区（独立机器人除外）。
- **添加机器人向导**：点击 {{ui:bot.sidebar.title}} 标签顶部的 {{ui:bot.sidebar.add}} 打开向导。左侧列出七个步骤，用底部的 {{ui:bot.wizard.back}} 和 {{ui:bot.wizard.next}} 依次进行（完成的步骤可在左侧点击返回）。① {{ui:bot.wizard.step.template}}：按分类或搜索（模板名称、做什么、示例名称）挑选模板，下方显示它做什么、何时工作和示例。② {{ui:bot.wizard.step.prompt}}：机器人一直遵守的{{ui:bot.wizard.instructions}}和定时与触发器要发送的{{ui:bot.wizard.prompt}}已用模板的文字填好，可以当场修改。点击上方的{{ui:bot.wizard.examples}}标签，两栏都会换成该示例的文字（触发器、定时和时间不变）；如果已经写了自己的文字，会先询问是否替换。模板的所有定时和触发器都发送这个请求，下方列出请求中可用的事件值（如 `{{pr.title}}`、`{{task.key}}`、`{{event.subject}}`）。③ {{ui:bot.wizard.step.where}}：一开始选中的是在你当前工作区文件夹中工作的独立机器人（“独立机器人 · 工作区名 文件夹”），也可以改为应用数据文件夹或所选文件夹中的独立机器人，或工作区机器人。模板需要时还要填写 GitHub 仓库（工作区机器人或工作区文件夹中的独立机器人留空则使用该工作区的仓库）或要运行的命令。④ {{ui:bot.wizard.step.who}}：代理、名字（🎲 换一个）和模型（从该代理的模型列表中选）。⑤ {{ui:bot.wizard.step.when}}：顶部的一句话用平实的文字读出机器人要做的事（如“工作日 09:00，以及新 PR 打开时 → 发送请求”），每次修改都会随之更新，⑦ 也会显示这句话。下面，模板的定时与触发器和你添加的项目都以同样形状的卡片逐一列出。卡片头部写着它关注什么，右侧是 {{ui:bot.wizard.job.remove}}（×）；去掉模板的项目后，{{ui:bot.wizard.add.title}} 一行会出现“+ 该项目”的标签，可以放回。卡片正文只显示“何时”：定时可在 {{ui:bot.wizard.time.kindAt}} 和 {{ui:bot.wizard.time.kindEvery}} 之间选择，并修改时间（或间隔与时段）和星期；触发器的条件显示为简单的标签，每个值可当场修改，用 × 去掉。{{ui:bot.wizard.cond.add}} 会给出该事件类型的条件：PR、CI 和合并有 {{ui:bot.wizard.cond.base}}（仅当有值时）、{{ui:bot.wizard.cond.label}}、{{ui:bot.wizard.cond.notAuthor}}，问题有 {{ui:bot.wizard.cond.label}}，MightyWork 任务有 {{ui:bot.wizard.cond.stage}}（从待办池到完成）、{{ui:bot.wizard.cond.tag}}、{{ui:bot.wizard.cond.title}}。展开卡片的 {{ui:bot.wizard.advanced}}，可以写只属于这一项的 {{ui:bot.wizard.job.prompt}}（留空则发送 ② 的请求），以及标签无法表示的 {{ui:bot.wizard.cond.raw}}：字段（该事件类型的字段列表或 {{ui:bot.wizard.cond.fieldOther}}）、用文字表示的比较（等于、等于其中之一、包含、匹配及其相反）、值和 {{ui:automation.form.whereIfPresent}}。在下方的 {{ui:bot.wizard.add.title}} 一行中可以添加 {{ui:bot.wizard.add.schedule}} 和 {{ui:bot.wizard.add.github}}（PR、问题、发布、合并、CI 失败、评审请求；仓库留空则用工作区的仓库）；点击 {{ui:bot.wizard.add.more}} 还可以添加 {{ui:bot.wizard.add.bot}}（@名称，任意机器人用 `*`；完成或失败）、{{ui:bot.wizard.add.event}}（`mighty event emit` 的名称）和 {{ui:bot.wizard.add.mightywork}}（填写项目键后，该项目的任务变化时，起初为变为待办阶段时）。添加的触发器和模板的触发器一样使用条件标签和 {{ui:bot.wizard.advanced}}，要做的事在 ⑥ 中设定。添加的项目也发送 ② 的请求，所以空白机器人的 ② 请求为空时，每个添加的项目都要写请求。新建的机器人立即处于唤醒状态并按计划工作；要让它休眠，请点击机器人行的 {{ui:bot.menu.sleep}}。⑥ {{ui:bot.wizard.step.limits}}：权限、每日预算和 {{ui:bot.wizard.limits.rules}}。为每个触发器（只有一个时直接显示，多个时按项目分组）选择 {{ui:automation.form.do}}：{{ui:bot.wizard.do.ask}}、{{ui:bot.wizard.do.develop}}（提交 PR、合并）、{{ui:bot.wizard.do.run}}（要运行的命令）或 {{ui:bot.wizard.do.emit}}（要发送的事件），再开关 {{ui:bot.wizard.limits.approve}}（会显示默认值；运行命令和会合并的开发默认开启）和 {{ui:bot.wizard.limits.oncePer}}。同时运行数、冷却时间和每日上限在 {{ui:bot.wizard.advanced}} 中。⑦ {{ui:bot.wizard.step.confirm}}：逐行查看将要创建的内容，选择 {{ui:bot.wizard.greet}} 后点击 {{ui:bot.new.create}}。创建后会打开该机器人的窗格。
- **独立机器人**：不属于任何工作区、在自己文件夹中工作的机器人。向导创建的机器人一开始就是独立机器人，在工作位置中选择 “独立机器人 · 工作区名 文件夹”（该工作区的文件夹）、{{ui:bot.wizard.where.independent}}（应用数据文件夹中的 `bots/名字`）或 {{ui:bot.wizard.where.pick}}。独立机器人不会出现在侧边栏的工作区列表中。独立机器人在某个工作区的文件夹中工作，或它的定时任务、触发器位于某个工作区或关注某个 GitHub 仓库（`owner/name`）时，机器人标签页中它那一行的名字后面会用小标签显示该工作区或仓库（有多个时显示第一个和 +数量，其余在鼠标悬停时以“关注”显示），双击会展开该工作区。工作区机器人在其行的右键菜单中选择 {{ui:bot.menu.makeIndependent}} 即可变为独立机器人，窗格与对话、工作文件夹（原工作区的文件夹）、定时任务和触发器都照常延续。远程或 SSH 工作区的机器人，以及在自己工作树中工作的机器人不能更改。
- **10 个模板**：{{ui:bot.wizard.category.review}}（{{ui:bot.template.prReviewer}}、{{ui:bot.template.afterMerge}}、{{ui:bot.template.issueTriage}}、{{ui:bot.template.ciDevelop}}）、{{ui:bot.wizard.category.release}}（{{ui:bot.template.afterRelease}}、{{ui:bot.template.mwRelease}}、{{ui:bot.template.mwDevelop}}）、{{ui:bot.wizard.category.watch}}（{{ui:bot.template.routine}}、{{ui:bot.template.siteWatch}}）和{{ui:bot.wizard.category.other}}（{{ui:bot.template.blank}}）。每个模板的触发器或定时各不相同；在同一时机工作的相似模板成了该模板的示例：{{ui:bot.template.prReviewer}}（示例 {{ui:bot.example.tests}}）、{{ui:bot.template.afterMerge}}（示例 {{ui:bot.example.security}}、{{ui:bot.example.docs}}）、{{ui:bot.template.afterRelease}}（示例 {{ui:bot.example.releaseNotes}}）、{{ui:bot.template.routine}}（示例 {{ui:bot.example.briefing}}、{{ui:bot.example.wrapUp}}、{{ui:bot.example.weekly}}、{{ui:bot.example.releaseReady}}、{{ui:bot.example.deps}}、{{ui:bot.example.todos}}、{{ui:bot.example.cleanup}}、{{ui:bot.example.perfWatch}}、{{ui:bot.example.taskKeeper}}）、{{ui:bot.template.blank}}（示例 {{ui:bot.example.guide}}）。示例只改说明和请求，触发器和定时仍用模板的。{{ui:bot.template.routine}} 在工作日 9:00 工作，时间在{{ui:bot.wizard.step.when}}步骤中修改。把 GitHub 模板建成独立机器人时需要仓库（`owner/name`），{{ui:bot.template.siteWatch}} 需要要运行的检查命令（请求中写作 `{{check.command}}`）。{{ui:bot.example.perfWatch}} 示例把基准测试命令写在请求里（`cargo bench`），请改成你的仓库的命令。名字留空则起一个可爱的名字。MightyWork 模板需要项目键（如 `MC`）。三个配方见下文[配方](#recipes)。
- **何时工作**：机器人设置表单列出它的计划和触发器及下次运行时间。{{ui:bot.dialog.addJob}} 会保存设置，并打开已填好发给此机器人的[计划和触发器](settings.md#schedules)表单。改名时，发给它的计划和触发器也会跟着改。侧边栏机器人行显示 ⏰ 和下次运行时间。
- **权限和预算**：在机器人设置表单中设置它运行时使用的权限和每日预算（美元）。预算用完后，当天剩余的计划和触发器会跳过，其他代理的请求也会被拒绝。整体每日预算仍然有效。
- **休眠**：用设置表单中的复选框或侧边栏机器人行的 {{ui:bot.menu.sleep}} 让它休眠。休眠的机器人显示 💤 并变暗；计划和触发器会跳过，其他代理的 `ask_bot` 和 `mighty agent send @名字` 会被拒绝。直接在它的输入框发送的请求照常接收。用 {{ui:bot.menu.wake}} 唤醒。 休眠期间跳过的计划和触发器只留在运行记录中，不生成报告。你亲自发起的运行（例如报告箱或运行记录中的 {{ui:automation.history.retry}}）休眠的机器人也会接收。
- **报告**：机器人完成计划、触发器或其他机器人交给它的工作后会留下一份报告。状态有 ✅ {{ui:bot.report.status.ok}}、📝 {{ui:bot.report.status.report}}、⚠️ {{ui:bot.report.status.needs}}、❌ {{ui:bot.report.status.failed}} 四种，附一行摘要、最多三个条目（含链接）、触发来源、耗时和费用。机器人用 mighty-terminal 工具 `bot_report` 来写；没写时，应用用最后回答的第一句和其中的链接生成（`NO_REPLY` 或空回答为 ✅），没能运行或失败的运行会以原因生成 ❌ 报告。同一机器人再次给出相同结果时，不新建卡片，只增加次数。
  - **报告箱**（{{ui:notice.title}}的{{ui:notice.tab.bots}}标签）：用 {{ui:bot.sidebar.title}} 标签顶部的 {{ui:bot.report.button}} 按钮（显示未读数）或机器人行菜单中的 {{ui:bot.report.menu}} 在窗口右侧打开。可按状态和机器人筛选；按日期排列，需要你和失败的排在前面，当天的 ✅ 折叠为一行。每张卡片有 {{ui:bot.report.open}}（需要你时为 {{ui:bot.report.answer}}）、{{ui:bot.report.followUp}}、{{ui:bot.report.rerun}} 和 👍/👎。按 Esc 关闭。
  - **机器人行**：空闲或休眠的机器人行下方显示最后一份报告（状态、时间、摘要），未读时带一个圆点。
  - **通知**：只有 ⚠️ 和 ❌ 发送桌面通知和手机通知（相同结果重复时只通知第一次）。
  - **文件**：报告还会按天写入应用数据文件夹中的 `bots/名称/reports/日期.md`。
- **由代理和命令创建机器人**：Claude、Codex 窗格中的代理可以把机器人规格（JSON）交给 mighty-terminal 工具 `create_bot`、`update_bot` 来提议机器人；`mighty bot new --spec 文件` 也一样。应用会在批准表单中显示提议方、名字、代理、权限、工作位置、预算以及计划和触发器，只有你按下 {{ui:bot.approve.create}} 才会创建。

```json
{
  "name": "pr-keeper",
  "agent": "codex",
  "permission": "acceptEdits",
  "instructions": "审查打开的 PR。",
  "budgetPerDay": 3,
  "schedules": [{ "cron": "0 9 * * 1-5", "prompt": "总结昨天以来的 PR" }],
  "triggers": [{ "on": { "type": "github", "events": ["pr_opened"], "repo": "owner/repo" }, "prompt": "请审查 {{pr.url}}" }]
}
```

```sh
mighty bot list                       # 机器人、计划和触发器、下次运行
mighty bot new --spec bot.json        # 显示批准表单（bot update：修改已有机器人）
mighty bot pause @pr-keeper           # 休眠（resume：唤醒）
mighty bot run @pr-keeper --job daily # 立即运行机器人的计划或触发器
```
- **名字**：1 到 32 个文字（任何语言，包括中文）和数字，中间可用 `-`、`_`、`.`（不能有空格）。不区分大小写：`@Reviewer` 和 `@reviewer` 是同一个机器人。同一配置中一个名字只能由一个窗格使用。
- **说明**：机器人负责什么、如何工作（最多 16 KB）。每次开始新对话时，说明会附在第一个请求之后，只发给代理；屏幕上的请求块只显示你发送的请求。修改后的说明从下一次新对话开始生效。
- **调用方法**：用 `@名字` 代替窗格 id。
  - `mighty agent send @reviewer "看看 PR 12"`、`mighty agent wait @reviewer --until done`、`mighty pane read @reviewer`
  - 计划任务的 heartbeat：`pane: "@reviewer"`。可以调用其他工作区的机器人；没有该名字的机器人时不会新建窗格，而是记录为失败。
  - 其他代理（Claude、Codex 窗格）：mighty-terminal 工具 `list_bots`、`ask_bot`、`bot_answer`。`ask_bot` 发送后立即返回；`bot_answer` 返回状态和最后的回答（隐去机密；`wait: true` 时最多等待 20 秒）。
- **安全规则**
  - 请求始终按机器人窗格自己的设置和权限运行。调用方无法扩大权限。
  - 机器人不能向自己发送请求；正在处理其他机器人交给它的请求时，也不能调用机器人（只能调用一层）。被拒绝时，`ask_bot` 会在 `refused` 中返回原因。
  - 当天预算用完后不再发送。

### 配方 {#recipes}
新建机器人向导中的三个模板会整体建好一个触发器（来源、条件、动作和策略）。在{{ui:bot.wizard.step.when}}步骤中，可用项目的条件标签和{{ui:bot.wizard.advanced}}里手动填写的条件修改条件；在{{ui:bot.wizard.step.limits}}步骤中修改{{ui:automation.form.do}}和策略（批准、只做一次等）；建好之后在[计划与触发器](settings.md#schedules)表单中修改。

- **{{ui:bot.template.ciDevelop}}**：接收 GitHub CI 失败（`ci_failed`）中 `base eq main`（仅当有值时）的事件并执行 `develop`：机器人的子窗格在新 worktree 中阅读日志、修复、测试、提交并打开 PR。因为 `oncePer: subject`，同一次运行（`run-123`）只处理一次。
- **{{ui:bot.template.mwRelease}}**：从 MightyWork 预设的 MCP 轮询（向导中填写项目键）接收 `mightywork.task.stage_changed` 中 `task.to eq todo` 且 `task.title matches ^v\d+\.\d+\.\d+ 릴리스` 的事件，用 `run` 运行 `/release {{task.version}}`。因为 `approve: true`，要在通知箱中按{{ui:automation.approvals.run}}才会开始；因为 `oncePer: subject`，同一个任务只询问一次。机器人指令要求开始时用 MightyWork MCP 工具把任务改为 `in_progress`，结束后留下评论并改为 `done`。
- **{{ui:bot.template.mwDevelop}}**：从同样的轮询接收 `mightywork.task.stage_changed` 中 `task.to eq todo` 且 `task.tags has bot:dev` 的事件并执行 `develop`。信号是移到待办的那一刻，而不是加标签的那一刻，所以只加了标签的任务还不会开始。机器人指令：用 `get_task` 读取需求，改为 `in_progress`，实现并测试后打开 PR，在评论中留下摘要和 PR 链接，再改为 `review`。

策略的作用：
- **批准**：需要批准的动作不会马上开始，而是以卡片形式在通知箱 [{{ui:notice.tab.events}}](#notices) 标签顶部等待。卡片显示事件、规则和将运行的内容（`run · /release v0.6.4 · @release`），并通知系统和手机一次。重启后仍保留，7 天未决定则记为过期。
- **每个对象只一次**：同一个 subject（`PR#12`、`MC-7`、`run-123`）不会再次运行，事件标签中显示“已处理过”。未能开始的运行会被忘记，下一个事件会再试。
- **同时运行**：`develop` 和 `run` 默认一次只运行一个，期间到来的事件会等待，前一个结束后再开始。
- **事件标签**：每个触发器都会显示 ✓ 已运行、⏳ 等待或等待批准、✗ 及原因，可以看到为什么运行或没有运行。你亲自按的{{ui:automation.events.replay}}、{{ui:automation.history.retry}}和立即运行不经过批准和策略。

### 上下文与用量 {#session-info}
发送按钮旁的圆环显示对话已使用了多少上下文。点击它可以查看{{ui:composer.sessionInfo.contextUsage}}、输入/输出/缓存 token、费用、所用模型和会话 ID。只显示 CLI 报告的值，不会估算未知的值。

![[session-info]]

## Mighty 视图 {#views}
所有智能体窗格（Claude、Codex、Antigravity、pi、opencode、Hermes）都以{{ui:graph.view.mighty}}视图显示：以块状流程显示请求、子智能体、后台任务和结果。没有需要切换的地方。

### Mighty 流程 {#diagram}
Claude 窗格的流程是：请求 → 中间发生的事 → {{ui:graph.block.result}}。请求块只包含你发送的内容。在答复到来之前，结果的位置上会有一张{{ui:graph.block.execution}}卡片，显示主会话的模型和它现在在做的事（Claude 最后说的话，或之后运行的命令）。点击卡片会展开包含全部消息和工具调用的{{ui:graph.block.execution}}块；答复到来后，{{ui:graph.block.result}}卡片会出现在那个位置。中间没有任何事情的简短提问只有请求和结果两个块。其他智能体的窗格仍然在请求块正下方接着{{ui:graph.block.execution}}块。

块的标题栏用最前面的状态点和标题显示当前状态。点的颜色表示状态（运行中为蓝色，等待你回应为黄色，完成为绿色，出错为红色，已停止为灰色）。{{ui:graph.block.result}}的标题也会染成该颜色，让已结束的结果一目了然。是谁在工作（主会话、子智能体、后台命令）由类型标记的颜色显示，把鼠标放在点上会告诉你角色。标题后面是子智能体的类型（Explore、你自己创建的智能体名称等）以及后台、分叉、worktree 标记，{{ui:graph.block.execution}}块还会显示窗格中选择的思考强度。模型与令牌数一起显示；还没有响应时，在请求的模型后面标注“请求”。如果你创建的智能体的定义文件里有 `color`，就使用该颜色。块较窄时这些标记会先隐藏，把鼠标放在点上可以看到全部。

在 Claude 窗格的流程中，除结果以外的内容都会折叠成小卡片。子智能体和后台命令无论在运行还是已结束都是卡片，卡片和块有相同的标题栏（图标、标题、状态、展开按钮），下面显示两行：现在在做的事（结束后为答复的第一行），以及类型和模型、已用时间、工具调用次数和令牌。只显示应用实际看到的值，没有进度百分比。点击卡片会展开为完整的块，用块标题栏上的{{ui:graph.card.fold}}按钮可以再次折叠。点击已结束的后台子智能体卡片，会展开{{ui:graph.background.noticeRun}}图中的那个块。在后台运行的命令（例如等待 CI、构建应用、开发服务器）也会以灰色卡片站在同一行，显示说明和命令、该命令最后输出的一行，以及运行时间或结束状态和退出码。

Claude 列出待办清单后，请求下方会按顺序为每个已开始的步骤显示一张卡片（第 1 步 → 第 2 步 → …）。在某个步骤期间启动的子智能体和命令显示在该步骤右侧，卡片上会多一行，例如 `↳ 2. 撰写说明`，表明属于哪个步骤。点击步骤卡片会展开该步骤期间的记录。没有步骤的请求中，子智能体并排在请求下方的一行，上方的分支说明把工作交给了多少个、用哪个模型，以及还有多少个在运行。应用会让 Claude 在每个使用工具的请求开始时先列出待办清单，所以除简短回答外，大多数请求都会有步骤。

在 Claude 窗格的流程下方、输入框上方，有一条{{ui:graph.log.title}}。它的标题行是当前状态：主会话的模型和状态、它的子智能体中有多少个在运行，以及有多少个后台命令在运行，各用其角色的颜色显示。展开后，它会列出最新请求中最近发生的五件事：时间、是谁（主会话的模型或子智能体的名称）、做了什么（工具调用、子智能体的开始和结束、提问、{{ui:graph.block.compact}}、通知到达）。点击某一行，流程会移到对应的块。用右端的按钮可以把它折叠成只剩状态行，每个窗格会记住是否折叠。

在后台启动的子智能体和后台命令（任务）在运行期间不会画成请求的块，而是在下面{{ui:graph.progress.title}}面板的{{ui:graph.progress.background}}组里。在 Claude 窗格的流程中，它们还会显示为上面的卡片。任务结束、通知送达 Claude 后，Claude 的每一次回答都会生成一个新的{{ui:graph.background.noticeRun}}图表，每个已结束的任务一个块：类型行（子智能体还是后台命令，以及结束状态）、命令、输出的最后几行，以及子智能体的报告。在{{ui:graph.view.timeline}}中，这个图表是新增的一行。手机上的 Mighty 视图也一样。

每个智能体窗格的流程左上角都有{{ui:graph.progress.title}}面板。它显示画面中请求的编号、状态和内容；智能体告知计划时，还会显示每个步骤是已完成、进行中（时间在走）还是未开始。步骤来自 Claude 的待办清单、Codex 的计划（`update_plan`）、opencode 和 Hermes 的计划以及 Gemini 的清单（task.md）。{{ui:menu.settings}} › {{ui:settings.nav.general}} 中的 {{ui:settings.agents.checklistToggle}}（默认开启）会请 Gemini、Codex、opencode 和 Hermes 在工作时留下计划。没有收到计划的请求，以及没有计划功能的 pi，会把智能体正在做的事（读取、写入、修改文件，命令，搜索，子智能体）逐行显示为步骤。如果请求启动了后台任务（命令、后台子智能体），底部的{{ui:graph.progress.background}}组会显示正在运行的数量和总数，并为每项显示一行（状态和耗时）：正在运行的排在前面，最多 100 行，可在面板内滚动查看，超出部分以数量显示。新请求开始时显示该请求；你自己滚动后，改为显示画面中的请求。最终结果到达时，流程会平滑地移到那个{{ui:graph.block.result}}块；如果那时你正在自己移动流程，或正在移到其他块，则保持不动。点击请求行会移到该请求，点击步骤行会移到该步骤的卡片（只有 Claude 窗格的流程有步骤卡片）。用标题栏的按钮折叠后，只留下标题和请求行。

- 拖动空白处或滚动即可移动流程。用缩放按钮可以在 50% 到 150% 之间调整。
- 点击某个块后，只在该块内部滚动。点击空白处或按{{kbd:Esc}}即可取消选择。
- 拖动块的右下角可以调整大小。右键点击可以选择{{ui:graph.block.resetSize}}。大小按会话保存。
- 滚动到最顶部后，可以用{{ui:graph.history.load}}每次加载 10 个之前的请求。加载的记录仅供查看。
- 在 Claude 和 Codex 窗格中，用标题栏中的拼图按钮可以打开已安装的插件和插件市场。

![[agent-mighty]]

![[agent-mighty-overview]]

### 记录 {#timeline}
用 Mighty 视图顶部的{{ui:graph.view.diagram}} | {{ui:graph.view.timeline}}切换开关，可以按时间顺序以列表形式查看请求。每个请求行之后是它的{{ui:graph.block.execution}}行，然后是子智能体行。

![[agent-timeline]]

### 结果卡片 {#result}
请求结束后，结果卡片会整理出结果。Claude 的回答结束后请求即变为完成。如果它启动的后台任务（包括给已结束的子智能体发消息而重新唤醒的）仍在运行，或已结束但 Claude 还没有回答它的通知，结果卡片会变成等待色的{{ui:graph.block.awaitingNotice}}块：在目前的回答下面，每个任务显示{{ui:plan.background.awaitingNotice}}或{{ui:plan.background.noticeDelivered}}，并显示“通知 1/2”这样的已收到通知数。在此期间，Mighty Pet 和窗格标题显示{{ui:graph.state.awaitingNotice}}。所有通知都得到回答后，它会变为{{ui:graph.block.result}}。Claude 收到后台任务的通知后再次回答时，这段回答不会接在请求的结果后面，每一次回答都会打开一个新的图表（[后台任务](approvals.md#background)）。如果同一请求的其他后台任务仍在运行，这段回答的卡片标题会显示“目前的结果 · 正在等待 N 个后台任务”，告诉你还剩几个。如果结果中提到了文件，可以用{{ui:graph.resultFiles.openButton}}按钮打开文件列表。点击块中的文件路径会弹出预览。最新的结果卡片会适应窗格宽度，高度随结果内容增长，因此较长的结果也无需在卡片内滚动，向下滚动流程即可读完；如果你手动调整过大小，会出现{{ui:graph.result.fitToWindow}}按钮。 如果是请求更改过的源代码或 Markdown 文件，预览标题处会出现 {{ui:files.changes.show}} | {{ui:files.changes.full}} 切换，并先显示{{ui:files.changes.show}}：只显示更改的行及其前后 3 行，旁边标有旧行号和新行号，添加的行为绿色背景，删除的行为红色背景。源代码按语言着色，行内更改的词会单独高亮。上方一行的{{ui:files.changes.unified}} | {{ui:files.changes.split}}切换可在单栏显示和新旧内容左右并排显示之间切换（会记住所选显示方式）。完整视图中的源代码也以同样方式着色。上方一行说明比较的对象：从结果文件列表或区块中打开时，是该请求第一次更改文件之前的内容（{{ui:files.changes.baseRun}}），没有该原始内容时则是最后一次提交。没有更改时不显示切换，只显示整个文件。

同一请求的后台任务之后 Claude 再次回答时，前一个结果卡片的标题变为{{ui:graph.block.resultPartialPlain}}，并附上开启该回答的任务名称(例如"中间结果 · Mac 构建")。只有请求真正的最后一个结果才是{{ui:graph.block.result}}。最后的结果卡片附有{{ui:graph.resultFiles.rollUpTitle}}列表。应用在请求开始时、每个清单步骤完成时、后台任务的回答结束时以及运行结束时为工作文件夹拍下 git 快照，并按步骤汇总其间更改的文件。无论代理是用编辑工具还是用命令修改，都一样；Codex、Gemini、opencode、Hermes 和 pi 窗格也一样(步骤就是各个工具调用的窗格，整个运行算作一组)。用{{ui:graph.rollUp.bySteps}} | {{ui:graph.rollUp.byFiles}}在按步骤分组和按文件列表之间切换。分组达到五个或以上时只展开最后两个，每组最多显示五行，其余用“再显示 N 个”展开。同一文件夹有三个或以上文件时归到文件夹名下，文件超过二十个时出现搜索框。点击文件名打开预览；点击右侧的 +/− 数字，会在下方显示该步骤对该文件的更改。回答中提到的文件接在其后。快照只在 git 仓库内的文件夹中拍摄，不会向仓库的 `.git` 写入任何内容，而是保存在应用数据文件夹的 `snapshots` 中，14 天未使用的会被删除。

![[result-card]]

### 验证凭据 {#verify-receipt}
经过[回合结束验证](delegation.md#verify)的请求，其结果卡片下方会附上凭据：通过或失败以及第几次检查、失败时停止的原因、每个命令是否通过及耗时（失败时为退出码或超时）、浏览器检查的页面数、控制台错误、异常和失败请求数，以及截图按钮。修复期间它会移到最后一个修复请求的结果卡片上。

- {{ui:verify.card.autoRepair}}：关闭后，此窗格失败时只记录结果，不请求修复。
- {{ui:verify.card.runAgain}}：立即检查一次（不请求修复）。检查期间按钮变为 {{ui:verify.card.stop}}，按下后会在当前命令结束后停止。
- 尚未检查的最新结果上有 {{ui:verify.card.run}} 按钮。手机的窗格画面也会用一行显示最新的验证结果。

### 审计记录 {#audit}
点击结果卡片标题上的 {{ui:turnAudit.open}} 按钮（或右键点击标题打开的菜单），可以按时间顺序逐行查看该请求做了什么。在自动编辑模式下长时间运行后，可用它确认做了哪些事。

- 会列出执行的命令、编辑的文件以及请求结束时已更改的文件（新增和删除的行数）、打开的地址和网络请求、计划卡片的决定和被拒绝的工具请求、子代理和委派产生的子窗格，以及[验证](#verify-receipt)结果。子代理做的事会附上其名称。
- 读取和搜索不列入列表，只在下方注明数量。可以用上方的按钮按类型筛选。
- 它只根据应用已保存的运行记录生成，不会新记录任何内容。因此未经询问就被允许的工具调用不会显示为审批行，只有被拒绝的请求和计划卡片的决定会显示为审批行。
- {{ui:turnAudit.copy}} 按钮或菜单中的 {{ui:turnAudit.copyMarkdown}} 会把列表复制为 Markdown。复制时会移除机密值和主文件夹路径。

### 审阅请求更改 {#request-changes}
Claude 窗格中的请求用 Edit、Write 或 MultiEdit 修改文件时，会保存每个文件修改前的内容。这样的请求结束后，结果卡片的标题栏会出现 {{ui:runChanges.openButton}} 按钮，点击即可打开 {{ui:runChanges.title}}：左侧是该请求修改的文件及增删行数，右侧是所选文件与请求前内容相比的更改。

- 点击更改中的某一行即可在该行留下意见。点击“将 N 条意见放入输入框”后，这些意见会带上文件和行位置，作为一条后续请求追加到输入框已有内容之后，不会立即发送。
- 勾选文件，点击“撤销所选 N 个”并确认后，这些文件会恢复为请求前的内容。请求新建的文件会移到废纸篓（Windows 上为回收站）。文件保留当前的编码和权限；请求之后你手动做的修改也会一并丢失。
- 如果之后的请求也改了同一文件，需要先撤销那个请求。已不存在、无法作为文本读取（超过 1 MiB 或二进制）或已与请求前相同的文件不会被撤销。
- 每处更改(hunk)的标题栏都有 {{ui:hunks.accept}} 和 {{ui:hunks.discard}}。{{ui:hunks.discard}} 只把这一处还原为请求前的内容，其他行保持不变。文件保留编码、换行符(CRLF 或 LF)、末尾换行与否和权限。放弃到最后一处、文件与请求前相同时，效果与撤销整个文件相同：请求新建的文件会移到废纸篓。{{ui:hunks.accept}} 不改动文件，只标记为已审阅，重启应用后仍会保留。已接受的部分会变淡并折叠，点击标题栏可再次展开。
- 左侧列表显示每个文件已处理的部分数(n/m)；所有部分都已接受或放弃的文件显示为 {{ui:hunks.reviewed}}(✓)。{{ui:hunks.acceptRest}} 接受所有尚未处理的部分。所有文件都审阅完后，该请求会从[审阅队列](layout.md#review)中移除。
- 审阅界面获得焦点时，`]`、`[` 跳到下一处、上一处，`a` 接受，`x` 放弃。
- 请求之后这部分又被修改时不会放弃({{ui:hunks.refusalShort.conflict}})。如果只是上方加了几行导致位置偏移，会找到该位置并放弃。之后的请求改了同样的行时只阻止这一处，离得远的部分仍可放弃。之后的请求没能保存该文件的原始内容时，整个文件都会被阻止。
- worktree 窗格、子窗格和对比运行的更改视图也使用相同的按钮和按键。那里以分支与基准分支分叉处提交中的内容为基准。
- 每个窗格保存最近 20 个请求的原始内容。Shell 命令修改的文件、其他代理的窗格以及远程工作区的窗格没有此按钮。
- 右键单击更改中的某一行，会在外部编辑器中打开该文件的这一行；标题栏的按钮({{ui:editor.open}})打开所选文件([在编辑器中打开](terminal-files-browser.md#open-in-editor))。
