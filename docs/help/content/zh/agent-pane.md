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

### 机器人 {#bots}
这台电脑上的代理窗格都是机器人。新建窗格时会自动起一个应用语言里常见的可爱名字（如 `@豆豆`、`@团团`），其他代理、计划任务和 `mighty` 命令都可以用这个名字向它发送请求。名字和说明可在窗格标题栏的 ··· 菜单或侧边栏右键菜单的 {{ui:bot.menu.edit}} 中修改。侧边栏会在窗格标题旁显示 `@名字`，点击它会在下方机器人列表中展开并显示该机器人。机器人仍是普通窗格：批准和问题卡片、队列、手机和远程镜像照常工作。

- **机器人列表**：侧边栏工作区列表下方的 {{ui:bot.sidebar.title}} 组把所有机器人分成两类列出：工作区窗格的机器人在 {{ui:bot.sidebar.workspaceBots}} 下，独立机器人在 {{ui:bot.sidebar.otherBots}} 下，每一类都可以单独折叠。每行显示状态和名字，工作区机器人会在名字后面显示所连接的工作区，下方显示窗格标题（独立机器人显示 {{ui:bot.sidebar.independent}}），点击即可打开它的窗格，双击会选中该机器人所属的工作区并在上方列表中展开（独立机器人除外）。该组可以折叠。
- **独立机器人**：不属于任何工作区、在自己文件夹中工作的机器人。点击 {{ui:bot.sidebar.title}} 组的 +，设置名字、代理和说明。{{ui:bot.new.create}} 在应用数据文件夹中的 `bots/名字` 文件夹里工作；{{ui:bot.new.pickFolder}} 在你选择的文件夹里工作。独立机器人不会出现在侧边栏的工作区列表中。
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

在后台启动的子智能体和后台命令（任务）在运行期间不会画成请求的块，而是在下面{{ui:graph.progress.title}}面板的{{ui:graph.progress.background}}组里。在 Claude 窗格的流程中，它们还会显示为上面的卡片。任务结束、通知送达 Claude 后，Claude 的每一次回答都会生成一个新的{{ui:graph.background.noticeRun}}图表，每个已结束的任务一个块：类型行（子智能体还是后台命令，以及结束状态）、命令、输出的最后几行，以及子智能体的报告。在{{ui:graph.view.timeline}}中，这个图表是新增的一行；在{{ui:graph.view.structure}}中，它是末尾新增的一张卡片（看板）。手机上的 Mighty 视图和流水线也一样。

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

### 流水线 {#structure}
在同一个切换开关中选择{{ui:graph.view.structure}}，可以把请求看成看板式的流水线。
- 视图一次只显示一项：没有风格的窗格显示一个请求，使用风格的窗格显示一条流水线。流水线是阶段只向前推进的一串请求；某个请求的阶段比前一个请求退回到更早的阶段时，就开始一条新流水线。
- 用顶部的 ◀ {{ui:graph.structure.board.previous}}和 ▶ {{ui:graph.structure.board.next}}前后切换。点击两者之间的标题（如"请求 3 / 7"）会打开列表，显示每一项的编号、标题、状态和阶段，可直接选择。
- 下方显示状态（使用风格的窗格还有流水线名称）。各阶段显示为每一列顶部的标签，标签之间用线相连。没有风格的窗格是{{ui:graph.structure.column.request}} → {{ui:graph.structure.column.execution}} → {{ui:graph.structure.column.result}}，使用风格的窗格是该模式的阶段。
- 内容放在并排的列中，每个阶段一列，列头显示阶段标签和卡片数。没有风格时，{{ui:graph.structure.column.request}}列放请求，{{ui:graph.structure.column.execution}}列放显示进度（最新消息和工具调用）与状态的卡片，后面每个子代理一张卡片，{{ui:graph.structure.column.result}}列放结果。使用风格时，每个阶段列放该阶段的请求卡片（请求、执行摘要和一行结果），其子代理卡片放在下方。嵌套的子代理会缩进，并注明它在哪个块之下。
- 颜色也会表示状态。每一列的背景铺上该阶段状态色的浅色（完成为绿色，运行中和等待通知为蓝色，等待你回应为黄色，出错为红色，已停止为灰色），当前阶段跟随其最新请求的状态，尚未进行的阶段保持原来的背景。每张卡片状态文字前的点是同样的颜色，背景比列的颜色更深；运行中的卡片保留流动的边框，等待中的卡片保留黄色边框，状态文字照常显示。
- 长文本默认折叠，点击{{ui:graph.structure.more}}即可就地展开。点击卡片标题栏中的小按钮，会在流程中显示该块。
- 视图从最新的一项打开，停留在那里时会随新的请求或流水线前进。查看较早的一项时出现新项，位置保持不变，▶ 旁会出现{{ui:graph.structure.board.newRequest}}或{{ui:graph.structure.board.newPipeline}}标记，点击即可转到最新的一项。
- 每一列各自移动：列头留在顶部，只有卡片上下移动。在某一列上用触控板或鼠标滚轮上下滚动，或拖动其中的空白处，只会移动这一列；列放不下时，横向滚动或拖动即可左右移动流水线。
- 在最新一项上，停在末尾的列会跟随新内容向下。自己移动过的列，或展开了文字的列，会停在原处，回到该列末尾后再继续跟随。来了新请求时，每一列都会显示末尾；之前的项打开时，每一列都从顶部开始。Windows 应用中整个流水线仍然一起移动。

![[agent-structure]]

### 结果卡片 {#result}
请求结束后，结果卡片会整理出结果。Claude 的回答结束后请求即变为完成。如果它启动的后台任务（包括给已结束的子智能体发消息而重新唤醒的）仍在运行，或已结束但 Claude 还没有回答它的通知，结果卡片会变成等待色的{{ui:graph.block.awaitingNotice}}块：在目前的回答下面，每个任务显示{{ui:plan.background.awaitingNotice}}或{{ui:plan.background.noticeDelivered}}，并显示“通知 1/2”这样的已收到通知数。在此期间，Mighty Pet 和窗格标题显示{{ui:graph.state.awaitingNotice}}。所有通知都得到回答后，它会变为{{ui:graph.block.result}}。Claude 收到后台任务的通知后再次回答时，这段回答不会接在请求的结果后面，每一次回答都会打开一个新的图表（[后台任务](approvals.md#background)）。如果同一请求的其他后台任务仍在运行，这段回答的卡片标题会显示“目前的结果 · 正在等待 N 个后台任务”，告诉你还剩几个。如果结果中提到了文件，可以用{{ui:graph.resultFiles.openButton}}按钮打开文件列表。点击块中的文件路径会弹出预览。最新的结果卡片会适应窗格大小；如果你手动调整过大小，会出现{{ui:graph.result.fitToWindow}}按钮。 如果是请求更改过的源代码或 Markdown 文件，预览标题处会出现 {{ui:files.changes.show}} | {{ui:files.changes.full}} 切换，并先显示{{ui:files.changes.show}}：只显示更改的行及其前后 3 行，旁边标有旧行号和新行号，添加的行为绿色背景，删除的行为红色背景。源代码按语言着色，行内更改的词会单独高亮。上方一行的{{ui:files.changes.unified}} | {{ui:files.changes.split}}切换可在单栏显示和新旧内容左右并排显示之间切换（会记住所选显示方式）。完整视图中的源代码也以同样方式着色。上方一行说明比较的对象：从结果文件列表或区块中打开时，是该请求第一次更改文件之前的内容（{{ui:files.changes.baseRun}}），没有该原始内容时则是最后一次提交。没有更改时不显示切换，只显示整个文件。

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
