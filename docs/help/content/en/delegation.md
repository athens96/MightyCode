---
title: Mighty Orchestration
order: 6
section: delegation
---
With Mighty Orchestration, while one agent pane works, you hand other work to child panes beside it and both go on at once. A child runs its parent's agent: Claude, Codex or Antigravity; it can also run another of the three ([Codex and Antigravity](#agents)). Each child pane works in its own git worktree and branch, so they never touch each other's files, and when it is done you merge its result into the parent's branch from its card in the sidebar. It used to be called agent delegation.

There are two ways to open a child pane.

- **Start one yourself**: in a running Claude pane, or a Codex or Antigravity pane in a mode where its child can change files ([Codex and Antigravity](#agents)), type your next request and press {{kbd:⌘Enter}}. The request runs right away in a child pane with a new worktree ([Run in a new worktree](#worktree)).
- **Let Claude hand work over**: ask Claude (or Codex) to split the work and hand it to child panes. It makes the children, reads their reports and merges them too ([Let Claude hand work over](#delegate)).

> [!warning]
> This is an experimental feature. It is on by default and can be turned off in Settings. Try it in a practice repository first.

## At a glance {#overview}
This is how it goes when you start a child yourself.

1. Open a git repository folder as a workspace and give a Claude pane some work.
2. While Claude works, type your next request and press {{kbd:⌘Enter}}. The running task goes on undisturbed, and a child pane opens beside this pane and runs the request right away.
3. The child pane is listed one step in under the parent pane in the sidebar, with its state beside it.
4. When the child finishes, commits and leaves a report, its state becomes {{ui:delegation.child.state.reported}}.
5. Press the {{ui:delegation.card.merge}} button on the child's card to bring the result into the parent's branch.
6. Once merged, you are asked whether to discard that child pane right away. {{ui:delegation.mergedDiscard.confirm}} cleans up the child pane, its worktree and its branch. The merged work stays in the parent's branch.

![[delegation-tree]]

| To do this | Use |
|---|---|
| Run after the current task | {{kbd:Enter}} (when {{ui:composer.busySend.title}} is {{ui:composer.busySend.queue}}, the default) |
| Run right away, apart from the current task | {{kbd:⌘Enter}} or the {{ui:queue.worktree}} button, {{kbd:Enter}} with an agent picked under {{ui:composer.busySend.title}} while it runs, or a queued row's {{ui:queue.runNow}} button |
| Bring the result into the parent's branch | The card's {{ui:delegation.card.merge}} button |
| Undo a merge | The card's {{ui:delegation.card.undo}} button |
| Throw the result away | The card's {{ui:delegation.card.discard}} button |

## Run in a new worktree {#worktree}
While a Claude, Codex or Antigravity pane is running, type a request and press {{kbd:⌘Enter}} or the round {{ui:queue.worktree}} button (the branch icon) beside stop. The running task goes on undisturbed, and your request runs right away in a new child pane with its own worktree. The new pane is shown at once, unless you have started typing in the parent pane meanwhile.

- In a running pane, {{kbd:Enter}} by default puts the request in the queue, to run after the current task ends. If you pick an agent under {{ui:composer.busySend.title}}, {{kbd:Enter}} too runs it right away in a new worktree with that agent (below). The hint in the composer shows what {{kbd:Enter}} does now.
- A new worktree is used only where one can be made: a git repository with a branch checked out that has a commit. Anywhere else (not a git repository, no commit yet, a commit checked out instead of a branch) {{kbd:⌘Enter}} queues the request, as {{kbd:Enter}} does.
- The child starts from the parent's last commit. It gets a branch `mighty/<child id>`, and its worktree is made outside your project folder (`~/.mightycode/worktrees/<child id>` on the Mac, `%USERPROFILE%\.mightycode\worktrees\<child id>` on Windows), so it never touches the parent's work. Changes you have not committed in the parent's folder do not reach the child.
- A child of the pane's own agent runs in the parent pane's permission mode, with its model and {{ui:composer.effort.label}}. In a style that starts every request in plan mode, it starts in plan mode. It opens in the parent's view, and when the parent was using a style in the Mighty view it starts in the same style (the style's progress is counted on its own from the child's first request). Your attachments go with it.
- Its first message is your text, followed by a note from the app on where the child works, asking it to commit and write REPORT.md when it is done, so that its card can merge it. A slash command (text starting with `/`) runs alone, as you typed it, and the app's note follows as the child pane's next request, so it never becomes the command's arguments.
- To run the request with another agent, pick Claude, Codex or Antigravity under {{ui:composer.busySend.title}} in the composer's bottom row while the pane runs (an entry such as 'Worktree · Codex'), then send with {{kbd:Enter}}. A request with attachments but no text goes to the queue. {{kbd:⌘Enter}} ({{kbd:Ctrl+Enter}} on Windows) and the {{ui:queue.worktree}} button always keep the pane's own agent. A greyed-out entry says why it cannot run: its CLI is not installed or not signed in (check {{ui:menu.settings}} › {{ui:settings.nav.tools}}), it has no mode that can change files within what the pane's mode allows, or 3 children you started are already open. A child of another agent starts in the widest of its own modes that can change files and does nothing the pane's mode does not allow ([Codex and Antigravity](#agents)), with the model and {{ui:composer.effort.label}} a new pane of that agent would get, and opens in the pane's view without its style. If that child cannot be made or started, your text and attachments go back to the composer and the transcript says why; they never go to the pane itself. Where the pane's own agent cannot have a child in its mode but another agent can, the {{ui:queue.worktree}} button is not shown, {{ui:composer.busySend.title}} offers only the other agents, and {{kbd:⌘Enter}} queues the request.
- A request already in the queue can go the same way with its row's {{ui:queue.runNow}} button. Pick the agent with the agent picker before the button (the pane's own agent by default). If its child cannot be made or started, the request goes back to the head of the queue and the transcript says why.
- Use a child pane like any pane of its agent: answer its approval cards, send follow-up requests and change its permission mode.
- The parent pane's Claude gets no notices about a child you started, since it never asked for it. The parent's tools neither list it nor merge it or send it follow-ups (`human_started`). Follow its state in the sidebar and use its card's buttons yourself.
- A parent pane can have up to 3 open children, with 10 GB of free disk. Closing or discarding finished child panes makes room.
- When the child cannot be made or cannot start after all (the pane already has 3 open children, the disk is low, its worktree or first run fails, and so on), the pane's transcript says why. Your text then becomes that pane's next request, as {{kbd:Enter}} would make it. With the queue full it goes back to the composer. A child that failed to start stays listed until you discard it from its card.
- In a child pane, or in a pane with Mighty Orchestration turned off, {{kbd:⌘Enter}} queues, as {{kbd:Enter}} does. What you send from the phone to a running pane always waits in the queue too. Nothing a person sends goes into the running task.

## Panes in their own worktree {#pane-worktree}
So that several agent panes of one workspace can work at once, a workspace's agent pane can work in its own git worktree and branch, as a child does. The panes never write over each other's files, and when a pane is done you merge it into the workspace branch from the bar over it.

- **When a pane opens in a worktree**: when another agent pane already works in the workspace folder, a new agent pane asks: {{ui:paneWorktree.choice.worktree}}, {{ui:paneWorktree.choice.always}} or {{ui:paneWorktree.choice.sameFolder}}. Set it ahead in the workspace row's right-click menu, {{ui:paneWorktree.menu.mode}}: {{ui:paneWorktree.mode.ask}}, {{ui:paneWorktree.mode.further}} or {{ui:paneWorktree.mode.every}}. With {{ui:paneWorktree.mode.every}} the first pane works in a worktree too, so the workspace folder only receives merges (a pane working in the workspace folder would otherwise block merges). A pane opened from the phone follows this setting and, where the setting asks, opens in the workspace folder. A pane that continues an earlier session always works in the workspace folder.
- **How it is made**: with a child's rules. A branch `mighty/<pane id>` starts at the workspace branch's last commit, and the worktree is made in `~/.mightycode/worktrees/<pane id>`. A folder where none can be made (not a git repository, for example) says why and offers only the same folder.
- **What the agent is told**: the pane's first request carries a note from the app, so the agent knows its worktree, branch and PORT and commits its work when done. You merge from the app.
- **The bar**: over the pane: its branch and base, commits the base lacks (↑), commits the base moved ahead by since (↓), changed files not committed (±, click to review them), and its PORT (click to open localhost). While changes are left the button reads {{ui:paneWorktree.bar.commitMerge}}: it commits them, new files too, then merges. New files that look like secrets or are over 5 MB are not committed, and the pane says which.
- **Conflicts**: when a merge is refused for a conflict, an {{ui:paneWorktree.bar.resolve}} button appears. It asks the agent to bring the base branch in, resolve the conflict and commit.
- **The bar's menu**: {{ui:paneWorktree.bar.changes}} (files changed against the base, with their diff), {{ui:paneWorktree.bar.terminal}}, {{ui:paneWorktree.bar.finder}} (Windows: {{ui:menu.showInExplorer}}), undo the last merge (only while the workspace folder has not changed since), {{ui:paneWorktree.bar.pr}} (after asking, pushes the branch to origin, never forced, and on GitHub opens the page to create the pull request), and {{ui:paneWorktree.bar.discard}} (refused while child panes branch from this pane's branch).
- **Preparing**: the files listed in the workspace row's {{ui:paneWorktree.menu.copy}} (such as `.env`) are copied into a new worktree, then {{ui:paneWorktree.menu.setup}} runs once. The pane's runs get the `PORT`, `MIGHTY_WORKSPACE_FOLDER` and `MIGHTY_WORKTREE` environment variables and those of its [port range](#mighty-yaml).
- **Closing and the list**: a pane closed with nothing unmerged takes its worktree with it. One with work left keeps it, and {{ui:paneWorktree.menu.list}} in the workspace row's menu opens it as a pane again, merges it or deletes it.
- **Review queue**: worktree panes and child panes that finished with changes gather in the [{{ui:review.open}}](layout.md#review). {{ui:editor.openFolder}} in the bar's menu opens the folder the pane works in in your external editor.

## Start from an issue {#issues}
Pick a MightyWork task, a GitHub issue, or a Linear, Jira or GitLab issue and start working on it. Open the list with {{ui:integrations.menu.browse}} in a workspace's right-click menu in the sidebar.

- **Connecting**: in {{ui:integrations.menu.settings}} in the same menu. MightyWork signs in or takes a token; GitHub uses `gh`. Under {{ui:integrations.tracker.section}}, pick Linear, Jira or GitLab and turn on {{ui:integrations.tracker.enabled}}.
  - Linear: a {{ui:integrations.tracker.linearToken}} and the {{ui:integrations.tracker.linearTeam}} (empty for all teams).
  - Jira Cloud: the {{ui:integrations.tracker.jiraSite}} (`https://your-team.atlassian.net`), the {{ui:integrations.tracker.email}}, an {{ui:integrations.tracker.jiraToken}} and the {{ui:integrations.tracker.jiraProject}}.
  - GitLab: `glab` when it is installed and signed in, else a {{ui:integrations.tracker.gitlabToken}}. Left empty, the {{ui:integrations.tracker.gitlabProject}} and address are read from `origin`.
  - Tokens are kept only in the OS keychain (the Mac keychain, Windows Credential Manager), never on screen, in logs or in state files. {{ui:integrations.tracker.test}} checks one; {{ui:integrations.auth.logout}} deletes it.
- **The list**: each tracker you turn on gets a tab. Filter with {{ui:integrations.filter.mine}}, {{ui:integrations.filter.open}} and {{ui:integrations.filter.all}}; the chip picked first follows the {{ui:integrations.tracker.filter}} ({{ui:integrations.tracker.filterMine}}, {{ui:integrations.tracker.filterOpen}}). The list is read only when it opens and when you refresh it.
- **{{ui:integrations.action.startWorktree}}**: opens a pane in [its own worktree](#pane-worktree) for the issue. A Linear, Jira or GitLab issue's branch is `<prefix>/<key>-<title>` (such as `lin/ENG-123-fix-login`, `jira/PROJ-7-…`, `gl/12-…`); MightyWork tasks and GitHub issues keep `mighty/<pane id>`. The issue's title, body (a Jira body is turned into Markdown), sub-items and link go into the first request. The worktree bar shows the linked issue's chip; click it to open the issue.
- **Pull requests**: a pull request made from the Git workbench gets the issue's link at the end of its body (with the commits' subjects when you leave the body empty).
- **{{ui:integrations.tracker.writeBack}}**: one switch each per tracker: {{ui:integrations.tracker.writeStart}}, {{ui:integrations.tracker.writePr}}, {{ui:integrations.tracker.writeMerge}} and {{ui:integrations.tracker.commentPr}} (on by default). Linear moves to the team's started or completed state, Jira takes the transition with the matching name, and GitLab uses the `In Progress` and `In Review` labels and closes. What was written shows in the pane's log. MightyWork follows the single {{ui:integrations.settings.autoSync}} switch, and nothing is written to GitHub.

## Forking a conversation {#fork}
To try another way without losing the context a pane built up, open a new pane in a new worktree that carries on its conversation. The source pane's conversation and files stay as they are.

- **Where**: the pane's ··· menu {{ui:fork.menu.pane}}, or {{ui:fork.menu.here}} in a result block header's right-click menu (the conversation up to that request). A running pane can be forked once it is done.
- **Worktree**: a `mighty/fork-<short id>` branch and worktree are made from the source pane's HEAD. When the source works in its own worktree, the fork merges into the same base branch. Copied files, the setup command, `.mighty.yaml` and the [port block](#mighty-yaml) work as for any worktree pane.
- **Uncommitted changes**: with {{ui:fork.dialog.carry}} on (the default), the source's changed and new files come along (`git stash create` makes them without touching the source, then they are applied in the new worktree). Gitignored files never come along. If they cannot be carried, the source stays as it is and the new pane's log says why.
- **The conversation**: for Claude and Codex the CLI copies the source session into a new one with the first request (Claude `--fork-session`, Codex `exec fork`); the source session is not changed. For Antigravity, opencode, Hermes and pi, and when you fork from a request that is not the last, the first request starts with a summary of the last request, the plan, the last result and the changed files (12,000 characters at most, lines that look like secrets removed). If the CLI cannot find the source session, the next request carries the summary.
- **Where it shows**: the new pane is listed under its source in the sidebar, with the source's name and a button to it over the pane. The phone's pane summary carries it as `forkedFrom`.

## Compare run {#compare}
For a hard task it is often quicker to try several approaches and keep the best. Hand one request to several agents at once (or the same agent several times, say three Claude children) from the same base commit, compare the results side by side and merge one.

- **Start**: write the request in the composer, then choose {{ui:compare.menu}} from the send button's right-click menu on an idle pane, or from the {{ui:composer.busySend.title}} menu on a running one. In the picker, pick rows of agent and model ({{ui:compare.picker.add}}; the same agent may appear more than once), the {{ui:compare.picker.base}} (HEAD by default, or a recent commit of this branch) and, if you like, a {{ui:compare.picker.test}} (a run action or task from `.mighty.yaml`), then press {{ui:compare.picker.start}}.
- **Which agents**: only agents that can start a child now are offered (Claude, Codex and Antigravity, when installed, signed in and with a mode that can change files). Up to four rows, but a pane may have at most three children you started, so only the room left can be picked.
- **Run**: each row opens a child pane as [Run in a new worktree](#worktree) does, all at once, with the same request, the same base commit and its own [port block](#mighty-yaml). A row with a model runs that model. When one row cannot start, its column says why and the others go on.
- **Budget**: with a daily budget set, a start that would likely pass it (today's spend plus the estimate) is warned about first. The estimate is rough: for each child, the average cost of one run of the same agent and model in the last 30 days of the usage ledger (the agent's average without that model, $1 per child without either).
- **Group card and {{ui:compare.view.title}}**: a card appears under the pane in the sidebar; press it to open {{ui:compare.view.title}}. Each column shows the agent and model, the state, the files and +/− lines against the base (`git diff --shortstat <base>...<branch>`), the time taken, the cost from the usage ledger and the chosen test's result (run once in the child's worktree after it reports), with {{ui:compare.action.open}}, {{ui:compare.action.diff}}, {{ui:compare.action.merge}} and {{ui:compare.action.discard}}. The group is saved with the pane, so it opens again after a restart.
- **Picking one**: {{ui:compare.action.merge}} works only for a child that reported, with the card's rules and the pre-merge checks. Once it is merged you are asked whether to discard the other children at once. Undoing the merge stays on the child's card. {{ui:compare.view.dismiss}} removes the card only; the children stay.
- The phone's pane payload carries the group as `compareRuns`; the phone does not show it yet.

## Ports, run actions and teardown {#mighty-yaml}
A `.mighty.yaml` at the top of the repository can give each worktree its ports, run actions, and a teardown command that runs when the worktree is removed. Its commands run only after you press {{ui:projectTasks.approve}} in the {{ui:projectTasks.title}} panel, and again only after you approve a changed file. Remote workspaces do not use it.

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

- **Port range**: each worktree (and the main checkout) gets `ports` ports (10 by default) from 41000 that no other one holds. Ports another program already listens on are skipped. The assignment is kept in `ports.json` in the app's data folder and given back when the worktree is removed; the ports of worktrees removed while the app was closed are given back at the next start. `ports: 0` assigns none. The bar shows the range, such as `:41010–41019`.
- **Environment variables**: the setup and teardown commands, run actions, tasks and the agent of a worktree pane get `MIGHTY_PORT` (the first port), `MIGHTY_PORT_COUNT`, `MIGHTY_WORKTREE_NAME`, `MIGHTY_ROOT_PATH` (the main checkout) and `MIGHTY_WORKTREE_PATH`. The shell expands `$MIGHTY_PORT` in a command (`%MIGHTY_PORT%` in cmd on Windows). `worktree.env` cannot set these names.
- **Run actions (`run`)**: the {{ui:projectTasks.runActions}} section of the {{ui:projectTasks.title}} panel shows each action's state with {{ui:projectTasks.start}} and {{ui:projectTasks.stop}}. An address such as `http://localhost:<port>` found in its output shows as a link (`url: auto`, the default), marked {{ui:projectTasks.urlOutside}} when its port is not in the worktree's range. A `url` address is shown as written; `off` shows none. An action with `concurrent: false` is marked {{ui:projectTasks.notConcurrent}}, and starting it first stops the same action in the repository's other worktrees. Running actions also show on the bar.
- **Teardown (`teardown`)**: when a worktree is discarded, or removed as its merged pane closes, its run actions stop first and the teardown command runs in the worktree folder. It is stopped after `teardownTimeoutSeconds` (60 by default). The worktree is removed even when the teardown fails or is stopped; the result stays in the pane's transcript and in the {{ui:projectTasks.title}} panel.
- **Personal settings**: the same keys in `.mighty.local.yaml` take precedence; run actions are replaced by name.

## Let Claude hand work over {#delegate}
A Claude or Codex pane (the parent) can also split its work and hand parts of it to other panes (its children), of its own agent unless it names another. When a child reports, the parent gets a notice, reads the report and decides whether to merge the result. Claude and Codex parents work in the Mac app and the Windows app.

1. Open a git repository folder as a workspace and open a Claude pane. That pane is the parent.
2. Name the tool in your request, for example "use the delegate tool to open two Mighty child panes and split this between them". If you only say "split this up", Claude may use its own built-in sub-agents instead.
3. The child panes open next to the parent, and the sidebar lists them one step in under it.

- The parent's tools are `delegate` (hand over a task; its `agent` can name the child's agent: `claude`, `codex` or `antigravity`), `list_children` (list the children), `child_status` (read state and report), `merge` (merge a child) and `follow_up` (send a follow-up). The parent cannot call `discard`; only you can discard, from a card.
- Each child gets a branch `mighty/<child id>` and a worktree (`~/.mightycode/worktrees/<child id>` on the Mac, `%USERPROFILE%\.mightycode\worktrees\<child id>` on Windows). A child starts from the parent's last commit.
- The task goes into TASK.md and the result into REPORT.md. Both sit in a folder git ignores inside the worktree, so they are never committed.
- A child's permission mode is the one the parent chose, which can be no wider than the parent pane's own mode. A child of another agent may do nothing the parent's mode does not: change files without asking only if the parent's mode does (Claude in accept-edits or auto mode, Codex in {{ui:permission.label.onRequest}} or {{ui:permission.label.acceptEditsCodex}}, Antigravity in {{ui:permission.label.acceptEdits}}, any agent in {{ui:permission.label.fullAccess}}), change files after asking only if the parent's mode changes files at all, run shell commands without asking only if the parent's mode does (Claude in auto mode, Codex in {{ui:permission.label.defaultCodex}} or {{ui:permission.label.acceptEditsCodex}}, any agent in {{ui:permission.label.fullAccess}}), and have full access only under {{ui:permission.label.fullAccess}}. So under Claude's accept-edits mode, which asks before shell commands, a Codex child gets {{ui:permission.label.onRequest}}, not {{ui:permission.label.acceptEditsCodex}}, and a Claude pane in plan or manual mode gets no Codex or Antigravity child at all. Only you can widen it, between runs, from the permission menu in the child pane's composer or in the phone's settings.
- A child pane opens in the parent pane's view (the same one of {{ui:graph.view.diagram}}, {{ui:graph.view.timeline}}, {{ui:graph.view.structure}}). If the parent was on {{ui:graph.view.structure}} in the Mighty view, the child shows {{ui:graph.view.structure}} too.
- The parent cannot answer a child's approval cards. When a card appears in a child pane, answer it there yourself.
- A parent can have up to 3 open children made by its `delegate` tool, counted apart from the 3 you can start yourself; neither kind blocks the other. A child cannot delegate further. Children that reported or were merged still count while their panes are open, so close finished child panes to make room. Delegation is refused when less than 10 GB of disk is free.
- The parent learns of a child's report from a notice ([Notices and held items](#notices)), sends follow-ups with `follow_up` ([Follow-ups](#follow-up)) and merges with `merge` ([Merging](#merge)).

## Codex and Antigravity {#agents}
Mighty Orchestration works in Codex and Antigravity panes too. By default a child runs its parent's agent: a Codex pane's children are Codex panes, and an Antigravity pane's are Antigravity panes.

Any agent can have a child of any of the three. Pick it under {{ui:composer.busySend.title}} while the pane runs or with a queued row's agent picker ([Run in a new worktree](#worktree)), or let a Claude or Codex parent name it in `delegate`'s `agent` (`claude`, `codex` or `antigravity`). The child works by its own agent's rules below: its modes, and who commits its work (a Claude child commits it; a Codex or Antigravity child's work is committed by the app). Notices and merging follow the parent's agent. `delegate` refuses an agent it does not know (`agent_unknown`), one that is not installed or not signed in (`agent_unavailable`), and a mode that is not one of the child agent's, in which it cannot change files, or that may do more than the parent's ([Let Claude hand work over](#delegate) says how modes compare).

- **Where a child can work**: a child starts in its parent's permission mode, so a Codex or Antigravity pane opens one with {{kbd:⌘Enter}} only in a mode where the child can change files: Codex in {{ui:permission.label.acceptEditsCodex}}, {{ui:permission.label.onRequest}} or {{ui:permission.label.fullAccess}}, Antigravity in {{ui:permission.label.acceptEdits}} or {{ui:permission.label.fullAccess}}. In any other mode {{kbd:⌘Enter}} queues the request, as before.
- **Codex** panes get the delegation tools only in {{ui:permission.label.acceptEditsCodex}} or {{ui:permission.label.fullAccess}}: in {{ui:permission.label.defaultCodex}} Codex refuses them, and in {{ui:permission.label.onRequest}} it would ask in a way the app cannot show. `delegate` refuses a child mode that cannot change files. Codex may call `merge` only when its run started in {{ui:permission.label.fullAccess}}; otherwise it is refused (`merge_human_only`), and you merge from the card. A running Codex turn takes no text, so a notice for a running Codex parent goes as its next run once that run finishes normally, and is held after a stop or an error.
- **Antigravity** panes have no delegation tools (Antigravity takes them only from its global settings, which the app never changes): start a child only with {{kbd:⌘Enter}}, the {{ui:queue.worktree}} button or {{ui:composer.busySend.title}}.
- Codex and Antigravity children cannot commit in their worktree except in {{ui:permission.label.fullAccess}}, so they are told to leave their changes. When a child's run finishes normally with REPORT.md changed, the app commits its changes on its branch (`mighty: <child id> report r<n>`) and records the report at that commit; after a stop or an error it waits for the next normal finish. New files that .gitignore leaves out, that look like secrets (`.env*`, `*.pem`, `*.key`, `*.pfx`, `*.p12`, `*.ppk`, `id_rsa*`, `id_ed25519*`, `id_ecdsa*`, `id_dsa*`, `credentials*`, `secrets.*`, `.npmrc`, `.pypirc`) or that are over 5 MB stay uncommitted, and the child pane names the last two kinds. The commit uses your git name and email (without them it fails and the pane says so) and is never signed. With an unresolved merge conflict in the worktree the app commits nothing and the pane says so.
- When the card's merge is refused because of a conflict, only a child that can commit (in {{ui:permission.label.fullAccess}}) is asked to resolve it. Otherwise switch the child to {{ui:permission.label.fullAccess}} and ask it in its pane, or resolve the conflict yourself in a terminal in its worktree and commit there.
- In a workspace that is a subfolder of its repository, a Codex child may also write its worktree's top folder, where REPORT.md is, and an Antigravity child gets that folder added.

## A child's state {#states}
The sidebar shows each child's state.

| State | Meaning |
|---|---|
| {{ui:delegation.child.state.creating}} | Its worktree and pane are being made. |
| {{ui:delegation.child.state.running}} | It is working. |
| {{ui:delegation.child.state.waiting}} | It waits for you to answer an approval or a question. |
| {{ui:delegation.child.state.reported}} | It left a REPORT.md and finished its run. It can be merged. A later run that ends without changing anything leaves it here. |
| {{ui:delegation.child.state.merged}} | It was merged into the parent's branch. |
| {{ui:delegation.child.state.ended}} | Its run ended without a new report. |
| {{ui:delegation.child.state.interrupted}} | Its run stopped because the app quit. |
| {{ui:delegation.child.state.failed}} | The child pane could not start. |
| {{ui:delegation.child.state.closed}} | Its pane is closed. It stays listed until it is discarded. |

When a child starts a new run, its {{ui:delegation.child.state.reported}} state clears; its next report gets the next report number. If a child ended without a report, ask it in its pane again to commit and write REPORT.md.

## Merging {#merge}
There are two ways to bring a child's result into the parent's branch.

1. **The card's {{ui:delegation.card.merge}} button** is for you. It is the only way to merge a child you started yourself. It fast-forwards when it can and otherwise makes exactly one merge commit. It checks for conflicts first and shows the reason on the card when it refuses.
   While the child is still working, the parent pane is running or the parent's folder has uncommitted changes, it does not refuse: the merge is queued, the card says what it waits on, and it goes through by itself once that passes. It is dropped when the child ends without a new report. When the parent branch the child was made from has since been merged into the branch checked out now (main, say) or deleted, it merges into the branch checked out now; the parent's `merge` tool follows the branch the same way.
   Once the merge goes through, you are asked whether to discard that child pane right away. {{ui:delegation.mergedDiscard.confirm}} closes the child pane and removes its worktree and branch. The merged work stays in the parent's branch, but the merge can no longer be undone. {{ui:delegation.mergedDiscard.later}} keeps the card.
   When it is refused because of a conflict, the app asks the child pane to bring in the parent branch, resolve the conflict, commit and report again. The card says so, and the child becomes {{ui:delegation.child.state.running}}. Once it has reported again, it merges by itself. This request goes once per report. When the child pane cannot start a run now, nothing is sent, the card keeps the refusal, and the next merge tries again. When the pane took the request but could not start it, or the child is in plan mode and only made a plan, ask the child pane yourself to resolve the conflict and report again. A Codex or Antigravity child is asked only in {{ui:permission.label.fullAccess}} ([Codex and Antigravity](#agents)).
2. **The parent's `merge` tool** works only for children Claude handed work to. It fast-forwards only. The parent passes the child's head that `child_status` gave it. The merge is refused, changing nothing, when the child has not reported, when the child's or the parent's folder has uncommitted changes, when the child's head moved, when the parent's branch has moved on since (diverged), or when the parent's branch is not checked out. Whether an approval card appears for this tool follows the parent pane's permission mode. A Codex parent may use it only when its run started in {{ui:permission.label.fullAccess}}; in any other mode it is refused with `merge_human_only`, and you merge from the card.

> [!tip]
> Merging one child moves the parent's branch forward, so the `merge` tool usually refuses the other children as diverged. Merge those with the card's {{ui:delegation.card.merge}} button.

Until the child starts a new run, the card keeps an {{ui:delegation.card.undo}} button after a merge, even when the pane is closed. It works only while the parent's branch is still at that merge, the parent pane is idle and there are no uncommitted changes. Undoing puts the files back as they were before the merge, and a child whose pane is open becomes {{ui:delegation.child.state.reported}} again.

> [!note]
> If a merge goes through but the delegation record cannot be saved, the card says so. The merge stays, and the record is written again with the next save.

## Discarding {#discard}
The card's {{ui:delegation.card.discard}} button removes the child's worktree and branch, unmerged work included. Only you can discard; the parent cannot. A merged child can also be discarded from the question asked right after the merge ([Merging](#merge)).

- The confirmation ({{ui:delegation.discard.title}}) shows every other worktree nested inside the child's worktree, which goes with it.
- When that list cannot be checked, the confirmation has no discard button, and nothing is stopped or removed.

> [!tip]
> The app never removes unmerged work by itself. A discard you confirm is the only way unmerged work goes away.

## Closing panes and cleanup {#close}
- **When you close a merged child's pane**, the app removes its worktree and branch if all of its work is merged and nothing is left uncommitted (`git branch -d`, never forced). Otherwise only the pane closes and the worktree stays.
- **When you close a child's pane before merging**, the app can no longer merge it. Its card only offers discard, and its branch and worktree stay until you discard it.
- **When you close a parent pane that has open children**, the desktop app asks first ({{ui:delegation.parentClose.title}}). Only the parent pane closes; its children keep working.
- Children of a closed parent stay as cards under the {{ui:delegation.tree.parentClosed}} item in the sidebar. Their notices are held on their cards, which show how many, and they take no more follow-ups.
- A workspace with open children, or with child worktrees not yet cleaned up, cannot be removed.
- {{platform:windows}} If a child's worktree holds a junction or a symbolic link (a linked `node_modules`, for example), closing the merged child's pane leaves the worktree in place instead of cleaning it up, so that nothing follows the link and empties the folder it points to. Remove it with the {{ui:delegation.card.discard}} button on its card: discard takes the link itself away first and never touches the folder it points to.

![[delegation-parent-closed]]

## Notices and held items {#notices}
When a child Claude handed work to reports or fails to start, the parent gets one notice. It also gets one when a run the parent started (the first task or a follow-up) ends without a report. A notice is only a short pointer starting with `[Mighty Code notice …]`; the parent reads the report itself with `child_status`. A child you started yourself sends no notices.

- If the parent is running Claude, the notice goes straight into that run and shows in the parent's Mighty view as an orange {{ui:graph.block.delegationNotice}} block. A notice that reaches the parent after it has finished its answer shows as the same block, and Claude's answer to it is not added to the parent request's result: it opens as its own {{ui:graph.block.delegationNotice}} diagram. A running Codex parent gets it as its next run, once that run finishes normally.
- If the parent finished its last run normally since the app opened and is now idle, the notice starts its next run.
- Otherwise the notice is held: after a stop, after an error, after the app reopens, or while a CLI update runs. Once something is held, new notices are held behind it, so the order is kept.

Held notices show at the top of the queue above the parent's composer, as rows with a pin. These rows have no remove button and, unlike ordinary queued requests, they survive a stop or a relaunch.

![[delegation-held]]

- The next time you send something, all held items go first, oldest first, ahead of your text, in one run.
- The {{ui:queue.runNow}} button on the top held row sends all held items now, in one run, to this pane. The other held rows have no button. A {{ui:queue.runNow}} that opens a new worktree leaves the held items held for this pane. While items are held, a request that could otherwise go into the running Claude stays greyed out until they are sent.
- Each notice is delivered exactly once, even across a relaunch. Held rows also show in the phone's queue, and a send from the phone delivers them too.

## Follow-ups {#follow-up}
The parent can send a child Claude handed work to up to two follow-ups with its `follow_up` tool. A follow-up reaches the child pane by the same rules as a notice, and each can be up to 48 KiB. You can type straight into the child pane's composer, with no limit on how many times.

## Turn it on or off {#enable}
{{platform:mac}} It is on unless you turn it off. Switch {{ui:settings.delegation.toggle}} under {{ui:settings.agents.sectionTitle}} on the {{ui:settings.nav.general}} tab of Settings; it applies from the next run that starts, with no relaunch. The same switch is the defaults key `delegation.enabled`, so this command in Terminal turns it off too, and `defaults delete dev.mightycode.native delegation.enabled` turns it back on.

```sh
defaults write dev.mightycode.native delegation.enabled -bool false
```

{{platform:mac}} Earlier versions were on only when that key was set, and turning it off meant `defaults delete …`. That now leaves it on, so to keep it off, switch off {{ui:settings.delegation.toggle}} in Settings.

{{platform:windows}} It is on unless you turn it off. Switch {{ui:settings.delegation.toggle}} under {{ui:settings.agents.sectionTitle}} on the {{ui:settings.nav.general}} tab of Settings; it applies from the next run that starts, with no relaunch. The same switch is the `delegation.enabled` value (REG_DWORD) under `HKEY_CURRENT_USER\Software\MightyCode`: zero is off, and a missing value or any other is on. So this command in Command Prompt or PowerShell turns it off too, and `reg delete "HKCU\Software\MightyCode" /v delegation.enabled /f` turns it back on.

```
reg add "HKCU\Software\MightyCode" /v delegation.enabled /t REG_DWORD /d 0 /f
```

{{platform:windows}} Earlier versions were on only when that value was set to 1. A 1 left from then keeps it on.

- Once it is on, every new run in a Claude or Codex pane gets the delegation tools. A run that is already going does not.
- Turned off, only the tools go away, and {{kbd:⌘Enter}} queues the request, as {{kbd:Enter}} does. Existing child panes, their cards and their cleanup keep working.

## Limits and support {#limits}
| Item | Limit |
|---|---|
| Open children per parent | 3 you start yourself, 3 made by `delegate` |
| Delegation depth | One level (children cannot make children) |
| Follow-ups per child (`follow_up`) | 2, up to 48 KiB each |
| Task (TASK.md, a request you start yourself) | 64 KiB |
| Free disk needed | 10 GB |

- A child runs its parent's agent, or another one ([Codex and Antigravity](#agents)). Claude and Codex panes can also make children with the delegate tool; Antigravity panes, with no tools, make them only with {{kbd:⌘Enter}}, the {{ui:queue.worktree}} button, {{ui:composer.busySend.title}} and {{ui:queue.runNow}} ([Codex and Antigravity](#agents)).
- The Mac app and the Windows app have it on by default, with a switch in Settings ([Turn it on or off](#enable)). The phone has no new screen, but held rows show in its queue too.
