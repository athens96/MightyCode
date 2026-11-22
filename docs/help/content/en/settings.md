---
title: Settings
order: 8
section: settings
---
Open {{ui:menu.settings}} ({{kbd:⌘,}}) or the settings button at the bottom of the sidebar. Pick a tab on the left, and close with {{ui:settings.closeButton}} or {{kbd:Esc}}. The last tab you viewed is remembered. The tabs are, in order: {{ui:settings.nav.general}}, {{ui:settings.nav.models}}, {{ui:settings.nav.styles}}, {{ui:settings.remote.navTitle}}, {{ui:settings.nav.companion}}, {{ui:settings.nav.tools}} and {{ui:settings.nav.about}}.

## General {#general}
Under {{ui:settings.display.sectionTitle}} on the {{ui:settings.nav.general}} tab you set how the app looks.
- {{ui:settings.display.themeLabel}}: {{ui:settings.display.themeDarkWindows}} or {{ui:settings.display.themeLightWindows}}. The button at the bottom of the sidebar switches it too.
- {{ui:settings.display.languageLabel}}: the app's language. See [Change the language](getting-started.md#language).

![[settings-general]]

### Text size and font {#text}
The {{ui:settings.text.sectionTitle}} group on the same tab changes the text of the whole app.
- {{ui:settings.text.sizeLabel}}: pick from 85% to 150%. Conversations, the composer, the sidebar, Settings and terminals grow and shrink together.
- {{ui:settings.text.uiFontLabel}}: the font of the app itself, such as the sidebar, menus, buttons and Settings. Pick any font installed on this computer; {{ui:settings.text.fontSystem}} is the system font (the macOS font on a Mac, Segoe UI on Windows).
- {{ui:settings.text.transcriptFontLabel}}: the font of agent replies, your own messages, the composer, and the body text of Mighty cards and boards. {{ui:settings.text.sameAsInterface}} follows the interface font.
- {{ui:settings.text.codeFontLabel}}: the font of code blocks, inline code, tool output and command rows, and terminal panes. Only monospaced fonts are listed: the installed fixed-pitch fonts, plus coding fonts such as JetBrains Mono, D2Coding, Fira Code, Cascadia Code, Cascadia Mono and Consolas when installed. The default at the top uses SF Mono for code and Menlo for terminals on a Mac, and Cascadia Mono (Consolas without it) on Windows. On a Mac, if the chosen font is removed, terminals fall back to Menlo. Open terminals change at once. Paths, versions, counts and other monospaced text in the app itself keep the system monospaced font.
- Under each font, a one-line preview is drawn in that font.
- {{ui:settings.text.reset}} puts the size and all three fonts back as they were.

On a Mac the View menu's {{ui:menu.textBigger}} ({{kbd:⌘=}}), {{ui:menu.textSmaller}} ({{kbd:⌘-}}) and {{ui:menu.textActualSize}} ({{kbd:⌘0}}) change the size too, and on Windows {{kbd:Ctrl+=}} / {{kbd:Ctrl+-}} / {{kbd:Ctrl+0}} (while the cursor is in a terminal pane on Windows, the terminal takes these keys). A change applies at once and stays after the app is reopened.

