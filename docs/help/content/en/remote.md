---
title: Remote connection
order: 10
section: remote
---
Link two computers' Mighty Code (Mac or Windows), and one computer can open the other's workspaces and run Claude, Codex, Antigravity and commands on that computer. Files and CLI sign-ins stay where the work runs; no project files are copied or synced.

There are two ways to link them.
- {{ui:settings.remote.viaPairing}}: pair once, as a phone does, and connect through the Mighty relay. No Tailscale is needed. See [Connect by pairing](#pair).
- {{ui:settings.remote.viaTailscale}}: connect straight to the other computer with its address and key inside the same [Tailscale](https://tailscale.com/kb/1017/install) network (tailnet). Remote panes that run this computer's panes there, and large attachments, need Tailscale.

When the other computer can't run Mighty Code, work in its folders as an [SSH machine](#ssh).

Everything happens in {{ui:menu.settings}} → the {{ui:settings.remote.navTitle}} tab. A line of introduction sits at the top, with the {{ui:settings.remote.shareTitle}} | {{ui:settings.remote.connectTitle}} | {{ui:settings.remote.phoneTab}} tabs below it. The inner tab you chose is remembered only while the app runs. For phones, see [Phone remote](phone.md).

## Before you start {#prepare}
- Install Mighty Code on both computers.
- On the computer that will do the work (the host), install and sign in to the CLIs you will use. Remote runs use the host's CLIs and accounts.
- To connect over Tailscale, install Tailscale on both computers and sign both into the same tailnet. No router port forwarding and no Tailscale Serve or Funnel are needed. Just make sure your tailnet policy lets the two devices reach each other.

## Share this computer {#share}
On the host computer:
1. Open {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → the {{ui:settings.remote.shareTitle}} tab.
2. Under {{ui:settings.remote.pickWorkspaces}}, check the local workspaces to share.
3. Under {{ui:settings.remote.methodsTitle}}, get the methods you want ready (see below).
4. Press {{ui:settings.remote.startSharing}} beside the status line at the top. While sharing, the {{ui:settings.remote.sharedListTitle}} list takes its place.

{{ui:settings.remote.methodsTitle}} has one row per method.
- {{ui:settings.remote.viaPairing}}: the {{ui:settings.remote.pairAccept}} switch and the relay's state; turning the switch on brings up the {{ui:settings.remote.pairCopyLink}} button. The link carries a pairing code for computers only (not the phone's QR code), so it is copied as a secret and cleared from the clipboard after a minute. In a run that cannot use the relay, the switch is greyed out and says why. See [Connect by pairing](#pair).
- {{ui:settings.remote.viaTailscale}}: when this computer has no Tailscale address, a line says so with an {{ui:settings.remote.tailscaleInstall}} button that opens the {{ui:settings.nav.tools}} tab. Before sharing, there is a {{ui:settings.remote.portLabel}} field; leave it as it is (43137) unless you need another. While sharing, this computer's Tailscale addresses and the {{ui:settings.remote.keyLabel}} appear. Show the key, copy it with {{ui:settings.remote.copyButton}}, and give it to the other computer.

Without Tailscale, sharing still starts when {{ui:settings.remote.pairAccept}} is on; then only paired computers connect, through the relay.

While sharing, the {{ui:settings.remote.allowFullAccess}} switch appears. While it is off (the default), connected computers cannot use remote command panes or run agents with full access; such requests are refused before they run, and the pane says why. It turns off again whenever sharing starts.

While this switch is off, connected computers also cannot use an agent pane that the sharing computer cannot confine to the shared folder, whatever its permission mode: on a Mac, Antigravity, pi, opencode and Hermes (none of them runs in a sandbox); on Windows, every agent other than Claude, Codex included (Windows cannot confine Codex in a sandbox the way a Mac does). Codex on a Mac runs in its own sandbox, and Claude panes on either computer still run and ask for permission as usual. In such a pane, allowing a tool, answering a question and approving a plan need the switch too, not only sending.

Tailscale sharing accepts connections on this computer's Tailscale addresses only. Sharing is always off when the app starts. {{ui:settings.remote.stopSharing}} stops the remote runs and revokes the key; sharing again gives a new key to pass on.

> [!warning]
> A connected device can run agents and commands in the shared workspaces. Depending on the permission mode, it may reach outside the folder with the host user's rights. A device with the Tailscale connection key connects straight away, so give the key only to devices you trust. A pairing link works once, and connects only after you click {{ui:settings.mobileRemote.approval.allow}} on the host.

## Connect to another computer {#connect}
On the computer that gives the work:
1. Open {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → the {{ui:settings.remote.connectTitle}} tab. {{ui:remote.menu.connect}} in the {{ui:workspace.addPane}} menu opens the same place.
2. In the two-way switch at the top, pick {{ui:settings.remote.pairLinkField}} or Tailscale address.
   - {{ui:settings.remote.pairLinkField}}: paste the link from the host and, if you like, give it a {{ui:settings.remote.nameField}}.
   - Tailscale address: enter the {{ui:settings.remote.addressField}} (its Tailscale IP or MagicDNS name) and the {{ui:settings.remote.keyField}}, and a name if you like. Unless the host changed the port, the address alone is enough.
3. Press {{ui:settings.remote.connectButton}}.

The {{ui:settings.remote.connectionsTitle}} list below has one row per connection: the computer's name, a method chip (Pairing or Tailscale), its state ({{ui:settings.remote.status.connected}}, {{ui:settings.remote.status.disconnected}} or {{ui:settings.remote.status.saved}}), the {{ui:settings.remote.refreshButton}} and {{ui:settings.remote.disconnectButton}} buttons, and a delete icon. The shared workspaces appear under the connected computer. Once connected, that computer's shared workspaces appear in the sidebar on their own, with the panes open there; {{ui:remote.mirror.view}} takes you straight to one ([The other computer's panes as they are](#mirror)). The folded {{ui:settings.remote.scopeTitle}} group sits at the bottom of the tab.

A remote workspace shows a computer icon and its host's name in the sidebar, and its connection state in the workspace header and the status bar. After the app restarts, the connection stays saved and connects again on the first send from a remote pane or when you press {{ui:settings.remote.refreshButton}}.

{{platform:mac}} The key is kept in the macOS Keychain only, never in the workspace state. If macOS asks for Keychain access when it connects again, choose "Always Allow"; the app ships with the same signature, so the choice survives updates.

{{platform:windows}} On Windows the key is never kept in the workspace state either: it is saved encrypted so only this Windows user can read it, in the app's data folder. Sharing over Tailscale needs one firewall rule ([Windows Firewall](#firewall)).

## Connect by pairing (no Tailscale) {#pair}
Without Tailscale on either computer, pair once as a phone does and connect through the same Mighty relay as the [phone remote](phone.md). The relay is built into the app, so there is nothing to set up. The connection is end-to-end encrypted, so the relay cannot read it, and the connected computer uses [the other computer's panes as they are](#mirror).

On the host computer:
1. Open {{ui:menu.settings}} → {{ui:settings.remote.navTitle}} → the {{ui:settings.remote.shareTitle}} tab and, in the {{ui:settings.remote.viaPairing}} row under {{ui:settings.remote.methodsTitle}}, turn on {{ui:settings.remote.pairAccept}}.
2. Pick the workspaces and press {{ui:settings.remote.startSharing}}. While it shares, this computer connects to the relay, and the same row shows its state.
3. Press {{ui:settings.remote.pairCopyLink}} and give the link to the computer that connects. The link is for computers only (a phone's QR code cannot pair a computer, nor the other way round), works once, and expires 10 minutes after you close this screen. When the other computer connects, this computer shows a "new device" window and a notification: click {{ui:settings.mobileRemote.approval.allow}} if it is the one you expect (with no answer in 60 seconds it is refused).

On the computer that connects:
1. In {{ui:settings.remote.navTitle}} → the {{ui:settings.remote.connectTitle}} tab, pick {{ui:settings.remote.pairLinkField}} in the switch, paste the link, name it if you like, and press {{ui:settings.remote.connectButton}}. Until the host allows it, a line asks you to allow it there.
2. Once connected, the computer appears in the {{ui:settings.remote.connectionsTitle}} list with a Pairing chip and in the sidebar as "Remote · name". Connecting and disconnecting work as for a Tailscale connection.

- The link carries a short-lived pairing code for computers only. Even if someone else gets it, nothing connects unless you allow it on the host. On the first connection the host hands this computer a device token of its own, which it uses from then on without asking again. The token is kept like a connection key, in the macOS Keychain (on Windows, in a file encrypted for this user only), never in the workspace state. While {{ui:settings.remote.pairAccept}} is off on the host, or it is not sharing, no new computer connects.
- A paired computer also appears, with a computer icon, in the {{ui:settings.mobileRemote.connectedDevicesTitle}} list on the host's {{ui:settings.remote.phoneTab}} tab (even with the phone connection switch off, and after a restart). {{ui:settings.mobileRemote.revokeRowButton}} on its row unpairs only that computer; your other phones and computers stay connected. Pressing {{ui:settings.mobileRemote.regenerateKeyButton}} unpairs every phone and computer and makes the host's relay ID and encryption key anew; connect again with a new link to use it again.
- Only the shared workspaces are visible, and the {{ui:settings.remote.allowFullAccess}} switch and the rule for agents that cannot be confined apply exactly as over Tailscale.
- The phone settings do not change. If phones are not allowed on the {{ui:settings.remote.phoneTab}} tab, phones cannot connect even with this switch on.
- The relay carries at most 1 MB at a time. Attachments sent over a paired connection are limited to about 750 KB in total (their contents grow in base64); larger ones are refused before anything is sent. Connect over Tailscale for large files. A long conversation shows its newest entries that fit in 1 MB.
- A paired connection offers [the other computer's panes as they are](#mirror) only. Remote panes that run this computer's panes there (the {{ui:settings.remote.openButton}} button) need a Tailscale connection.
- When the host stops sharing or turns the switch off, the connected computer says sharing stopped. Once the host shares again, press {{ui:settings.remote.refreshButton}} to reconnect; no new link is needed.

## Windows Firewall {#firewall}
{{platform:windows}} On Windows, a {{ui:settings.remote.firewallTitle}} group sits under the {{ui:settings.remote.shareTitle}} and {{ui:settings.remote.phoneTab}} tabs. It manages one rule that opens the app's ports only to Tailscale's ranges, and a line shows its current state.
- {{ui:remote.windows.firewall.add}} adds the rule. Windows shows its administrator prompt once.
- If the rule already exists, {{ui:remote.windows.firewall.update}} appears in its place and fits the rule to the ports in use now. If the rule does not open every port in use, the group says it needs updating.
- {{ui:remote.windows.firewall.remove}} removes the rule.

This one rule opens both the workspace-sharing port and the phone's [direct connection](phone.md#direct) port. So a change from either tab changes the same rule, and removing it closes both ports. A paired connection goes through the relay and needs no rule.

## The other computer's panes as they are {#mirror}
When you connect to another computer, the workspaces that computer shares appear in the sidebar with the agent panes open there. Conversations, the {{ui:graph.view.mighty}} views, progress and status follow live, and you can send, stop and answer approvals, questions and plans from here. The panes run on that computer with its CLIs and accounts; what you send becomes that pane's next request, queued there if it is busy.

- Under this computer's workspaces, the sidebar gets a "Remote · server name" header for each server, with its address and connection state. A green dot means connected, orange reconnecting, gray disconnected. The header's … menu has {{ui:remote.mirror.connect}} and {{ui:remote.mirror.disconnect}}.
- A remote pane carries a {{ui:remote.mirror.paneBadge}} strip showing which computer runs it, the connection state and how many requests wait in that computer's queue. The status bar reads like "Remote · Running on that computer · Connected".
- The queue above the composer is that computer's queue for the pane. Requests queued there, taken by its run or removed show up here right away. × and {{ui:queue.runNow}} act on that computer, by its rules; a request run now in a new worktree runs with that pane's agent. On an older computer without this, the queue is shown only.
- {{ui:workspace.addPane}} opens a new agent pane on that computer, and closing a pane closes it there too.
- After you disconnect, the panes stay as they last were and can't send. Removing the connection removes that server's workspaces. These panes aren't saved on this computer, so after reopening the app they come back once you connect.
- Conversations show up to the last 80 entries, without pictures. Rename panes and change models or permission modes on that computer. Command panes aren't shown, and [what remote panes can't do](#use) applies here too.
- Sending to a pane running with full access, and "Always allow", work only when that computer has {{ui:settings.remote.allowFullAccess}} on. Sending to, and answering for (allowing a tool, a question, a plan), an agent pane that computer cannot confine needs the switch too: on a Mac, Antigravity, pi, opencode and Hermes; on Windows, every agent other than Claude.
- If that computer's Mighty Code is an older version without this feature, its panes don't appear and the header says why. Use {{ui:settings.remote.openButton}} in the {{ui:settings.remote.connectionsTitle}} list to work in its workspaces as this computer's remote panes (over Tailscale).

## Using remote panes {#use}
- Use agent panes as you would local ones. Model, permission mode, effort and Fast mode travel to the host; a setting the host's CLI does not support is reported before the run.
- Permission, question and plan cards appear in this app, and your answers reach the run on the host. A request you send while it runs waits in the queue, as in a local pane, and starts when the run ends.
- The {{ui:graph.view.mighty}} views ({{ui:graph.view.diagram}}, {{ui:graph.view.timeline}}, {{ui:graph.view.structure}}) draw exactly as for a local run.
- A {{ui:remote.menu.newCommand}} pane runs each command in a new shell in the host's shared folder. Interactive programs and password prompts are not supported.
- A remote pane shows its host's OS (macOS or Windows) in the status bar and the session info. A {{ui:remote.menu.newCommand}} pane runs commands in the login shell on a Mac host and in PowerShell on a Windows host.
- Attached files are sent as their content: up to 8 at a time, 5 MiB each, 8 MiB in total.

These are not available in a remote workspace, because they read this computer's disk or network:
- [Mighty Orchestration](delegation.md) (child worktrees)
- The files pane
- [Styles](styles.md) that read project files, status lines, and resuming earlier sessions

pi, opencode and Hermes panes cannot be opened in a remote workspace yet. Remote workspaces do not appear on the [phone](phone.md).

## When the connection drops {#disconnect}
- If the network drops, a running remote pane ends in an error and the {{ui:settings.remote.connectionsTitle}} list says why. The host stops a run it no longer hears from within about 30 seconds.
- {{ui:settings.remote.disconnectButton}} stops that connection's runs and keeps the connection. Press {{ui:settings.remote.refreshButton}} or send from a remote pane to use it again. To remove the connection too, click the delete icon on its row.
- If a Tailscale host restarted sharing, you need its new key. Connecting to the same address with the new key replaces the saved connection's key.
- While the host computer sleeps or Mighty Code is closed there, no new work can be sent.

## SSH machines {#ssh}
Run terminals and agents in another computer's folders over SSH, with nothing installed there and no pairing. It suits dev servers, cloud VMs and machines with Linux-only tools.

**Before you start**
- This computer needs the OpenSSH client. Macs have it; on Windows, turn on the "OpenSSH Client" feature.
- Your OpenSSH signs in: `~/.ssh/config`, ssh-agent and your keys as they are. The app never stores passwords or keys. Make sure `ssh host` gets in without a password.
- For a machine you haven't used before, run `ssh host` once in a terminal to check and accept its host key. The app never accepts a host key for you.

**Adding a machine**
1. Go to {{ui:menu.settings}} → the {{ui:settings.remote.navTitle}} tab → the {{ui:ssh.settings.sectionTitle}} box.
2. Pick a Host from `~/.ssh/config` in the {{ui:ssh.settings.fromConfig}} menu, or type `user@host:port` and press {{ui:ssh.settings.addButton}}. Wildcard Hosts and `Match` blocks aren't listed.
3. {{ui:ssh.settings.test}} signs in without asking for a password and reads the OS, the login shell and whether tmux is there. It tells "can't reach", "sign-in refused" and "unknown host key" apart.
4. Type a remote folder (starting with `~` or `/`, such as `~/src/app` or `/srv/app`) and press {{ui:ssh.settings.open}}. The folder opens as a workspace with one terminal pane.

The machine shows in the sidebar under an "SSH · name" header, with its workspaces below. The header's … menu has {{ui:ssh.menu.test}}.

**What works**
- Terminal panes: the machine's login shell in that folder.
- Agent panes: Claude, Codex, Antigravity and pi run with the CLI installed on that machine and its account there. Approval and question cards show in this app. If the CLI isn't on the machine, the run ends with an error.
- The machine's login shell may be fish, csh or tcsh as well as bash or zsh.
- Reviewing a request's changes, hunk by hunk: what an agent pane's request changed on the machine is reviewed as on this computer. The machine's folder is snapshotted before and after each request (git is needed there; the repository itself is never written), so requests of Codex, Antigravity and pi work as well as Claude's. Putting back writes the machine's file in one step and never overwrites a file that changed meanwhile. A file the request made is moved into Mighty Code's keeping folder on the machine, not deleted.
- Attachments: dropped files go up to a temporary folder on the machine that only you can read, and are removed when the run ends.
- Files pane: open, edit and save the machine's files (up to 1 MiB each). A save replaces the file there in one step and keeps its encoding, line endings and permissions. If an agent or someone else changed the file after you opened it, nothing is overwritten: you're told and choose to reload or overwrite.
- On a Mac, the panes of one machine share one SSH connection. Windows' OpenSSH can't, so each pane connects on its own.
- Connect locally: when a pane's output names a dev server such as `localhost:3000`, the SSH bar over the pane offers to connect it. It forwards to a free port on this computer; click the address to open it in the browser. Closing the pane ends the forward.
- With [background work](settings.md#general) on, SSH panes keep working after the app closes.
- A worktree per pane: an agent pane can work in [its own worktree](delegation.md#pane-worktree). The worktree and its `mighty/<pane id>` branch are made in `~/.mightycode/worktrees/<pane id>` on the machine, and merge, commit and merge, undo merge, discard, keep on close and the worktree list follow the same rules as on this computer, with the machine's git. Whether work is left is always asked of the machine; if it can't be asked (the connection is down), the worktree is kept, never removed. Copied files, shared folders and the approved `.mighty.yaml` setup and teardown commands run on the machine too, and the port block is picked from ports free on the machine. Open localhost in the bar's menu forwards that port to this computer first. Viewing changes and accepting or discarding hunks work on the machine's files.
- Work map: the machine's git history (version tags, commits, `VERSION`, release state) is read over SSH. A version summary is made by Claude on this computer from the commit titles and release notes text only.

**When the connection drops**
- The terminal pane says "The SSH connection was lost." with a {{ui:ssh.terminal.reconnect}} button.
- If the machine has tmux, the {{ui:ssh.settings.tmux}} switch is on and terminal panes run inside a tmux session. The work keeps going on the machine, and reconnecting attaches to the same session. You can turn the switch off per machine.
- Agent runs keep going on the machine. This app reconnects after 2, 5, 15, 30, 60 and 120 seconds (then every 120 seconds) and reads on from where it stopped; a waiting approval card stays, and an answer you give while the connection is down reaches the CLI once it's back. If the connection doesn't come back within 30 minutes, the run ends here with an error and may keep running on the machine until it finishes.

**Not yet**
- Remote git changes, Mighty orchestration, compare runs, forking a pane, and opencode and Hermes panes.
- Showing a worktree in the file manager or opening it in an editor from the worktree bar (the folder is on the machine). The pre-merge check's pull request, CI and review threads (its git state works).

**Mighty Code server (remote Mac)**
When the machine is a Mac, Mighty Code can run there without a window, and you use that Mac's workspaces and agent panes here [as they are](#mirror). Only this machine's SSH is used: no Tailscale, no pairing link. Linux and Windows machines can't get one (there is no Linux build); use them the way above, with nothing installed.

1. Under the machine, in {{ui:ssh.server.title}}, press {{ui:ssh.server.detect}}: it reads the macOS version, the installed app (`~/Applications`, else `/Applications`) and its version, and whether the server runs.
2. {{ui:ssh.server.install}}: this app downloads the Mac package of its own version from the update information (the latest when its version isn't there; the confirmation says so), checks the update signature, installs it over SSH in `~/Applications/MightyCode.app` on that Mac and starts it without a window. That Mac checks the code signature again. No administrator rights (sudo) are used.
3. When it's installed it connects at once: "Remote · name" appears in the sidebar with that Mac's workspaces. Later, {{ui:ssh.server.connect}} or the sidebar's connect joins again.
- When the server's version differs from this app's, {{ui:ssh.server.update}} appears. It stops the running server, puts the new version in place and starts it again.
- The app starts only while the same user is logged in at that Mac's screen. For a Mac without a display, turn on automatic login there.
- When the SSH connection drops the server keeps running on that Mac, and this app reconnects a few times. If only the server on that Mac restarted while SSH stayed up, this app pairs again and reconnects by itself.
- {{ui:ssh.server.remove}} stops the server and deletes `~/Applications/MightyCode.app` (it asks first). Mighty Code's data on that Mac stays, and an app in `/Applications` is never touched.
