---
title: Phone remote
order: 9
section: phone
---
With the phone app you can follow and drive Mighty Code on your Mac or Windows PC while you're away: check on tasks, send requests, and answer approvals and questions.

The computer and the phone connect through a relay server. The default relay is built into the app, so there is nothing to set up. Everything is encrypted end to end, so the relay can't read it. You don't need to open router ports or use a VPN. If you use Tailscale, the phone can also [connect directly](#direct) without the relay.

## Get the phone app {#get-app}
Download the Android app (APK) from the [download page](https://pub-b705901d18c940159d40f58683d758ad.r2.dev/mightycode/index.html) and install it.

## Pair your phone {#pair}
On the computer:
1. Open {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → the {{ui:settings.remote.phoneTab}} tab.
2. Turn on {{ui:settings.mobileRemote.allowToggle}}. On Windows it is {{ui:windows.mobile.allow}}. Its state shows beside it; if it stays stuck connecting to the relay, click {{ui:settings.mobileRemote.reconnectButton}}.
3. Under {{ui:windows.mobile.pairingTitle}}, a QR code appears beside three steps.

On the phone:
1. Open the phone app and tap {{ui:phone.hosts.addHost}} on the {{ui:phone.tabs.hosts}} tab. The first time, a connection guide shows first.
2. Scan the computer's QR code with {{ui:phone.pair.mode.qr}}. If you can't use the camera, click {{ui:settings.mobileRemote.copyLinkButton}} on the computer, send yourself the link and paste it in {{ui:phone.pair.mode.manual}}. The link carries the key, so it is copied as a secret and cleared from the clipboard after a minute.
3. Tap {{ui:phone.pair.connect}}. The computer shows a "new device" window and a notification, and the phone asks you to allow it on the computer. Click {{ui:settings.mobileRemote.approval.allow}} on the computer and the phone connects and appears under {{ui:settings.mobileRemote.connectedDevicesTitle}}. Without an answer in 60 seconds, or with {{ui:settings.mobileRemote.approval.refuse}} (the default button), it does not connect. Allow only a phone you are pairing yourself.

Next to the QR code are the {{ui:settings.mobileRemote.hostIdLabel}} and the {{ui:settings.mobileRemote.keyLabel}}. The key shows only when you click its show button. The QR code, the link and the key are a one-time pairing code: it keeps working while it is on screen, expires 10 minutes after you close the screen, and is replaced once a device connects or is refused. Devices already paired connect with their device token and are not asked again.

{{ui:windows.mobile.connectionTitle}} offers two ways to connect.
- {{ui:windows.mobile.relayTitle}}: the default. Nothing to do.
- {{ui:mobileRemote.direct.toggle}}: set the port and apply it; its state shows below. See [Connect directly over Tailscale](#direct).

![[settings-mobile]]

Paired phones and [paired computers](remote.md#pair) are listed one per row under {{ui:settings.mobileRemote.connectedDevicesTitle}}; a computer shows a computer icon (kept after the app restarts). The list is shown whenever there is a paired device, even with the phone connection switch off. When you stop using a device, click {{ui:settings.mobileRemote.revokeRowButton}} on its row. Only that device is unpaired; your other phones and computers stay connected. The pairing code on screen is renewed at the same time. Pairing a phone again with the QR code asks for your approval like a new device (it says it replaces one), and once allowed its old pairing stops working.

> [!warning]
> {{ui:settings.mobileRemote.regenerateKeyButton}} unpairs every paired phone and every paired computer and makes this computer's relay ID and encryption key anew, so every device has to pair again with a new QR code or link. Use it if someone connected without your permission or you lost a device.

## Workspaces the phone sees {#workspaces}
In the {{ui:settings.remote.phoneTab}} tab, {{ui:settings.mobileRemote.workspacesTitle}} chooses which of this computer's workspaces a paired phone sees.
- {{ui:settings.mobileRemote.workspacesAll}}: the default. The phone sees every workspace on this computer and opens its panes, files (a file like `.env` too) and runs.
- {{ui:settings.mobileRemote.workspacesShared}}: the phone sees only the workspaces [{{ui:settings.remote.shareTitle}}](remote.md#share) is sharing right now. With sharing off it sees no workspace at all; it cannot open another workspace's panes, files or work map or send to them, and gets no notifications from their panes.

A change applies from the phone's next request; the connection stays up.

## Connect directly over Tailscale {#direct}
When the Mac and the phone are on the same [Tailscale](https://tailscale.com) network, the phone connects straight to the Mac without the relay. Pairing and encryption stay the same, and when the Mac can't be reached directly the phone uses the relay on its own.

1. Turn on Tailscale on the Mac and the phone and sign in with the same account.
2. In the {{ui:settings.remote.phoneTab}} tab on the computer, check that {{ui:mobileRemote.direct.toggle}} is on (it is by default). When the line below it shows Tailscale addresses, you're set. If it says {{ui:mobileRemote.direct.status.notFound}}, Tailscale is off on the Mac.
3. Phones you already paired don't need pairing again. The Mac tells them its addresses the next time they connect.

On the phone, the Mac's card in the {{ui:phone.tabs.hosts}} tab shows {{ui:phone.direct.route.direct}} or {{ui:phone.direct.route.relay}}.

It works the same with a Windows PC, which needs one Windows Firewall rule so the connection isn't blocked. In the {{ui:settings.remote.firewallTitle}} group under the {{ui:settings.remote.phoneTab}} tab, click {{ui:remote.windows.firewall.add}} and allow Windows' administrator prompt. The rule lets only connections from Tailscale addresses reach this app. Once the rule exists, {{ui:remote.windows.firewall.update}} fits it to the ports in use now and {{ui:remote.windows.firewall.remove}} removes it. Turning the direct connection or phone access off removes the rule too (it stays while this computer is [sharing workspaces](remote.md#share), which still need its port), and Windows asks for administrator approval again. It is one rule that opens both the phone's direct port and the workspace-sharing port, so a change on the {{ui:settings.remote.shareTitle}} tab changes the same rule ([Windows Firewall](remote.md#firewall)).

## What you can do on the phone {#use}
The phone app has three tabs.

| Tab | What it shows |
|---|---|
| {{ui:phone.tabs.dashboard}} | {{ui:phone.dashboard.title}}: the counts of running, waiting and finished work across every paired computer, then each computer's workspaces and sessions in turn. Open a session to read the conversation and send requests. |
| {{ui:phone.tabs.alerts}} | Approvals and questions waiting for you, and finished work |
| {{ui:phone.tabs.hosts}} | Your paired computers (Mac or Windows) and whether they are connected |

Inside a session you can send and stop requests, change the model, permission and thinking effort, run slash commands, attach files and browse the workspace's files (read-only).

While a session's task runs, the send button becomes {{ui:queue.add}}, and what you send waits in the queue as that pane's next request. Nothing you send goes into the running task.

Queued requests show a {{ui:queue.runNow}} button when the computer's app is new enough. It runs the request in a new worktree child pane on the computer, or in this pane, as the computer decides. A greyed-out one says why when you tap it. The phone does not pick the agent.

When the computer's app supports editing the queue, each request also shows {{ui:queue.edit}} and ↑ ↓ buttons. {{ui:queue.edit}} edits the words in place and {{ui:queue.editSave}} saves them (empty words are not saved). ↑ and ↓ change the order, and the computer runs the queue in that order. Rows the computer holds for the pane cannot be edited, moved or removed.

In the {{ui:phone.session.view.blocks}} view a finished request shows the request and its {{ui:phone.blocks.finalResult}} at once. Its steps are folded into one row that opens when you tap it. A request still running shows every step.

When the result names files in the workspace, a {{ui:graph.resultFiles.title}} list hangs below the {{ui:phone.blocks.finalResult}}. It holds the same files as the list beside the result in the desktop app, each with its folder dimmed under its name. A long list shows its first five files and opens the rest with the button below them. Tap a file to open it in the workspace file view. The list comes only with a successful result, and it does not show for a workspace on another computer or with an older desktop app that does not send it. When the request changed that source or Markdown file, {{ui:files.changes.show}} | {{ui:files.changes.full}} chips appear at the top and it opens on {{ui:files.changes.show}}: only the changed lines, with old and new line numbers, added lines on green and removed lines on red. This view comes only from a desktop app that supports it.

The {{ui:phone.session.view.structure}} view shows the pipeline the desktop app sends as boards, one at a time. In a pane without a style each request is a board; in a pane with a style each pipeline is a board: a run of requests whose step never goes back. ◀ and ▶ at the top move between boards, and tapping the place in the middle ("Request 3 / 7") opens the list to pick one. When a new board arrives while you read an older one, a {{ui:graph.structure.board.newRequest}} or {{ui:graph.structure.board.newPipeline}} badge appears; tap it to go to the newest board. Below them are the board's status and its steps as a row of chips: finished steps are green, the current step is in the accent colour and the steps still to come are grey, each with its card count. Each step has its own column; swipe sideways or tap a chip to move between them, and the board opens on its current step. A pane without a style has the columns {{ui:graph.structure.column.request}} → {{ui:graph.structure.column.execution}} → {{ui:graph.structure.column.result}}, with the sub-agents' cards after the {{ui:graph.structure.column.execution}} card. In a pane with a style each column holds its step's requests and their sub-agents, and a finished request folds to one card that opens when you tap it. A running card's border pulses. This view needs the desktop app (Mac or Windows) on the same version as the phone app or newer; with an older one the view asks you to update the desktop app.

A child pane made in the desktop app (Mac and Windows) by [Mighty Orchestration](delegation.md) is listed one step in under its parent pane, with its state ({{ui:delegation.child.state.reported}}, {{ui:delegation.child.state.merged}} and so on) on the right. An older app doesn't send this, so its child panes are listed like any other pane. Merging or discarding a child can't be done from the phone; it is done in the desktop app (Mac or Windows). Only the children of a compare run can be merged or discarded from the phone, in the compare view described below. Gemini panes of a desktop app that runs Antigravity (Mac 0.4.23 or later, Windows 0.4.28 or later) are shown as Antigravity ({{ui:badge.beta}}); those of an older app are shown as Gemini.

A pane that works in its own worktree shows the worktree bar at the top of its screen. Next to PORT you see the port range given to that worktree (for example `:41010–41019`), and under it the [run actions](delegation.md#mighty-yaml) running there with their dev-server addresses. An address is the computer's own, so tapping it only copies it. With a computer on Mighty Code 0.6.0 or later, {{ui:phone.worktree.runStart}} and {{ui:phone.worktree.runStop}} start and stop those run actions from the phone. Only the run actions of a `.mighty.yaml` you approved on the computer are offered (the phone sends a name, never a command), nothing runs in an SSH or remote workspace, and each start or stop leaves a line in the pane on the computer. When the computer is on [Tailscale](#direct) and the dev server also listens on its Tailscale address (for example a server started with `--host 0.0.0.0`), the address shows the Tailscale one and tapping it opens the server in the phone's browser; the phone has to be on the same Tailscale network. A server that listens only on `localhost` is never exposed: its address is copied as before. Tap {{ui:git.checks.tab}} to see, read-only, what the desktop app's checks tab shows: uncommitted changes, the PR's state, CI checks, unresolved review threads and remaining to-dos. When you merge and these checks still warn about something, the phone lists it and asks once more; tap {{ui:git.checks.merge.confirm}} to merge.

A pane that started a [compare run](delegation.md#compare) shows a compare card under the bar. Tap it to open {{ui:compare.view.title}}: each child's agent and model, state, changed files, time, cost and test result. You can {{ui:compare.action.merge}} a child that has reported, or {{ui:compare.action.discard}} a child. After a merge the phone asks whether to discard the other children at once ({{ui:compare.rest.title}}).

A pane the desktop app marks as waiting for you (a terminal notification or bell, an approval or question card, a failed setup) gets an orange dot after its title in the session list, and the {{ui:phone.tabs.dashboard}} tab shows how many such panes there are in a line under its counts. Tap the line to open the pane marked last. A [forked pane](delegation.md#fork) shows a link to the pane it came from, and a pane running in the sandbox shows a {{ui:sandbox.badge}} badge and the last thing the sandbox blocked. These marks need a desktop app that sends them (0.6.0 or later).

## Approvals and questions {#approvals}
When an agent asks for approval or asks a question, the {{ui:phone.tabs.alerts}} tab shows a count.
- Permission requests: tap {{ui:permission.allow}} or {{ui:permission.deny}}.
- Antigravity permission card: each declined action shows its kind and its command, address or path. Choose {{ui:permission.allowOnce}}, {{ui:antigravity.permission.allowAlways}} or {{ui:antigravity.permission.deny}}; allowing continues the same conversation ([Antigravity's permission card](approvals.md#antigravity-permission)). A card with only actions no rule can allow offers just {{ui:antigravity.permission.deny}} and tells you to change the permission mode. When a command chains, pipes, redirects or may delete data, {{ui:antigravity.permission.allowAlways}} is not shown. If the host cannot take your answer, the card stays with the reason. An older phone app that does not know the card shows it as an ordinary permission request.
- Questions: pick your answers and tap {{ui:phone.questionnaire.submit}}.
- Plan cards: answer with the same buttons as on the Mac ({{ui:plan.card.approveAuto}}, {{ui:plan.card.revise}} and so on).

## Notifications {#notifications}
Your phone gets a notification when a run on a connected computer (Mac or Windows) finishes or fails, and when an agent needs you: a permission request, a question, a plan to approve or an expired sign-in. Only the Android app gets them for now.
- The first time you open the app (Android 13 or later) it asks once whether to allow notifications. To turn kinds off, open {{ui:phone.settings.title}} on the {{ui:phone.tabs.hosts}} tab and change the {{ui:phone.settings.notifications}} switches. If notifications for the app are off in the system settings, {{ui:phone.settings.title}} tells you so.
- On a permission request you can tap {{ui:permission.allow}} or {{ui:permission.deny}} right in the notification. An Antigravity card offers {{ui:permission.allowOnce}}, {{ui:antigravity.permission.allowAlways}} and {{ui:antigravity.permission.deny}}. A card that can only be allowed once, such as a chained command or one that can delete data, has no {{ui:antigravity.permission.allowAlways}}. Question and plan notifications have no buttons; tap them and answer in the app. An Antigravity card with nothing that can be allowed also comes without buttons; tap it and answer in the app.
- The buttons work only after you unlock the phone. If you tap one while it is locked, the notification changes to "{{ui:phone.push.outcome.locked}}" and keeps its buttons. If you already answered on the computer it changes to {{ui:phone.push.outcome.already}}, and if the answer could not be sent it changes to {{ui:phone.push.outcome.failed}}. In that last case, tap the notification and answer in the app.
- You can turn notifications off for each workspace. Tap the bell next to a workspace's name to turn its notifications off; the bell is then struck through. Tap it again to turn them back on. Every workspace starts on, and the choice is for this phone only, so other phones connected to the same computer still get them. A notification can still arrive if the app has not seen that workspace yet, or has not had the computer's list for over a day.
- Tapping a notification opens that session. You get no notifications about a session while it is on your phone's screen.
- Notifications are private on the lock screen: if you chose to hide sensitive content there, the title and text are hidden.
- The computer encrypts each notification so only this phone can read it; the relay and Google can't. Notifications go through the relay, so over a direct Tailscale connection they still arrive, as long as the computer can reach the internet.
- Both the desktop app and the phone app need a version with this feature. A desktop app without it sends no notifications. If you paired with a very old phone app, pair again.
- The default relay moved to a new server in 0.4.58. If a phone paired before then gets no notifications, pair it again once with the QR code.
- While phone access is off on the computer it sends nothing, and when you turn it back on it does not report what happened in the meantime.

When the app crashes it sends the developers only where it stopped, never conversations, commands or paths. You can turn {{ui:phone.settings.crashReports}} off in {{ui:phone.settings.title}}.

## Screen view {#screen}
Watching a computer's screen from the phone and, if you allow it, controlling it is a {{ui:badge.beta}} feature.

> [!warning]
> Since 0.4.58, screen view & control is paused (on the Mac and on Windows). The phone offers no screen button, and Settings on the computer has nothing for screen view. The access you set for each phone and the permissions you granted are kept and come back when it returns.

## Desktop app update {#desktop-update}

When the Mac or Windows app has a new version, a **{{ui:phone.appUpdate.title}}** strip appears above that computer's session list on the phone. Press **{{ui:phone.appUpdate.install}}** and the computer downloads the new version, installs it and restarts the app. You do not need to be at the computer.

- When requests are running, it first tells you how many will stop and goes on only once you confirm.
- The connection drops for a moment while the app restarts and comes back by itself.
- The computer downloads the update from the official address and installs it only after checking its signature. The phone only tells it to start.
- This works with computers on 0.4.55 or later.

## Change the relay address {#own-relay}
The default relay (Cloudflare) is built into the app, so usually there is nothing to change. To use another relay, open the folded {{ui:settings.advancedTitle}} group at the bottom of the {{ui:settings.remote.phoneTab}} tab.
- {{ui:settings.mobileRemote.relayLabel}}: enter the relay's address (`wss://…`) and click {{ui:settings.mobileRemote.applyButton}}. Leave it empty to use the default relay.
- {{ui:settings.mobileRemote.legacyAppsToggle}}: off by default. Turned on (after a warning), very old phone apps without a device token connect with the {{ui:settings.mobileRemote.legacyKeyLabel}} shown below it alone, without approval. Anyone who knows that key can control this computer remotely, so turn it on only when you need it and off again afterwards. The current app does not need it.
