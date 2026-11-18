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

Shell 退出后，请点击{{ui:terminal.restart}}。之前的命令记录可以在窗格菜单的{{ui:pane.menu.terminalHistory}}中查看。退出应用后，终端不会重新打开。

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
| Markdown | 在{{ui:files.markdown.rendered}}和{{ui:files.markdown.source}}视图之间切换。 |
| 图片、PDF | {{ui:files.image.fit}}、{{ui:files.image.actualSize}}、{{ui:files.image.zoomIn}}、{{ui:files.image.zoomOut}}。PDF 只显示第一页。 |
| 其他 | {{ui:files.preview.unsupported}}。可以用{{ui:menu.showInFinder}}打开。 |

工作区以外的文件不会显示。如果文件有变化，请点击{{ui:files.tree.refresh}}。

![[files-pane]]

## 网页链接 {#web-links}
回复中的网页链接以及智能体打开的网页，都会在默认浏览器中打开。
