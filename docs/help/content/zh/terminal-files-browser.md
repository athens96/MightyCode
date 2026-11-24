---
title: 终端和文件窗格
order: 7
section: terminal-files-browser
---
可以在智能体窗格旁同时打开终端和文件窗格。

## 终端窗格 {#terminal}
在工作区文件夹中打开的真实终端。
1. 选择{{ui:menu.addTerminalPane}}（{{kbd:⌘T}}），或选择{{ui:workspace.addPane}} → {{ui:workspace.newTerminal}}。
2. 直接输入命令。

Shell 退出后，请点击{{ui:terminal.restart}}。之前的命令记录可以在窗格标题栏 … 菜单的{{ui:pane.menu.terminalHistory}}中查看，右键点击窗格标题栏后的{{ui:terminal.history.title}}也一样。退出应用后，终端不会重新打开。

![[terminal-pane]]

### 通知与响铃 {#terminal-signals}
终端窗格里的程序或应用不解析的 CLI 发送通知或响铃时，会在该窗格上标记。通知支持 OSC 9（`printf '\e]9;构建完成\a'`）、OSC 777（`notify;标题;正文`）和 OSC 99（kitty，仅标题和正文）。报告进度的 `9;4;…` 不算通知。正文去掉控制字符后最多显示 200 个字符，每个窗格每秒只接收一条。
- 当你在看其他窗格或应用在后台时，侧边栏中该窗格的行和标签页会出现圆点，窗格边框会高亮。如果是通知，还会发送系统通知；响铃只做标记。
- 智能体窗格等待批准或回答问题时，以及 worktree 的 setup 失败时，也会出现同样的标记。{{ui:phone.dashboard.title}}会显示等待中的窗格数量，点击即可前往。
- 按 {{kbd:⇧⌘U}}（Windows：{{kbd:Ctrl+Shift+U}}）会前往最近发出提醒的窗格。再按一次前往下一个。菜单栏{{ui:menu.workspace}}菜单中的{{ui:attention.next}}也一样。
- 查看该窗格后标记会消失。你正在看的窗格的通知既不标记也不发送系统通知。
- 如不想接收，请在[设置](settings.md#companion)中关闭{{ui:terminalNotifications.toggle}}。

### 打开的端口与 PR {#pane-ports}
每 10 秒读取一次窗格的 shell、智能体、run 操作以及其下运行的程序打开的 TCP 端口，并显示在侧边栏的窗格行和 worktree 栏中。仅在应用窗口可见时读取当前工作区的窗格。点击 worktree 栏中的端口会在浏览器中打开 `localhost`。

worktree 窗格还会显示该分支 PR 的编号、状态（打开、草稿、已合并、已关闭）和检查结果（通过、失败、进行中）。PR 每 2 分钟重新读取一次，点击会打开 PR 页面。侧边栏较窄时，端口和 PR 只显示图标。

### 智能体终端 {#agent-terminal}
当 Claude 或 Codex 运行需要你查看的命令（开发服务器、登录等）时，智能体窗格右侧会出现专用的终端窗格。你可以直接向运行中的命令输入内容。关闭该窗格后，命令仍会继续运行，直到退出应用；可以用窗格标题栏中的{{ui:agentTerminal.terminalPane.open}}按钮重新打开。

## 文件窗格 {#files}
用于浏览工作区文件的只读窗格，不会修改文件。
1. 选择{{ui:menu.showFiles}}（{{kbd:⇧⌘E}}），或点击工作区标题中的文件夹按钮。
2. 在左侧列表中选择文件。用{{ui:files.tree.filter}}查找名称，用方向键和{{kbd:Return}}移动。
3. 在右侧查看内容。

| 文件 | 显示方式 |
|---|---|
| 源代码、文本 | 显示行号和语法高亮。大文件只显示前 1 MB。 |
| Markdown | 在{{ui:files.markdown.rendered}}和{{ui:files.markdown.source}}视图之间切换。超过 128 KB 的文件以源码显示。 |
| HTML | 在绘制为页面的{{ui:files.markdown.rendered}}和{{ui:files.markdown.source}}视图之间切换。 |
| 图片、PDF | {{ui:files.image.fit}}、{{ui:files.image.actualSize}}、{{ui:files.image.zoomIn}}、{{ui:files.image.zoomOut}}。PDF 只显示第一页。超过 50 MB 的图片不预览，超过 1 亿像素的图片只能以适合窗口的大小显示。 |
| 其他 | {{ui:files.preview.unsupported}}。可以用{{ui:menu.showInFinder}}打开。 |

工作区以外的文件不会显示。如果文件有变化，请点击{{ui:files.tree.refresh}}。

![[files-pane]]

### 在编辑器中打开 {#open-in-editor}
要修改文件，就在你常用的编辑器中打开它。知道行号的地方会直接跳到那一行。
- 文件窗格：预览标题栏中的{{ui:editor.open}}按钮，以及在列表中右键单击文件或文件夹的菜单({{ui:editor.openFolder}})。
- 对话和 Mighty 流程：{{kbd:⌘}}-单击(Windows：{{kbd:Ctrl}}-单击)文件名，会在编辑器中打开该行，而不是预览。预览标题栏和结果文件列表每行上的按钮也一样。
- 查看更改：在[审阅请求更改](agent-pane.md#request-changes)和 worktree 窗格的查看更改中右键单击某一行会打开该行，按钮则打开所选文件。
- worktree 窗格栏的菜单和[审阅队列](layout.md#review)会打开 worktree 文件夹。

在[设置](settings.md#agents)的{{ui:settings.editor.label}}中选择编辑器。自动会按 VS Code、Cursor、Zed、JetBrains IDE(IntelliJ IDEA、WebStorm、PyCharm 等)、Sublime Text 的顺序使用找到的第一个。先在登录 shell 的 PATH 中查找命令(`code`、`cursor`、`zed`、`idea`、`subl`)，然后在 Mac 上查找“应用程序”文件夹中应用自带的命令，在 Windows 上查找常见安装文件夹中的程序。{{ui:settings.editor.system}}用文件的默认应用打开，不带行号，且永不打开可运行代码的文件。工作区或 worktree 之外的路径不会被打开。

## 网页链接 {#web-links}
回复中的网页链接以及智能体打开的网页，都会在默认浏览器中打开。

## 实时预览 {#live-preview}
点击智能体窗格标题栏中的地球图标({{ui:preview.button.name}})，会查找此工作区中正在监听的开发服务器。它会逐一询问{{ui:preview.menu.main}}和各 worktree 窗格端口块中的端口，以及在那里运行的运行操作找到的地址，只保留 Web 服务器。选择一个，就会在它所属窗格的预览中打开；有多个窗格时，全部打开会为每个窗格各打开一个。

1. 预览是单独启动的本机已安装的 Chrome、Edge、Chromium 或 Brave 窗口。每次都使用新的配置文件，不会使用你平时浏览器的登录信息和 Cookie。如果都没有安装，地址会在默认浏览器中打开，且无法选取元素。
2. 打开预览的窗格会在输入框上方出现一条工具栏。点击{{ui:preview.strip.pick}}，再在预览窗口中点击元素，该元素的标签、唯一选择器、主要样式、部分文本以及 React、Vue、Svelte、Angular 组件线索会附加到输入框，应用也会回到该窗格。再补充一句“把这个按钮的间距调小”之类的话发送即可。
3. {{ui:preview.strip.shotPage}}会附加预览中可见的画面；选取元素后只截取该元素区域。控制台错误和失败的请求按钮会一次附加页面上次加载以来的错误(没有时为禁用状态)。
4. 点击{{ui:preview.strip.close}}或关闭预览窗口即可结束。最后一个预览关闭后，该浏览器也会退出，配置文件会被删除。

> [!note]
> 预览只打开 `localhost`、`127.0.0.1` 或 `::1` 上、且在该窗格端口块或运行操作地址范围内的地址。页面试图跳转到范围外时会被阻止，并在工具栏中提示。窗格中的程序在端口块之外监听的端口只会在菜单中列出，不会打开；用 `MIGHTY_PORT` 启动服务器即可打开。预览页面在应用之外的程序中打开，因此无法访问应用。SSH 和远程工作区暂不支持预览。
