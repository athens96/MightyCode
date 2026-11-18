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

If the shell exits, click {{ui:terminal.restart}}. Earlier command history is in the pane menu under {{ui:pane.menu.terminalHistory}}. Terminals are not reopened after the app quits.

![[terminal-pane]]

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
| Markdown | Switch between {{ui:files.markdown.rendered}} and {{ui:files.markdown.source}}. |
| Pictures and PDF | {{ui:files.image.fit}}, {{ui:files.image.actualSize}}, {{ui:files.image.zoomIn}}, {{ui:files.image.zoomOut}}. Only the first page of a PDF is shown. |
| Anything else | {{ui:files.preview.unsupported}}. Open it with {{ui:menu.showInFinder}}. |

Files outside the workspace are not shown. If files changed, click {{ui:files.tree.refresh}}.

![[files-pane]]

## Web links {#web-links}
A web link in a reply, and a page an agent opens, open in your default browser.