### Agent panes {#agents}
The {{ui:settings.agents.sectionTitle}} group on the same tab holds the settings for agent panes and a few key tips.
- {{ui:settings.display.statusLineToggle}}: runs the status line command from your Claude settings and shows it under the composer.
- {{ui:settings.delegation.toggle}}: on by default. Claude panes can open child panes in their own worktrees, and {{kbd:⌘Enter}} in a running Claude pane runs your request in a new one. See [Mighty Orchestration](delegation.md#enable).
- {{ui:settings.agents.checklistToggle}}: on by default. While on, Gemini, Codex, opencode and Hermes get a short instruction to keep a plan (a checklist) while they work, so the progress panel shows real plan steps. The pane's request keeps only what you typed. When off, or when no plan comes, the panel shows what the agent does as steps ([Mighty View](agent-pane.md#diagram)).
- {{ui:settings.editor.label}}: what {{ui:editor.open}} starts. Choose Auto (the first one found), one of the editors found on this computer, or {{ui:settings.editor.system}}. The editors are looked for again each time Settings opens ([open in an editor](terminal-files-browser.md#open-in-editor)).
- The tips below them: {{kbd:Enter}} sends and {{kbd:Shift+Enter}} starts a new line. Drag a tab onto another to merge them, or to an edge to split. The command pane is an interactive terminal that opens your login shell on a Mac, and PowerShell (or cmd if it is missing) on Windows.

## Models {#models}
{{ui:settings.phaseModels.sectionTitle}} on the {{ui:settings.nav.models}} tab sets which model each phase of work uses, for this whole computer. The phases are {{ui:settings.phaseModels.phase.planning}}, {{ui:settings.phaseModels.phase.execution}}, {{ui:settings.phaseModels.phase.review}} and {{ui:settings.phaseModels.phase.subagents}}.
- Leave a phase at {{ui:settings.phaseModels.defaultOption}} and the CLI decides.
- {{ui:settings.phaseModels.paneModel}} uses the model picked on the pane's model chip.
- Model names that aren't listed can be added under {{ui:settings.phaseModels.registeredTitle}} with {{ui:settings.phaseModels.addButton}}. If the model supports effort, turn on {{ui:settings.phaseModels.supportsEffortLabel}}.

![[settings-models]]

## Styles {#styles}
The {{ui:settings.nav.styles}} tab manages your list of styles: register styles from files, review them, and allow or block them. If the list looks wrong, click {{ui:settings.styles.rescanButton}}. Hover over a style's name to see its id, hash and counts. See [Styles](styles.md#custom) for details.

![[settings-styles]]

## Remote connection {#remote}
The {{ui:settings.remote.navTitle}} tab lets other computers (Mac or Windows) and phones use this computer's workspaces, or opens the workspaces of another computer. Under a line of introduction at the top there are three tabs.
- {{ui:settings.remote.shareTitle}}: turn sharing on and off with {{ui:settings.remote.startSharing}} / {{ui:settings.remote.stopSharing}} beside the status line at the top. Pick the {{ui:settings.remote.pickWorkspaces}}, and see the {{ui:settings.remote.viaPairing}} and {{ui:settings.remote.viaTailscale}} rows under {{ui:settings.remote.methodsTitle}}. See [Share this computer](remote.md#share).
- {{ui:settings.remote.connectTitle}}: connect to another computer with a pairing link or a Tailscale address, and manage your connections in the {{ui:settings.remote.connectionsTitle}} list below. See [Connect to another computer](remote.md#connect).
- {{ui:settings.remote.phoneTab}}: pair a phone. See [Phone](#mobile) below.

{{platform:windows}} Under the {{ui:settings.remote.shareTitle}} and {{ui:settings.remote.phoneTab}} tabs there is a {{ui:settings.remote.firewallTitle}} group. See [Windows Firewall](remote.md#firewall).

### Phone {#mobile}
The {{ui:settings.remote.navTitle}} › {{ui:settings.remote.phoneTab}} tab lets phones connect, pairs a phone with a QR code, and manages connected devices. There is no relay to set up. See [Phone remote](phone.md#pair). The folded {{ui:settings.advancedTitle}} group holds the {{ui:settings.mobileRemote.relayLabel}} field for a relay you run yourself (with its {{ui:settings.mobileRemote.applyButton}} button and the default address under it) and the {{ui:settings.mobileRemote.legacyAppsToggle}} switch. It starts open when either was changed from its default.

Screen view & control is paused for now, so Settings has nothing for it ([Screen view](phone.md#screen)).

## Mighty Pet & alerts {#companion}
The {{ui:settings.nav.companion}} tab sets up Mighty Pet and the completion notification.
- {{ui:companion.settings.enabled}}: Mighty Pet, a raccoon that walks on your screen. Drag it to move it; click it to open or close its bubble.
- {{ui:companion.settings.task}}: the bubble shows the request, the current step and the time taken. You can also answer approvals there ({{ui:permission.allowOnce}} / {{ui:permission.deny}}). The symbol button on an approval, question or plan bubble opens that pane, where you see the whole request or type your own answer.
- Over the bubble sits the {{ui:graph.progress.title}} card of that pane's latest request: the same as the pane's progress panel, with the request number, the steps done, its state, every step and piece of background work, each with its time. With more than eight lines the card's list scrolls inside it, opening on the current step. Click the card's header to fold it to the header and again to unfold it; it stays folded the next time the app opens. Click a step line to open that pane. A request with no steps and no background work has no card, and neither does a bubble that shows an approval, question or plan, or a closed bubble.
- {{ui:companion.settings.pet}}: choose the pet. Add more with {{ui:companion.settings.import}}, read the pets again with {{ui:companion.settings.reload}}, and put Mighty Pet and its bubble size back where they started with {{ui:companion.settings.resetPosition}}.
- {{ui:companion.settings.motion}}: less movement.
- {{ui:companion.settings.macNotifications}} ({{ui:windows.notifications.toggleLabel}} on Windows): a Mac notification when a request finishes successfully. Clicking it opens that pane. The notification never contains the request or its result.
- {{ui:terminalNotifications.toggle}}: marks a pane when its terminal or CLI sends a notification or rings the bell. An OS notification is sent only while the app is in the background or you are looking at another pane. On at first ([Notifications and the bell](terminal-files-browser.md#terminal-signals)).
- Under it, the system's notification permission is shown, and {{ui:windows.notifications.settingsButton}} opens the system's notification settings.

![[settings-companion]]

![[companion-pet]]

## Components {#components}
The {{ui:settings.nav.tools}} tab shows whether everything the app needs is ready, and is where you install, sign in to and update the agent CLIs. Only the items for this computer's system are shown.

The top line has a view picker and two buttons.
- {{ui:settings.components.filterAll}} / Needs attention: with Needs attention, only the rows that need something are shown: an item not installed, not signed in, with an update, failed or needing a check. The number beside it is how many.
- Refresh (↻, {{ui:setup.button.recheck}}): reads PATH again and checks every item again.
- {{ui:settings.components.updateAllButton}}: updates every installed agent CLI now, each the way it was installed (npm, Homebrew and so on).

The list has three groups: {{ui:setup.group.agent}} (Claude, Codex, Antigravity, pi, opencode and Hermes), {{ui:settings.components.groupTools}} (Git (on a Mac, the Xcode command line tools), Homebrew (Mac), Node.js, Bun (Mac), uv, Tailscale, gh and glab) and {{ui:settings.components.groupPlugins}} (ouroboros, omc and gstack (Mac)). Each item is one row: a disclosure mark (▸), its name, its version and, for an agent CLI, the signed-in account. On the right are its status and the one button it needs most.
- Status: {{ui:setup.status.installed}}, {{ui:settings.components.statusInstalled}} (an agent CLI installed and signed in), {{ui:settings.components.statusUpdateAvailable}}, {{ui:settings.cliAccounts.summarySignedOut}}, {{ui:settings.cliAccounts.statusNotInstalled}}, {{ui:setup.status.attention}}, {{ui:setup.status.installing}}, {{ui:setup.status.failed}}.
- Button: {{ui:settings.components.installButton}} when it is missing ({{ui:setup.button.openPage}} for what only a download page installs), {{ui:settings.cliAccounts.buttonLogin}} when it is not signed in, {{ui:setup.button.update}} when it is out of date. A row with nothing to fix has no button.

Click a row to open it in place (▾). The opened row shows the details (which version is needed, the last update's result, the account, why it needs a check, what the item is for) and every button of the item, for an agent CLI {{ui:settings.cliAccounts.buttonChange}}, {{ui:settings.cliAccounts.buttonLogout}}, {{ui:settings.cliAccounts.refreshTooltip}} and so on. Click it again to close it. The rows you opened stay open until you quit the app.

Under the list, the {{ui:settings.components.groupAutoUpdate}} box has three switches and the last run's time. Point at a switch to see what it does.
- {{ui:settings.components.autoUpdateClis}}: checks for agent CLI updates when the app opens, and every 6 hours.
- {{ui:settings.components.autoUpdatePlugins}}: updates Claude and Codex plugins too.
- {{ui:settings.components.autoLogin}}: when a sign-in expires, signs in again and resends the failed request.

{{ui:settings.components.installButton}} installs only that item and the prerequisites it is missing. Every command is shown in one confirmation first.
- {{platform:mac}} It runs in a terminal pane in the app, or in Terminal.app when no local workspace is open.
- {{platform:windows}} It runs in a separate PowerShell window.

Whether the item got installed is decided by checking again afterwards. The [{{ui:setup.title}}](getting-started.md#setup) window that opens on first run is a checklist: pick what is missing and click {{ui:setup.button.install}}.

At the bottom of the same tab is the {{ui:settings.toolkit.sectionTitle}} box: a list of tools you gather yourself, empty at first. Add a tool with {{ui:settings.toolkit.addButton}}; each entry has {{ui:settings.toolkit.approveButton}} and {{ui:settings.toolkit.removeButton}} buttons. Only approved tools can be installed. Every command is shown before anything is installed, and you get a {{ui:settings.toolkit.confirmTitle}} step. You can {{ui:settings.toolkit.exportButton}} or {{ui:settings.toolkit.importButton}} the list.

![[settings-tools]]

### Sign in {#cli-accounts}
You sign in to each agent CLI on its row. A CLI that is not signed in shows {{ui:settings.cliAccounts.buttonLogin}} right on its row; its other account buttons are in the opened row.
1. Click {{ui:settings.cliAccounts.buttonLogin}}. For Claude, choose {{ui:settings.cliAccounts.buttonLoginClaude}} or {{ui:settings.cliAccounts.buttonLoginConsole}}.
2. The CLI's sign-in starts in a terminal. Approve it in the browser.
   - {{platform:mac}} A terminal pane opens in the app. It opens in a local workspace, so at least one workspace must be open.
   - {{platform:windows}} A separate console window opens. No workspace is needed.
3. When you're signed in, the row's status changes. If it doesn't, open the row and click {{ui:settings.cliAccounts.refreshTooltip}}. A changed account is used from the next request.

To switch accounts open the row and click {{ui:settings.cliAccounts.buttonChange}}; to sign out click {{ui:settings.cliAccounts.buttonLogout}}. Turn on {{ui:settings.components.autoLogin}} in the {{ui:settings.components.groupAutoUpdate}} box to sign in again automatically when the sign-in drops and resend the failed request.

The opened Claude row also has {{ui:settings.cliAccounts.resetModelsButton}} and {{ui:settings.cliAccounts.bedrockButton}}. {{ui:settings.cliAccounts.bedrockButton}} opens the Claude CLI's AWS Bedrock setup wizard in a terminal, the same way as a sign-in; refresh the status when you are done. {{platform:windows}} Windows has one more button, {{ui:settings.cliAccounts.resetBedrockButton}}, which backs up your Claude user settings and then deletes the Bedrock key and the Bedrock model settings.

### Updates {#cli-update}
The agent CLIs are updated separately from the app.
- {{ui:settings.components.updateAllButton}}: updates every installed agent CLI now. {{ui:setup.button.update}} on an out-of-date CLI's row runs the CLI update too.
- {{ui:settings.components.autoUpdateClis}} in the {{ui:settings.components.groupAutoUpdate}} box: checks when the app opens, and every 6 hours.
- {{ui:settings.components.autoUpdatePlugins}}: updates Claude and Codex plugins too.

Each CLI is updated the way it was installed (npm, Homebrew and so on). The last update's result for each CLI shows when you open its row, and the plugin updates' results under the {{ui:settings.components.groupPlugins}} group.

## About {#about}
The {{ui:settings.nav.about}} tab updates the app and shows where its data is kept.
- {{ui:settings.appUpdate.sectionTitle}}: the current version, the automatic-update switch, the state and its button, and the release notes. See [Updates](getting-started.md#update). The folded {{ui:settings.advancedTitle}} group holds the {{ui:settings.appUpdate.manifestUrlAddressLabel}} field and a note about signatures; it is open by itself when an address was entered.
- {{ui:settings.appInfo.sectionTitle}}: the {{ui:settings.appInfo.openHelpButton}} button and the {{ui:settings.appInfo.stateLocationLabel}}. The {{ui:settings.appInfo.stateLocationLabel}} is the folder with the app's settings and history; open it with {{ui:menu.showInFinder}} on a Mac or {{ui:menu.showInExplorer}} on Windows.

![[settings-about]]
