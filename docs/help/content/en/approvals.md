---
title: Approvals, questions and sign-in
order: 5
section: approvals
---
When an agent needs your decision, a card appears in its pane. Answer it and the work goes on. The same requests also show up in Work status, on Mighty Pet and on your phone.

## Approve a permission {#permission}
When the agent wants to do something outside its permission (run a command, change a file and so on), an approval card appears.

![[permission-card]]

1. Read what it wants to do. Commands and file paths are shown in boxes; the exact input is under Original JSON.
2. Click {{ui:permission.allowOnce}} or {{ui:permission.deny}}.

An approval applies to this one request only. No rule is made that keeps allowing it.

> [!note]
> Approval cards appear in Claude panes ({{ui:permission.label.claude.default}} and similar modes) with Codex's {{ui:permission.label.onRequest}} mode, and in opencode panes in every mode but {{ui:permission.label.fullAccess}}. In Hermes panes they appear for file edits and dangerous commands. In other modes the CLI's own settings allow or block the action. Antigravity works differently and [asks in its own way](#antigravity-permission).

## Antigravity's permission card {#antigravity-permission}
Antigravity cannot ask the app while it works. When it meets an action it would have to ask about (running a command, writing a file and so on), it skips just that action, finishes the turn, and then asks about everything it declined in one card. The card appears in {{ui:permission.label.default}} too, and the same card appears on your phone.

The card lists every declined action. For each one you see its kind ({{ui:antigravity.permission.kind.command}}, {{ui:antigravity.permission.kind.url}}, {{ui:antigravity.permission.kind.file}} and so on), the command, address or path, and the rule that would allow it.

1. Read what was declined.
2. Pick a button.
   - {{ui:permission.allowOnce}}: adds a rule for just the declined actions to Antigravity for a moment and continues the same conversation. When that run ends (finished, failed, stopped or the app closed), the app removes only the rules it added.
   - {{ui:antigravity.permission.allowAlways}}: keeps the same rule in Antigravity's settings (`permissions.allow` in `~/.gemini/antigravity-cli/settings.json`), so it runs without asking next time. On Windows the file is `%USERPROFILE%\.gemini\antigravity-cli\settings.json`.
   - {{ui:antigravity.permission.deny}}: closes the card and continues nothing.

Allowing continues in the same conversation. If the pane is busy with something else, the continuing run waits in the queue.

Rules match from the start. A command rule allows **every command that starts with its text** (`command(git status)` also allows `git status --short`), and a file rule for a folder allows every file in it. The {{ui:permission.allowOnce}} rule also goes into Antigravity's settings for the whole computer, so while it is there it applies to **every Antigravity run on this computer** (other panes and workspaces, and `agy` in a terminal). An always-allowed command rule allows any arguments after it, so for a command whose arguments can make it do something else (`find … -exec`, `curl … -o` and the like), prefer {{ui:permission.allowOnce}}.

A command that chains (`;`, `&&`, `||`), pipes (`|`), redirects (`>`, `<`), substitutes (backticks, `$(`), runs in the background (`&`), or deletes data or raises privileges (`rm`, `sudo`, `git push --force`, `git reset --hard`, `git clean`, `kill` and so on) offers only {{ui:permission.allowOnce}}. The card's {{ui:antigravity.permission.allowAlways}} is turned off with the reason; on the phone the button is not shown.

A command with anything but plain letters, digits, spaces and `._/:=@+,-` (quotes, `$`, `~`, `%`, `\` and the like) also offers only {{ui:permission.allowOnce}}, since a kept rule allows every command that starts the same way; a command with invisible or direction-changing characters, or spaces other than a plain space, gets no rule at all. A file rule is made only for a plain path on this computer's own disk (on Windows a drive path such as `C:\…` on a fixed drive), never for a network path such as `\\server\share`, which the app does not even look up. An address rule is made only when its host is plain (no `@`, `\`, `%`, spaces or non-ASCII letters), and the card shows the host as a browser reads it.

The app never makes a rule from a command with `*` or a parenthesis, or one for the whole disk (`/`) or your home folder (`~`). Nor does it make a file rule inside Antigravity's own settings folder (`~/.gemini`) or the app's data folder, or for a folder above them (such as your home folder): a rule there would let Antigravity change its own permissions. A command that mentions `.gemini`, a shell or interpreter on its own or given code inline (`bash`, `python3 -c …`, `node -e …`, `cmd /c …`), and a wrapper that runs another command (`npx`, `xargs`, `timeout`, `env` and so on) also offer only {{ui:permission.allowOnce}}, because `command(bash)` would allow anything that starts with `bash -c`. The card says which reason applies. Such an action shows as one no rule can allow.

This differs from Claude. Claude stops before acting and waits for your answer; Antigravity finishes the turn, asks afterwards, and carries on once you allow it. So when the card appears, the rest of the work may already be done.

> [!note]
> An action that no rule can allow still shows on the card, with guidance to change the permission mode. A card with only such actions offers just {{ui:antigravity.permission.deny}}. If Antigravity's settings file cannot be read or is not JSON, the app leaves it alone and shows the reason on the card. The app only adds and removes rules under `permissions.allow` and leaves every other setting as it was. If a rule it added for a moment cannot be removed, it tries again shortly, and again when the app opens next.

An Antigravity pane on a [remote workspace](remote.md) changes the settings of the computer that runs it. Each card's allow can be used only once there, and {{ui:antigravity.permission.allowAlways}} is accepted only while {{ui:settings.remote.allowFullAccess}} is on on that computer.

## Answer questions {#questions}
When Claude, pi, opencode or Hermes asks you to choose between options, a {{ui:phone.questionnaire.title}} card appears. A question that asks for a written answer without options shows only the text field.

![[question-card]]

1. Read the question and pick an answer. Some questions take more than one.
2. If no option fits, write your own in {{ui:phone.questionnaire.custom}}.
3. If there are several questions, go on with {{ui:phone.questionnaire.next}}.
4. When all are answered, click {{ui:phone.questionnaire.submit}}. Picking alone doesn't send anything.

## When the terminal waits for input {#terminal-prompt}

When a command the agent ran in the terminal (or one you started with {{ui:run.shellBlock.run}} on a shell block in a reply) waits for input, a **{{ui:terminalPrompt.title}}** card appears above that agent pane's composer. Answer on the card instead of going to the terminal pane, and the answer is typed into the terminal as it is. The same card shows in that pane on the phone.

- **Yes/no questions** (like `(Y/n)`): press {{ui:terminalPrompt.yes}} or {{ui:terminalPrompt.no}}. The answer Enter alone would give is marked ({{ui:terminalPrompt.default}}).
- **Passwords**: type into the hidden field and press {{ui:terminalPrompt.send}}. What you type is never shown or kept in the conversation.
- **Text**: type and press {{ui:terminalPrompt.send}}, or press {{ui:terminalPrompt.enterOnly}} to take the default.
- **Lists**: move with {{ui:terminalPrompt.up}} and {{ui:terminalPrompt.down}}, then press {{ui:terminalPrompt.select}}.

The card appears once the command's output has paused for a moment, and goes when you answer, the command prints again or it ends. The agent never answers for you. You can still type in the terminal pane yourself ({{ui:terminalPrompt.showTerminal}}).

## When the sign-in drops {#login-recovery}
If a request fails because the CLI sign-in expired, a sign-in recovery card appears above the composer.
1. Click {{ui:loginRecovery.loginButton}}.
2. Approve the sign-in in the browser that opens. If the browser shows a code, paste it into the card and click {{ui:loginRecovery.codeSubmit}}.
3. Once you are signed in, the failed request is sent again automatically.

If you already signed in somewhere else, click {{ui:loginRecovery.resendButton}}. If the browser sign-in doesn't work for you, use {{ui:loginRecovery.terminalButton}}.

{{platform:mac}} Antigravity, pi, opencode and Hermes sign in in a terminal. A sign-in terminal opens beside the failed pane. Antigravity (agy) opens a browser where you sign in with your Google account. In pi, type `/login` in the terminal and pick a subscription or enter an API key. opencode runs `opencode auth login`: pick a provider, then sign in or enter an API key. Hermes runs `hermes model`: pick a model provider, sign in, then pick a default model. Once the sign-in is confirmed, the failed request is sent again automatically. The terminal stays open after you sign in.

{{platform:windows}} On Windows too, Antigravity, pi, opencode and Hermes sign in in a terminal. The sign-in terminal opens in a window of its own (a new tab when Windows Terminal is installed), in the failed pane's folder, and what you do there is the same as on the Mac. Antigravity (agy) opens a browser where you sign in with your Google account. In pi, type `/login` and pick a subscription or enter an API key. opencode runs `opencode auth login`, and Hermes runs `hermes model`. Once the sign-in is confirmed, the failed request is sent again automatically. The app leaves the terminal open.

> [!tip]
> Turn on {{ui:settings.cliAccounts.autoLoginToggle}} on the {{ui:settings.nav.tools}} tab and sign-in starts again the moment it drops.

## Background work {#background}
Claude sometimes runs long commands or sub-agents in the background. Then work is still going on after Claude's answer is done.
- Background work shows in the {{ui:graph.progress.background}} group of the progress panel above the diagram (from the start of the request). The "1/3" beside the group's title is the number of tasks still running and the total this request started. Each row shows its state, its kind (command or sub-agent), the first line of its command or prompt, and the time it has taken; hover over a row to see it in full.
- When a task ends, Claude is notified and carries on with the result. An ended task shows as ended and stays in the group until you send the next request.
- While it runs, background work is not drawn in the request's blocks in the Mighty view; it lives only in the progress panel. The phone keeps a folded {{ui:plan.background.title}} line above the Mighty view that shows the ended and running counts, like "Done: 1 · Running: 2", and opens to the rows of the tasks still running.
- When a task ends and its notice goes to Claude, nothing is added to the request: each answer of Claude's opens a new diagram. It is titled after the finished work, like "{{ui:graph.background.noticeRun}} · code review agent", and shortened to "… and N more" when several ended together. It shows the ended task's {{ui:graph.block.task}} block (command, how it ended, summary) and Claude's answer. It is not a request, so it takes no request number. In the flow it is a new card at the bottom; in {{ui:graph.view.timeline}} it is a new row, and in {{ui:graph.view.structure}} a new card (board) at the end. A notice that names nothing opens no diagram unless Claude answers it. Until every notice has been answered, the request's own result card stays an {{ui:graph.block.awaitingNotice}} block ([result card](agent-pane.md)).
- A sub-agent started in the background works the same way. When it ends, its block in the Mighty view shows {{ui:graph.state.completed}}, {{ui:graph.state.error}} or {{ui:graph.state.stopped}} (also {{ui:graph.state.stopped}} if the app restarted before it finished), and the final report it hands back becomes the block's result. A message it sends while still running is added to the block's log and does not end it; Claude's answer to that message gets its own {{ui:graph.background.messageRun}} diagram. Further messages from the same agent, with Claude's answers, go on in that same diagram. Claude's answer to the end shows in the {{ui:graph.background.noticeRun}} diagram.
- A request you send now waits in the queue by default and starts after the background work ends. To start it now, click stop: the queue stays and its first request starts next. In a pane with [Mighty Orchestration](delegation.md#worktree) on, in a git repository, {{kbd:⌘Enter}} ({{kbd:Ctrl+Enter}} on Windows) runs it right away in a new worktree, and if you picked an agent under {{ui:composer.busySend.title}} below the composer, {{kbd:Enter}} too runs it right away in a new worktree with that agent ([Keep asking while it runs](agent-pane.md#queue)).
- Folding the progress panel folds the background group with it.

![[background-work]]

## Sandbox {#sandbox}
Each workspace can run its agents in a sandbox: the macOS sandbox on the Mac, the [experimental Windows sandbox](#sandbox-windows) on Windows. It is off by default. Right-click the workspace in the sidebar, choose {{ui:sandbox.menu.settings}}, turn on {{ui:sandbox.settings.enabled}} and press {{ui:sandbox.settings.save}}. It applies from the next run, and the pane's header shows a {{ui:sandbox.badge}} badge. Click the badge to open the settings.

### What it blocks {#sandbox-blocked}
- **Writes**: only in the workspace (or the pane's worktree) folder, the agent's own settings folders, the temporary folders and package caches.
- **git**: commits, branches, merges and rebases work. `.git/config`, `.git/hooks` and `.git/info` cannot be changed.
- **Instruction files**: with {{ui:sandbox.settings.instructions}} on (the default), `.mighty.yaml`, `AGENTS.md`, `CLAUDE.md` and the agents' project settings cannot be changed. Add more paths under {{ui:sandbox.settings.readOnly}}.
- **Reads**: `~/.ssh`, `~/.aws`, `~/.gnupg`, `~/.config/gh`, browser cookies, the app's data folder and the other agents' sign-in files cannot be read. Each agent reads only its own sign-in.
- **Connections**: with {{ui:sandbox.settings.network}} on (the default), the agent goes out only through the app's proxy. Only the agent's API hosts and what you list under {{ui:sandbox.settings.hosts}} pass at once.

### Windows (experimental) {#sandbox-windows}
{{platform:windows}} It works without administrator rights. The agent's CLI runs with a low-integrity, write-restricted token, inside a job that cannot touch other windows or the clipboard. The settings box shows the {{ui:sandbox.settings.experimental}} mark and the {{ui:sandbox.settings.guarantees}} list.

**Approval before it turns on**: turn on {{ui:sandbox.settings.enabled}} and press {{ui:sandbox.settings.save}}, and the list of folder permission changes comes first. The workspace folder gets a low integrity label and a write entry, `.git/config`, `.git/hooks`, `.git/info` and the instruction files get a write-deny entry, secret folders such as `~/.ssh` and `~/.aws` get a no-read-up label, and each agent's folders (`~/.claude`, `~/.codex`…) become writable for that agent only. Nothing changes until you press {{ui:sandbox.windows.approve}}. Every change is recorded; turning the sandbox off and saving undoes the workspace's changes (a secret folder's label, shared by workspaces, when the last one turns it off). Before removing the app, press {{ui:sandbox.windows.undoAll}}.

**What it does not stop**:
- The connection allow list is advisory. Only programs that follow the proxy setting go through the list and the cards; a program that ignores the proxy and connects directly is not stopped.
- Instruction files that did not exist when it was turned on (a new `AGENTS.md`) can be changed.
- Another agent's sign-in file can be read (writing to it is stopped). Browser profiles are not closed.
- A folder with a low integrity label can also be written by other low-integrity programs on this computer.
- A workspace on a network folder (`\\server\share`) or a drive that is not NTFS cannot be sandboxed.

**Agents**: Claude, Codex, Antigravity, OpenCode and pi run in the sandbox. Hermes is refused (its program and settings share one folder); it is never run unsandboxed. Package caches and temporary files go to a folder of the workspace's own under `%USERPROFILE%\AppData\LocalLow\MightyCode\sandbox`.

### Allowing a connection {#sandbox-allow}
When the agent connects somewhere not on the list, the **{{ui:sandbox.network.cardTitle}}** card appears above the composer.
- {{ui:sandbox.network.allowOnce}}: allowed until this run ends.
- {{ui:sandbox.network.allowAlways}}: added to the workspace's allow list and allowed.
- {{ui:sandbox.network.deny}}: blocked. No answer within 5 minutes also blocks it.

A blocked action stays as a line in the progress, and shows with its reason in the same pane on the phone and in the settings' {{ui:sandbox.settings.blocked}} list. You can answer the card on the phone too ({{ui:sandbox.network.allowAlways}} only on the Mac).

### Reviewer agent {#sandbox-reviewer}
With {{ui:sandbox.settings.reviewer}} on, a read-only agent (Claude's haiku by default) looks first whenever an approval or connection card appears.
- It allows only safe actions on its own: reading files, editing files inside the workspace, build and test commands, package registries.
- It cannot allow writes outside the workspace or dangerous commands. It denies them or leaves them to you.
- No answer within 20 seconds, or an error, is never an allow: you are asked.
- Each decision leaves a line in the pane. The card stays up meanwhile, so you can answer first, and your answer wins.
- Only your own words are permission. The reviewer's text never counts as permission. Reviewer runs cost money and show in the usage.

### Limits {#sandbox-limits}
- Shell panes' commands, SSH workspaces (the agent runs on the remote machine) and remote workspaces are not sandboxed.
- {{platform:windows}} The Windows sandbox is weaker than the Mac's. See [Windows](#sandbox-windows).
- A sandboxed run ends when the app closes. It does not keep running in the background.
- macOS's `sandbox-exec` is deprecated by Apple. Without it, a run in a sandboxed workspace is refused; turn the sandbox off to run.
- Agents that keep their sign-in in the keychain (Claude, Codex, Antigravity) can read the login keychain. macOS asks separately for other apps' items.
