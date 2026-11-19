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
