---
title: Terminal and files panes
order: 7
section: terminal-files-browser
---
You can keep terminal and files panes open next to your agent panes.

## Terminal pane {#terminal}
A real terminal that opens in the workspace folder.
1. Use {{ui:menu.addTerminalPane}} ({{kbd:⌘T}}), or {{ui:workspace.addPane}} → {{ui:workspace.newTerminal}}.
2. Type commands as usual.

If the shell exits, click {{ui:terminal.restart}}. Earlier command history is under {{ui:pane.menu.terminalHistory}} in the pane header's … menu, and under {{ui:terminal.history.title}} when you right-click the pane header. Terminals are not reopened after the app quits.

![[terminal-pane]]

### Notifications and the bell {#terminal-signals}
When a program in a terminal pane, or a CLI the app does not read, sends a notification or rings the bell, its pane is marked. Notifications come as OSC 9 (`printf '\e]9;Build done\a'`), OSC 777 (`notify;title;body`) and OSC 99 (kitty's, title and body only). The `9;4;…` progress report is not a notification. The text shows without control characters, up to 200 characters, and a pane takes at most one a second.
- While you look at another pane or the app is in the background, the pane's sidebar row and tab get a dot and its border is highlighted. A notification also sends an OS notification; the bell only marks the pane.
- An agent pane waiting for an approval or an answer, and a worktree whose setup failed, are marked the same way. {{ui:phone.dashboard.title}} shows how many panes are waiting; click it to go there.
- Press {{kbd:⇧⌘U}} (Windows: {{kbd:Ctrl+Shift+U}}) to go to the pane that asked most recently. Press it again for the next one. {{ui:attention.next}} in the menu bar's {{ui:menu.workspace}} menu does the same.
- Looking at the pane clears the mark. The pane you are looking at gets neither a mark nor an OS notification.
- To stop them, turn off {{ui:terminalNotifications.toggle}} in [Settings](settings.md#companion).

### Open ports and pull requests {#pane-ports}
Every 10 seconds the TCP ports opened by a pane's shell, agent and run actions, and anything running under them, are read and shown in its sidebar row and on its worktree bar. Only the current workspace's panes are read, while the app's window is showing. Click a port on the worktree bar to open `localhost` in the browser.

A worktree pane also shows its branch's pull request: its number, state (open, draft, merged, closed) and checks (passed, failed, running). It is read again every 2 minutes; click it to open the pull request. When the sidebar is narrow, the ports and pull request show as icons only.

### Agent terminals {#agent-terminal}
When Claude or Codex runs a command you should see (a dev server, a sign-in and so on), a terminal pane of its own opens to the right of the agent pane. You can type into the running command. Closing the pane leaves the command running until the app quits; reopen it with {{ui:agentTerminal.terminalPane.open}} in the pane header.

## Files pane {#files}
A read-only pane for browsing the workspace's files. It never changes them.
1. Use {{ui:menu.showFiles}} ({{kbd:⇧⌘E}}) or the folder button in the workspace header.
2. Pick a file in the list on the left. Find names with {{ui:files.tree.filter}}, and move with the arrow keys and {{kbd:Return}}.
3. Read it on the right.

| File | How it shows |
|---|---|
| Source and text | Line numbers and highlighting. Only the first 1 MB of a large file is shown. |
| Markdown | Switch between {{ui:files.markdown.rendered}} and {{ui:files.markdown.source}}. A file over 128 KB is shown as source. |
| HTML | Switch between {{ui:files.markdown.rendered}}, drawn as a page, and {{ui:files.markdown.source}}. |
| Pictures and PDF | {{ui:files.image.fit}}, {{ui:files.image.actualSize}}, {{ui:files.image.zoomIn}}, {{ui:files.image.zoomOut}}. Only the first page of a PDF is shown. A picture over 50 MB is not previewed, and one over 100 megapixels is shown fitted only. |
| Anything else | {{ui:files.preview.unsupported}}. Open it with {{ui:menu.showInFinder}}. |

Files outside the workspace are not shown. If files changed, click {{ui:files.tree.refresh}}.

![[files-pane]]

### Open in an editor {#open-in-editor}
To change a file, open it in the editor you use. Where the line is known, the editor goes straight to it.
- Files pane: the {{ui:editor.open}} button in the preview's header, and the menu you get by right-clicking a file or folder in the list ({{ui:editor.openFolder}}).
- Conversation and Mighty flow: {{kbd:⌘}}-click (Windows: {{kbd:Ctrl}}-click) a file name to open it at its line in the editor instead of the preview. The button in the preview's header and on each row of a result's file list do the same.
- Changes: in [reviewing a request's changes](agent-pane.md#request-changes) and in a worktree pane's changes, right-click a line to open the file at that line; the button opens the selected file.
- The worktree pane bar's menu and the [review queue](layout.md#review) open the worktree's folder.

Choose the editor with {{ui:settings.editor.label}} in [Settings](settings.md#agents). Auto uses the first one found of VS Code, Cursor, Zed, the JetBrains IDEs (IntelliJ IDEA, WebStorm, PyCharm and the rest) and Sublime Text. Commands on your login shell's PATH (`code`, `cursor`, `zed`, `idea`, `subl`) come first; then, on the Mac, the command inside the app in your Applications folder, and on Windows the program in its usual install folder. {{ui:settings.editor.system}} opens the file in its default app, without the line, and never opens a file that runs code. A path outside the workspace or worktree is not opened.

## Web links {#web-links}
A web link in a reply, and a page an agent opens, open in your default browser.
