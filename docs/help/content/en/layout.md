---
title: The window
order: 2
section: layout
---
The window has a sidebar on the left and a work area on the right. In the work area you can stack panes as tabs or split them side by side or top and bottom.

![[overview]]

## Sidebar {#sidebar}
From the top, the sidebar has:
- {{ui:sidebar.searchPlaceholder}}: press {{kbd:⌘K}} to jump to the search field.
- {{ui:phone.dashboard.title}}: the state of every pane in every workspace, in one place.
- {{ui:review.sidebar}}: the panes and requests with changes to review, gathered together ([Review queue](#review)).
- The {{ui:sidebar.workspacesHeader}} list: click a workspace to switch to it. Click the arrow beside it to show or hide its panes.
- At the bottom: the help button, the theme button (dark or light) and the settings button.

When a workspace is expanded, its last row is {{ui:workspace.addPane}}. Right-click a pane row for {{ui:menu.rename}} and {{ui:menu.closePane}}.

### Resize the sidebar {#sidebar-resize}
Drag the border between the sidebar and the work area to change the sidebar's width ({{ui:sidebar.resize}}).
- The border turns the accent colour when you point at it. Drag it left or right to set the width. The width stays within set limits and is kept after you reopen the app.
- Double-click the border to go back to the default width.
- Select the border with Tab and press {{kbd:←}} / {{kbd:→}} to narrow or widen it by 10 px.
- Drag the border all the way to the left to collapse the sidebar. When you bring it back, it has the width it had before the drag.

### Collapse the sidebar {#sidebar-collapse}
Collapse the sidebar to give the work area more room.
1. Click the sidebar button at the top-left of the work area's header or press {{kbd:⌃⌘S}}. The {{ui:phone.dashboard.title}} screen and the first screen have the same button in the same place. The View menu in the menu bar also has {{ui:sidebar.collapse}}.
2. Use the same button or {{kbd:⌃⌘S}} to bring it back ({{ui:sidebar.expand}}).

The sidebar stays collapsed after you reopen the app. Pressing {{kbd:⌘K}} while it is collapsed opens it and jumps to search.

![[sidebar-collapsed]]

## Work status {#dashboard}
Click {{ui:phone.dashboard.title}} under the search field in the sidebar to see the panes of all workspaces, sorted by state.
- The tiles at the top count {{ui:phone.dashboard.stat.running}}, {{ui:phone.dashboard.stat.waiting}} and {{ui:phone.dashboard.stat.done}}.
- The {{ui:dashboard.usage.title}} card shows your account usage. The limits are shared with other apps and sessions on the same account.
- Each workspace has a {{ui:files.pane.title}} button and an {{ui:workspace.addPane}} menu.
- Click a card to go to that pane. Choosing a workspace or pane leaves the dashboard.

![[dashboard]]

## Review queue {#review}
Everything ready to review, in one place. Click {{ui:review.sidebar}} under {{ui:phone.dashboard.title}} in the sidebar, or press {{kbd:⇧⌘R}} (Windows: {{kbd:Ctrl+Shift+R}}), to open the {{ui:review.open}} panel. The number beside the entry is how many items wait.
- What it lists: [worktree panes](delegation.md#pane-worktree) and [child panes](delegation.md#states) that finished with changes, and each pane's newest finished request whose changed files were kept ([reviewing a request's changes](agent-pane.md#request-changes)). A pane that is still working shows once it finishes. A request whose files were all reviewed this way leaves the list.
- Each row shows the pane's title, its agent, kind and workspace, the branch and its base, the files changed with lines added and removed, and its state ({{ui:review.status.ready}}, {{ui:review.status.conflict}}, {{ui:review.status.merged}}). Conflicts come first, merged items last.
- Buttons: {{ui:review.action.changes}}, {{ui:review.action.merge}} ({{ui:review.action.commitMerge}} for a worktree pane with files not committed), {{ui:editor.open}}, {{ui:review.action.pr}} (worktree panes), {{ui:review.action.discard}}, {{ui:review.action.focus}} and {{ui:review.action.dismiss}}. Merge, discard and PR are the same actions as the pane's bar or the child's card, with the same confirmations. A request's {{ui:review.action.changes}} opens the request's changes review.
- {{ui:review.action.dismiss}} hides an item until its work changes again.
- The list reads git again when a run ends, when the panel opens and when you click {{ui:review.refresh}}, never on every redraw.
- Worktree and child pane rows carry a pre-merge checks summary ({{ui:review.checks.ok}} or the number of warnings). Click it to open the pane's {{ui:git.checks.tab}} tab ([Checks](#checks)). The summary comes from the last checks reading and what the app already knows (uncommitted files, remaining to-dos, the pane's PR status); the list never calls gh on its own.

## Commit & PR {#git}
Do a workspace's git work in one panel. Open it from {{ui:git.workbench.open}} in the sidebar's workspace menu, the branch in the workspace header, or `/git` in the composer. Opened from a worktree pane, it works in that pane's worktree.
- {{ui:git.checks.changesTab}} tab: changed files and their diffs, staging by file or hunk, switching branches, a drafted commit message, commit, push, creating a PR and asking to fix failed CI. No PR is made from the default branch.
- {{ui:git.checks.tab}} tab: everything to look at before a merge, on one screen ([Checks](#checks)).

### Checks {#checks}
For a worktree pane (or the current branch for other panes) it shows five things.
- {{ui:git.checks.git.title}}: how many changes are not committed, and how many commits it is ahead (↑) of and behind (↓) its base branch.
- {{ui:git.checks.pr.title}}: the PR (MR on GitLab) number, state, whether it is a draft, conflicts, and the review result (approved, changes requested).
- {{ui:git.checks.ci.title}}: each check and its result. With a failed check, {{ui:git.pr.fixChecks}} hands it to the agent with the failed log.
- {{ui:git.checks.threads.title}}: each unresolved review thread's file and line, author, first comment, reply count and link.
- {{ui:git.checks.checklist.title}}: the items not done in the checklist of the pane's last request.

- It reads again when the tab opens and every 30 seconds while it stays open, never while it is closed.
- Each part is read on its own: without gh or glab, signed out, or with no PR, only that part shows {{ui:git.checks.unknown}}.
- Pick threads and click {{ui:git.checks.threads.toComposer}}: a list of `file:line (author): comment` under "Please address the review comments on PR #42." goes into the pane's composer. Nothing is sent yet. Lines that look like secrets are hidden, account details leave links, and at most 2,000 characters go in.
- Before a merge: if any check failed or is still running, a thread is unresolved, to-dos remain, changes are not committed (except for commit and merge), the PR is a draft, has conflicts or has changes requested, the merge buttons on the worktree bar, the child cards and the review queue show the reasons and ask once more. {{ui:git.checks.merge.confirm}} merges as usual; nothing is blocked.

## Add a pane {#add-pane}
A pane is an AI agent (Claude, Codex, Antigravity, pi, opencode or Hermes), a terminal or a files pane.

1. Click {{ui:workspace.addPane}} in the sidebar.
2. Pick what to add: an agent pane (Claude, Codex, Antigravity, pi, opencode or Hermes), {{ui:workspace.newTerminal}} or {{ui:menu.showFiles}} ({{kbd:⇧⌘E}}). The bottom group of the menu has the three ways to add a workspace: {{ui:workspace.add.existing}}, {{ui:workspace.add.newFolder}} and {{ui:workspace.add.fromGit}} ([Open a workspace](getting-started.md#open-workspace)).
3. A new AI pane takes the model, thinking effort, permission and view of the most recently used pane of the same agent. The conversation starts fresh.

Shortcuts: {{ui:menu.newClaudePane}} is {{kbd:⌘N}}, {{ui:menu.addTerminalPane}} is {{kbd:⌘T}} and {{ui:menu.showFiles}} is {{kbd:⇧⌘E}}. Up to 128 panes can be open across all workspaces (remote panes that show another computer's panes as they are do not count).

> [!note]
> Every agent pane but Claude's (Codex, Antigravity, pi, opencode, Hermes) carries a {{ui:badge.beta}} label.

### Continue an earlier session {#resume}
When you add an agent pane and this folder has earlier sessions, you are asked what to do. Antigravity's are its conversations in this folder; the ones its sub-agents ran are not listed.
1. Click {{ui:resume.choice.startNew}} for a new conversation, or {{ui:resume.choice.resume}} for an earlier one.
2. Pick a session in {{ui:resume.title}}. Use {{ui:resume.search}} to find it.
3. The new pane continues that session.

![[resume-choice]]

![[resume-list]]

### Search past conversations {#conversation-search}
{{ui:conversationSearch.open}} ({{kbd:⇧⌘F}}) finds words in the requests and answers of this folder's past sessions. It also opens from the magnifier button in the workspace header, the menu you get by right-clicking a workspace in the sidebar, and the Workspace menu in the menu bar.
- Sessions where one message holds every word are listed, newest first, up to 50, with the words found marked. Tool output is not searched.
- Claude, Codex, Antigravity and pi sessions are searched; opencode and Hermes sessions are not. Sessions open in a pane and sessions older than 60 days are left out.
- Choose one with {{kbd:↑}} {{kbd:↓}} and press {{kbd:Enter}}, or click {{ui:conversationSearch.resume}}: a new pane continues that session.
- {{ui:conversationSearch.export}} saves the conversation's requests and answers as a Markdown file.
- It is not available in a remote workspace, whose session records are on that computer.

## Tabs and splits {#tabs-splits}
- Drag a tab onto the middle of another group to merge it as a tab ({{ui:layout.drop.merge}}).
- Drop it on the left, right, top or bottom edge of a group to split that way.
- Drag a divider to resize; double-click it to split evenly.
- Press {{kbd:Esc}} while dragging to cancel.
- Double-click a tab to rename it. Right-click it for {{ui:menu.rename}}, {{ui:menu.focusPane}} and {{ui:menu.closeTab}}.

The layout is saved per workspace. Switching workspaces keeps each layout as it was.

### The pane menu {#pane-menu}
The … button in a pane's header has:
- {{ui:menu.rename}}
- {{ui:menu.focusPane}}: show only this pane, large. Click again for {{ui:pane.menu.restoreLayout}}.
- {{ui:pane.menu.copyLog}}: copies an agent pane's log. A terminal pane has {{ui:pane.menu.terminalHistory}} in its place ([Terminal pane](terminal-files-browser.md#terminal)).
- {{ui:pane.menu.newConversation}}: the next request no longer continues the earlier conversation (agent panes).
- {{ui:menu.closePane}}

Right-clicking a pane's header offers {{ui:menu.rename}}, {{ui:session.menu.focus}} and {{ui:session.menu.close}}, and a terminal pane also has {{ui:terminal.history.title}}.

## Keyboard shortcuts {#shortcuts}
| Shortcut | Action |
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
| {{kbd:Enter}} | Send the request. In a running pane, queue it or run it right away in a new worktree with the agent picked, as {{ui:composer.busySend.title}} says ([Keep asking while it runs](agent-pane.md#queue)) |
| {{kbd:⇧Enter}} | New line |
| {{kbd:⌘Enter}} | In a running pane, run it now in a new worktree (a pane with [Mighty Orchestration](delegation.md#worktree) on, in a git repository); in any other running pane, add it to the queue |
| {{kbd:Esc}} | Close a sheet or the slash list |
| {{kbd:⌘=}} / {{kbd:⌘-}} / {{kbd:⌘0}} | {{ui:menu.textBigger}} / {{ui:menu.textSmaller}} / {{ui:menu.textActualSize}} |
| {{kbd:⌘?}} | {{ui:menu.help}} |
