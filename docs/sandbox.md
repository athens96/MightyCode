# The agent sandbox (MC-34, Windows MC-42)

An opt-in, per-workspace OS sandbox for agent runs, a network allow list behind a local proxy, and an optional reviewer agent
that may settle safe requests. Everything is **off by default**. The user help is `docs/help/content/<lang>/approvals.md#sandbox`.

| Piece | Where |
|---|---|
| Settings (per workspace) | `mighty-core` `SandboxConfig` on `Workspace.sandbox` (`enabled`, `limitNetwork`, `allowedHosts`, `protectInstructions`, `readOnlyPaths`, `reviewer`, `reviewerAgent`, `reviewerModel`) |
| Profile generation | `crates/mighty-run/src/sandbox/profile.rs` |
| Allow list, provider hosts, safe hosts | `crates/mighty-run/src/sandbox/hosts.rs` |
| Proxy | `crates/mighty-run/src/sandbox/proxy.rs` |
| Network card (gate) | `crates/mighty-run/src/sandbox/gate.rs` |
| Reviewer rules | `crates/mighty-run/src/sandbox/reviewer.rs` |
| Wrapping a run, run lifetime | `crates/mighty-run/src/sandbox/mod.rs`, `manager.rs` (`Exec::sandbox`, `RunManager::review`) |
| App side (setup, refusals, reviewer run, methods) | `src-tauri/src/sandbox.rs`; `cards.rs` (`permissionAnswer` `always`); `phone.rs` (`sandboxBlocked`) |
| Card | `mighty-composer` `cards.rs` `network_card` (`allowAlways`) |
| Page | `native/windows/web/src/sandbox/` (settings box), pane badge in `pane/PaneView.tsx`, workspace menu in `shell/Sidebar.tsx` |
| Windows (MC-42): plan, access-list texts, approval record, guarantees | `crates/mighty-run/src/sandbox/windows.rs`; `sandbox/mod.rs` `prepare_windows` |
| Windows calls (labels, access lists, the token, job UI limits) | `crates/mighty-platform-windows/src/sandbox.rs`; `process.rs` (`CreateProcessAsUserW`); `agent_io.rs` (the agents' pipe) |
| Windows approval and undo (app) | `src-tauri/src/sandbox.rs` (`sandboxSave` `approve`, `sandboxUndoAll`, `windows_ready`), `platform.rs` `sandbox_acl` |
| Checks | `mighty-run` `sandbox::*` unit tests, `tests/sandbox_mac.rs` (real `sandbox-exec`), `mighty-composer` `page_models.rs`, `mighty-remote` `phone.rs`, `src-tauri` `sandbox::tests`, web `sandbox/sandbox.test.ts` |

## What is sandboxed

An **agent run** (`kind != "shell"`) of a **local** workspace whose sandbox is on, on the Mac (Seatbelt, below) or on Windows (MC-42,
"Windows" below: a different mechanism with weaker promises). Not sandboxed:

- shell panes (the person's own commands) and terminal panes;
- SSH workspaces (MC-33): the CLI runs on the far machine; the switch refuses to turn on there and the box says so;
- remote (mirrored) workspaces: their runs are on their host;
- the `/advisor` and reviewer runs themselves (they are read-only by their CLI flags, `advisor::read_only`).

A Mac without `/usr/bin/sandbox-exec` refuses a sandboxed run with `sandbox.warning.unavailable`: a run the person asked to be sandboxed never starts unsandboxed. A profile that
cannot be made (a folder path holding `"` or a control character, the run folder being `/` or the home folder) refuses the run
(`sandbox.error.prepare`): someone who turned the sandbox on never gets an unsandboxed run by surprise because of a bad path.

A sandboxed run is **not handed to the background daemon** (MC-3): its proxy and gate live in the app, so it ends with the app.

## How a run is wrapped

`Exec::sandbox` (after the launch is built, before `spawn_keyed`):

1. `SandboxHost::setup` (the app) gives the workspace's config, the home folder, the data folder and `<data>/sandbox`.
2. With the allow list on, a `NetworkGate` and a `Proxy` on `127.0.0.1:<free port>` start; the launch gets `HTTPS_PROXY`,
   `HTTP_PROXY`, `ALL_PROXY` (both cases) and `NO_PROXY=localhost,127.0.0.1,::1`.
3. The CLI's own folders are created if missing (outside the sandbox: inside it the CLI may write in them but not make their parents).
4. The profile is generated and written to `<data>/sandbox/<pane>-<id>.sb` (folder 0700, file 0600, `create_new`).
5. The launch becomes `/usr/bin/sandbox-exec -f <profile> <program> <args…>`, `SSH_AUTH_SOCK` removed, `MIGHTY_SANDBOX=1` set.
6. At the run's end (`RunEnd`), waiting network cards are withdrawn, the proxy stops and the profile is deleted. Profiles a crashed
   app left behind are deleted at the next start.

## The profile

`deny default`, then what any CLI needs to start (the Codex/Chromium base: `process-exec`, `process-fork`, signals and process info
inside the sandbox, `sysctl-read`, POSIX semaphores and shared memory, the PTY devices, `/dev/null` and friends, and a short
`mach-lookup` list: libinfo, opendirectory membership, logging, notifications, `dirhelper`, FSEvents, configd/DNS configuration,
`trustd` and `ocspd` for TLS). LaunchServices, Apple Events and the pasteboard are not looked up, so `open`/`osascript` cannot start
anything outside the sandbox. All paths are canonical (`/tmp` → `/private/tmp`): Seatbelt matches the kernel's path, and a
non-canonical rule silently matches nothing (found while building this: `touch` under `/tmp` was not denied until it was
`/private/tmp`).

### Reads

`(allow file-read*)`, then `(deny file-read* file-write* …)` for:

| Always closed (relative to home) |
|---|
| `.ssh`, `.aws`, `.gnupg`, `.config/gh`, `.config/gcloud`, `.azure`, `.kube`, `.docker/config.json`, `.netrc`, `.git-credentials`, `.password-store`, `Library/Cookies`, `Library/Safari`, `Library/Messages`, `Library/Mail`, `Library/Application Support/Google/Chrome`, `Library/Application Support/Firefox`, `Library/Application Support/com.apple.TCC` |
| The app's data folder (`~/Library/Application Support/MightyCode`: state, tokens, control and daemon sockets), except `<data>/agent-io` (the run's MCP config file and the agent IO socket), re-opened for reading |
| `Library/Keychains`, for the providers that do not sign in through the keychain |
| Every *other* provider's sign-in files (below) |

`~/.npmrc` stays readable (private registries need it); with the allow list on, a token in it can only reach listed hosts.

### Per provider

| Provider (CLI) | Own folders: read + write | Sign-in files closed to the others | Keychain |
|---|---|---|---|
| Claude (`claude`) | `~/.claude`, `~/.claude.json*` (written through a temp file beside it) | `~/.claude/.credentials.json` | yes (`Claude Code-credentials`) |
| Codex (`codex`) | `~/.codex`, `$CODEX_HOME` | `~/.codex/auth.json` | yes (`auto` store) |
| Antigravity (`agy`) | `~/.gemini` | `~/.gemini/oauth_creds.json`, `~/.gemini/google_accounts.json` | yes |
| OpenCode (`opencode`) | `~/.config/opencode`, `~/.local/share/opencode`, `~/.local/state/opencode`, `~/.cache/opencode` | `~/.local/share/opencode/auth.json`, `mcp-auth.json` | no |
| pi (`pi`) | `~/.pi` | `~/.pi/agent/auth.json` | no |
| Hermes (`hermes`) | `~/.hermes`, `$HERMES_HOME` | `~/.hermes/auth.json`, `~/.hermes/.env` | no |

Keychain access means `~/Library/Keychains` is readable and `com.apple.SecurityServer`, `com.apple.securityd.xpc` and
`com.apple.security.agent` may be looked up. Items of other apps still need macOS's own consent; the database file is encrypted.

Unix sockets: only the agent IO socket (`MIGHTY_AGENT_IO_SOCKET`) and sockets inside the provider's own folders. Not the control
socket, the daemon socket, ssh's ControlMaster (`/tmp/mc-<uid>-ssh`), ssh-agent or Docker.

### Writes

`(allow file-write*)` for the run folder (the workspace, or the pane's worktree), `TMPDIR`, `/private/tmp`, the per-user
`/private/var/folders/…` parent of `TMPDIR` (its `C` cache), the provider's own folders, and these caches (relative to home):
`Library/Caches`, `.cache`, `.npm`, `.bun/install/cache`, `.cargo/registry`, `.cargo/git`, `go/pkg/mod`, `.gradle/caches`,
`.m2/repository`. Nothing on `PATH` (`~/.cargo/bin`, `~/.local/bin`) and no shell start-up file is writable. The secret places stay
closed even where they would fall inside a writable folder.

### Git

Decided rule: committing, branching, merging, rebasing, cherry-picking, fetching and pane worktrees work; nothing that can make git
(or another tool) run a program later can be changed.

- `(deny file-write*)` the folder's `.git` (a folder, or a worktree's `.git` file, which could be re-pointed) and the repository's
  git folders (the one in the folder or the nearest folder above it; for a pane worktree its own admin folder
  `<common>/worktrees/<name>` and the main repository's `<common>`, read from `.git`'s `gitdir:` and `commondir`).
- `(allow file-write*)` again in each git folder: `objects/`, `refs/`, `logs/`, `worktrees/`, `lfs/`, `rebase-merge/`,
  `rebase-apply/`, `sequencer/`, `modules/`, every `[A-Z_]+` file and its `.lock` (`HEAD`, `ORIG_HEAD`, `FETCH_HEAD`, `MERGE_*`,
  `COMMIT_EDITMSG`, `AUTO_MERGE`…), `index`, `sharedindex.*`, `packed-refs`, `shallow`, `gc.pid`, `gc.log` (and their `.lock`).
- `(deny file-write*)` last: `config`, `config.worktree`, `commondir`, `gitdir` (also under `worktrees/*/` and `modules/**/`), and
  `hooks/` and `info/` anywhere there. `core.hooksPath`, `core.fsmonitor`, filters and aliases live in `config`; `info/attributes`
  names filters; hooks run on the next commit outside the sandbox.

Checked for real: `git commit -am` works under the profile; writes to `.git/hooks/pre-commit`, `.git/config` and
`.git/info/attributes` fail.

### Instruction files (switch on by default)

Read-only at the folder's root: `.mighty.yaml`, `.mighty.local.yaml`, `.mcp.json`, `.envrc`, `.claude/settings.json`,
`.claude/settings.local.json`, `.codex/config.toml`, `.gemini/settings.json`, `opencode.json`; in every folder, in any letter case
(APFS ignores case): `AGENTS.md`, `CLAUDE.md`, `CLAUDE.local.md`, `GEMINI.md`. These either steer the next agent or run code (MCP
servers, hooks, direnv). The person's `readOnlyPaths` (relative, no `..`) are added as `subpath`s. Deleting, renaming or replacing
them is refused too (all are `file-write*` operations).

### Network

With the allow list on: `network-outbound` only to `localhost:<proxy port>`, the Mods bridge's port
(`MIGHTY_CLAUDE_BRIDGE_URL`), the pane's own `PORT`/`MIGHTY_PORT`, and loopback entries of the allow list (`localhost:3000`); binding
and accepting on localhost are allowed (dev servers). No DNS socket: names are resolved by the proxy, which also removes DNS as a way
out. With the allow list off: any TCP.

## The proxy

An HTTP proxy on `127.0.0.1` (one per run). It takes `CONNECT host:port` (tunnels) and plain-HTTP requests in absolute form
(`GET http://host/…`, sent on as they came); anything else is `400`. Heads are capped at 16 KiB and 15 s; 256 connections at once.

- A target passes when an allow-list entry matches it exactly: `host` (port 443), `host:port`, `*.host` (names under it, not the name
  itself; never `*.com` or an IP), IP literals only as their own entries (`[::1]:8080`). Names are lowercased without a final dot.
- An IP literal for a listed name is another target (asked about), as is the listed name on another port.
- A listed *name* is resolved by the proxy and refused when any address it gives is loopback, link-local (the cloud metadata
  service), unspecified, multicast or broadcast (DNS rebinding). `localhost` is reached only when `localhost:<port>` is listed.
- Anything else goes to the gate: one card per target ("이 접속을 허용할까요? host:port", 이번만 / 항상 / 거부), answered once for the
  rest of the run. 항상 adds `host[:port]` to the workspace's list (the app, `permissionAnswer` with `always`). No answer in 5 minutes,
  or the run's end, is a refusal; more than 16 waiting cards are refused at once. A host that cannot show cards refuses at once.
- A refusal is `403` with `X-Mighty-Sandbox: blocked`, a `web` activity row in the pane ("샌드박스가 막았습니다: host:port — 이유", the
  progress panel), and a record kept per pane (20) for the settings box and the phone's `sandboxBlocked` field.

Provider API hosts listed for each agent (`hosts::provider_hosts`; from the CLIs' docs and binaries, **not proven by a paid run**):

| Provider | Hosts (443) |
|---|---|
| Claude | `api.anthropic.com`, `claude.ai`, `console.anthropic.com`, `platform.claude.com`, `statsig.anthropic.com` |
| Codex | `api.openai.com`, `chatgpt.com`, `auth.openai.com`, `ab.chatgpt.com` |
| Antigravity | `cloudcode-pa.googleapis.com`, `daily-cloudcode-pa.googleapis.com`, `oauth2.googleapis.com`, `accounts.google.com`, `generativelanguage.googleapis.com`, `antigravity.google.com` |
| OpenCode | `models.dev`, `opencode.ai`, `api.anthropic.com`, `api.openai.com`, `openrouter.ai` |
| pi | `api.anthropic.com`, `api.openai.com`, `chatgpt.com`, `generativelanguage.googleapis.com`, `openrouter.ai` |
| Hermes | `openrouter.ai`, `api.openai.com`, `api.anthropic.com`, `inference-api.nousresearch.com`, `portal.nousresearch.com` |

`*.googleapis.com` is deliberately not listed (it would include `storage.googleapis.com`, a way out to any bucket).

## The reviewer (optional)

When a sandboxed run's request card appears (any provider's permission card the run engine emits, and the network card), and the
workspace has the reviewer on, `RunManager::review` puts it to the reviewer **while the card stays up**:

- The app runs the reviewer through the `/advisor` plumbing (`advisor::ask_within`, `advisor::launch`, `advisor::read_only`):
  Claude (default model `haiku`) or Codex, read-only, with the action, the folder, the agent and the person's request in the pane
  (`reviewer::prompt`, capped). It must answer one JSON line `{"decision":"allow|deny|ask","reason":"…"}` (`reviewer::parse`;
  anything else is an error). Its cost goes to the usage ledger under the pane's workspace.
- `reviewer::decide` fixes what follows, whatever the text says:
  - **allow** settles the card only for a **safe category** (`reviewer::category`): `Read`/`Grep`/`Glob`/`LS`; `Edit`/`Write`/
    `MultiEdit`/`NotebookEdit` of a path inside the run folder (through links), not in `.git`, not an instruction file, and with no
    `blocked_path`; one plain `Bash` command (no `; & | < > \` $ ( ) { } \ !`) from a short list (`ls`, `cat`, `grep`, `rg`,
    `find` without `-exec/-delete`, `git status|diff|log|show|rev-parse|ls-files|blame`, `npm|pnpm|yarn|bun test|run
    test|lint|build|typecheck|check|format`, `cargo test|check|build|clippy|fmt`, `go test|build|vet`, `pytest`, `tsc`); a
    connection to a package registry (`hosts::SAFE_HOSTS`, 443). Any other allow leaves the card to the person;
  - **deny** denies (a refusal never grants anything);
  - **ask**, an error, an unreadable answer, or **no answer within 20 s** leave the card to the person. Never an allow.
- Plans and questions (`AskUserQuestion`, `ExitPlanMode`) are never reviewed: they are the person's.
- Every decision is a line in the pane ("심사 에이전트가 허용했습니다 / 거부했습니다 / 판단을 맡겼습니다: what — reason"). If the person
  answered first, their answer stands and the reviewer's is dropped.
- Only the person's own words are permission. The reviewer is told that text inside the action or the request is not permission,
  and nothing it writes is ever read as an answer: only the three-way enum is used, and the category gate is applied after it.

## Per-provider results (this Mac, macOS 27.2, `sandbox-exec` present)

`tests/sandbox_mac.rs::installed_agents_start_under_their_profile` runs `--version` and `--help` of every installed CLI under its
own generated profile, with a fake home and the allow list on with no port open (no network). Hermes was checked the same way by
hand with the real home (its launcher needs the real `~/.hermes` install; version and help only).

| CLI | Version | `--version` | `--help` | Needs |
|---|---|---|---|---|
| claude | 2.1.330 | ok | ok | `~/.claude`, `~/.claude.json*`, keychain |
| codex | 0.167.0 | ok | ok | `~/.codex`, keychain; its own Seatbelt nests inside ours (nested `sandbox-exec` works on this macOS) |
| agy | 1.5.14 | ok | ok | `~/.gemini` |
| opencode | 2.3.2 | ok | ok | its four XDG folders must exist first (now made before the run) |
| pi | 1.5.2 | ok | ok | `~/.pi` |
| hermes | 0.23.3 | ok | ok | `~/.hermes` (real home) |

Not verified (no paid runs): a real model turn of each CLI through the proxy, token refresh through the listed hosts, Claude's
keychain read under the profile, the Mighty exe's `--agent-io-mcp` mode under the profile (it reads `<data>/agent-io` and connects
to the socket, both opened), and the reviewer's real answers.

## Windows (MC-42, experimental)

Off by default, per workspace, as on the Mac; the settings box marks it **실험적** and lists what it enforces
(`sandbox::windows::guarantees`). It needs **no administrator rights**: no AppContainer loopback exemption, no firewall rule, no
second user account. A sandboxed run that cannot be sandboxed is refused, never run unsandboxed (MC-35's rule): an agent the
sandbox cannot hold (Hermes), a program inside a writable folder, a change outside the approval, a change that fails, a network or
non-NTFS folder, or a host that cannot make the token (`ProcessHost::spawn` answers `Invalid`; the daemon never gets such a run).

### The options and the decision

| Option | Writes | Secret reads | Network allow list | Admin | Verdict |
|---|---|---|---|---|---|
| **AppContainer** (with `internetClient`) | Only where the container SID is granted | Closed by default (the profile does not grant `ALL APPLICATION PACKAGES`) | **Cannot be enforced**: the container cannot reach our `127.0.0.1` proxy without a loopback exemption (`CheckNetIsolation LoopbackExempt` / `NetworkIsolationSetAppContainerConfig`, which needs elevation), so the proxy would have to be dropped and `internetClient` lets the run reach any host directly | Exemption: yes | Rejected: no proxy, no cards; every tool's install, `node`, Python, Git for Windows, `~/.gitconfig` need read grants ("the wrong shape" for open-ended tools, as OpenAI found for Codex) |
| AppContainer without network + a named-pipe proxy | As above | As above | A container may open a named pipe whose DACL names it, but no CLI speaks HTTP proxy over a pipe; an in-container TCP→pipe shim would need loopback inside the container, unverified | No | Rejected: unprovable without a Windows machine, and still needs the read grants |
| **Restricted token, write-restricted** (Codex's unelevated sandbox) + **low integrity** + **Job Object** | Two walls: the integrity check and the restricting SIDs | Closable with a medium **no-read-up** label on the secret folders (a label change on the person's folders, approved) | **Advisory**: a low-integrity process may use loopback, so the proxy and its cards work, but a program that ignores `HTTPS_PROXY` connects directly (WFP filters or firewall rules would need admin) | No | **Chosen** |
| Windows Sandbox (Hyper-V) | VM | VM | VM | Feature to enable; Pro only | Not per run |
| WSL2 + bubblewrap | Linux namespaces | Yes | Yes (`--unshare-net` + socket) | WSL to install | Later: the CLIs would have to live in WSL |

**The loopback question, answered:** an AppContainer cannot connect to `127.0.0.1` unless an administrator exempts it, so the Mac's
loopback proxy is unreachable from one; a low-integrity (non-AppContainer) process *can* reach loopback, so the chosen design keeps
the proxy, the gate and the 이번만/항상/거부 cards — but nothing forces a program through them. The UI says so
(`sandbox.settings.networkHelpWindows`, the guarantee list's 권고), and so does the help.

### How a run is started (`prepare_windows`)

1. `cli_refusal`: Hermes (its program, Python and state are one folder, `%LOCALAPPDATA%\hermes`: opening it would let a run rewrite
   the program its next, unsandboxed run starts) and unknown agents are refused (`sandbox.windows.hermes`, `unknownAgent`).
2. `windows::plan` for the run (below). A program (or the batch file cmd.exe runs) inside a writable folder of the plan is refused
   (`sandbox.windows.programWritable`).
3. `SandboxHost::windows_ready`: the app checks the plan against the workspace's approval in the record and makes the changes within
   it (a new pane worktree, a newly found instruction file, an approved agent's folder at that agent's first run); anything outside
   it refuses the run (`sandbox.windows.notApproved`), a failed change too (`changeFailed`).
4. The proxy and gate as on the Mac; the run's own `TEMP`/`TMP`/`TMPDIR` and the package caches (`npm_config_cache`,
   `YARN_CACHE_FOLDER`, `BUN_INSTALL_CACHE_DIR`, `PIP_CACHE_DIR`, `UV_CACHE_DIR`, `GOCACHE`, `GOMODCACHE`) in the workspace's private
   folder `%USERPROFILE%\AppData\LocalLow\MightyCode\sandbox\<16 hex>` (the person's `%TEMP%` is not writable at low integrity);
   `SSH_AUTH_SOCK` removed, `MIGHTY_SANDBOX=1`.
5. `SpawnRequest::restricted` = the restricting SIDs. The Windows process host (`process.rs`) then: `OpenProcessToken` →
   `CreateRestrictedToken(DISABLE_MAX_PRIVILEGE | WRITE_RESTRICTED, [workspace SID, agent SID, Everyone, logon session SID])` →
   `SetTokenInformation(TokenIntegrityLevel, Low)` → a default DACL naming the user, SYSTEM, the logon SID and the synthetic SIDs →
   `CreateProcessAsUserW` (a restricted copy of the caller's own primary token needs no privilege) into the kill-on-close job, which
   also gets UI limits (`HANDLES`, both clipboards, `SYSTEMPARAMETERS`, `DISPLAYSETTINGS`, `GLOBALATOMS`, `DESKTOP`, `EXITWINDOWS`).
   Such a request never goes to the background daemon (`RoutedProcesses::spawn_keyed` keeps it local); the run ends with the app.
6. At the end: the proxy stops, the run's temporary folder is removed.

The synthetic SIDs are `S-1-15-3-1024-<8 × u32 of SHA-256("MightyCode.Sandbox/<kind>/<value>")>` (the shape of a capability SID
derived from a name): one per workspace (`workspace_sid`), one per agent (`agent_sid`). No account has them; only the restricting list
and the entries made for them name them, so an undo removes exactly what was added.

### The plan (`windows::plan`) — what changes on which folder

| Kind | Paths | Change |
|---|---|---|
| Workspace (and a pane worktree of its repository) | the folder | low label `(ML;OICI;NW;;;LW)` + allow `(A;OICI;0x1301bf;;;<workspace SID>)` (modify without `WRITE_DAC`, `WRITE_OWNER`, `FILE_DELETE_CHILD`) |
| Git | each git folder inside the approved folders: `config`, `config.worktree`, `commondir`, `gitdir`, `hooks\`, `info\` (also under `worktrees\*`, `modules\*`; a missing `hooks`/`info` is made first), a worktree's `.git` file; the git folder itself | deny `(D;OICI;0xd0156;;;<workspace SID>)` (write, append, EA, attributes, delete, delete child, `WRITE_DAC`, `WRITE_OWNER`) + medium label `(ML;;NW;;;ME)` as a second wall; the git folder: deny delete only |
| Instructions (switch on) | the root's `INSTRUCTION_FILES` and every `AGENTS.md`/`CLAUDE.md`/`CLAUDE.local.md`/`GEMINI.md` found (any case; `.git` and `node_modules` skipped; 20,000 entries at most) **that exist** | as git |
| Read-only paths | the person's `readOnlyPaths` that exist | as git |
| Agent | `~/.claude` + `~/.claude.json`, `~/.codex` (+ `CODEX_HOME`), `~/.gemini`, opencode's four XDG folders, `~/.pi` | low label + allow for **that agent's** SID (one agent's run cannot write another's folders); a missing folder is made at that agent's first run |
| Secret | `.ssh`, `.aws`, `.gnupg`, `AppData\Roaming\gnupg`, `.config\gh`, `AppData\Roaming\GitHub CLI`, `.config\gcloud`, `AppData\Roaming\gcloud`, `.azure`, `.kube`, `.docker`, `.netrc`, `_netrc`, `.git-credentials`, `.password-store` (relative to home) **that exist** | medium label with no-read-up `(ML;OICI;NRNW;;;ME)`: the person's medium programs read them as before, a low process cannot |
| App data (the app's own) | `%APPDATA%\MightyCode` (or the profile's) | the same no-read-up label (its state, tokens, record) |
| Private (the app's own) | `…\LocalLow\MightyCode\sandbox\<key>` | allow for the workspace SID |

Refused folders: a drive root, the home folder or one holding it, one inside the data folder or a secret place, a network path
(`\\server\share`: labels are not kept), a volume without persistent ACLs (`labels_supported`: NTFS/ReFS only).

### Approval, record and undo

- `sandboxSave` with `enabled` on Windows computes the plan for every agent (`provider: None`). Unless the workspace's approval in the
  record already covers it, nothing is saved: the answer carries `needsApproval` and the box lists every change grouped by kind
  (`groupChanges`), with "the folder's other low-integrity programs can write it too" said in the approval text. **승인하고 켜기**
  sends `approve: true`: `Record::approve` makes each change (an agent folder that does not exist waits), keeps what each label
  replaced (`before`) and the folders it made, and keeps the approval (the ids of every listed change); a failure takes back what that
  approval made and saves nothing.
- The record is `<data>\sandbox\windows-changes.json` (inside the closed data folder: a sandboxed run cannot read or rewrite it), one
  writer at a time, written through a temporary file. Shared changes (agent folders, secret labels, the data folder) carry their
  users: they are made once and undone when the last workspace using them lets go.
- Turning the sandbox off (`sandboxSave` with `enabled: false`) undoes the workspace's changes, the last made first: a label is put
  back as it was (a low label on a folder with `TreeResetNamedSecurityInfoW`, so the low labels a low process gave the files it made
  go too), an entry is taken out by its type and SID only (Windows may write the rights in other words), folders it made are removed
  when empty. A change that cannot be undone stays recorded for the next try, and the box shows the failures.
- **모든 변경 되돌리기** (`sandboxUndoAll`, offered in the box "before you remove the app") undoes every recorded change and turns
  the sandbox off in every workspace. The Inno uninstaller does not run it (the app is not started at uninstall); the help says to
  press it first.

### What is enforced and what is not

| Promise | Mac | Windows | Why |
|---|---|---|---|
| Writes only in the run folder, the agent's folders, temp, caches | enforced | **enforced** | integrity check + write-restricted check, both needed |
| `.git/config`, hooks, info read-only; commits work | enforced | **enforced** | deny entries for the workspace SID; the grant has no `FILE_DELETE_CHILD`, so `config` cannot be replaced through its folder |
| Instruction files read-only | enforced | **partial** | only files that exist at the approval or a run's start; a new `AGENTS.md` (or `.mcp.json`) in a folder without one is not refused |
| Secret folders and app data unreadable | enforced | **partial** | the listed folders that exist (one made after the approval is closed at the next run's start); browser profiles are not closed (their own sandboxes run at low integrity and would break) |
| Other agents' folders | closed | **partial** | writes refused (per-agent SID); another agent's sign-in file stays readable (a label cannot tell one low process from another) |
| Network only through the allow list | enforced | **advisory** | the proxy and cards work for programs that honour `HTTPS_PROXY`; direct connections are not stopped |
| No input to other apps, no clipboard, no system settings | enforced | **enforced** | low integrity (UIPI) + the job's UI limits |
| Children stay inside | enforced | **enforced** | the token is inherited; the kill-on-close job allows no breakaway |

Also unlike the Mac: a low-labelled workspace can be written by **any** low-integrity process of the computer (not only ours) —
the "low-integrity sink" OpenAI named when it rejected integrity labels for Codex; we accept it, for the read-side protection and
UIPI it buys, and say so in the approval text. The agents' pipe (`mighty-agent-io-…`) now also admits the logon session's low,
write-restricted processes (a sandboxed run's MCP server), but only while some workspace's Windows sandbox is approved (the app
reads its change record at start and on every approval or undo); a machine that never approved it keeps the pipe owner-only. Its
requests still need the pane's token. A pipe instance already waiting when the first approval is made stays owner-only, so the
very first sandboxed run right after approving may start without the Mighty tools once.

### Per agent (expected; none checked on a real Windows machine yet)

| Agent | Writable | Expected | Known risks |
|---|---|---|---|
| Claude | `~/.claude`, `~/.claude.json` | runs | `~/.claude.json` is replaced through a temporary file beside it in the home folder, which is not writable: Claude must fall back to writing in place, else its settings are not saved; its Bash tool runs Git Bash (MSYS2) at low integrity |
| Codex | `~/.codex` (`CODEX_HOME`) | runs | its own Windows sandbox nested inside ours (it cannot grant ACLs: no `WRITE_DAC`); the keyring store, if used, may be refused |
| Antigravity (`agy`) | `~/.gemini` | runs | its Google sign-in if kept outside `~/.gemini` |
| OpenCode | `~/.config/opencode`, `~/.local/share/opencode`, `~/.local/state/opencode`, `~/.cache/opencode` | runs | Bun at low integrity |
| pi | `~/.pi` | runs | npm's `pi.cmd` through cmd.exe |
| Hermes | — | **refused** | program and state share `%LOCALAPPDATA%\hermes` |

For every agent: node, git (MinGW) and Python are expected to work at low integrity with the private `TEMP` and caches; npm/pip
caches go to the private folder (re-downloads); `git` over SSH cannot read `~/.ssh` (closed) and HTTPS credential helpers may be
refused; anything writing `%APPDATA%`, `%LOCALAPPDATA%` or `HKCU` outside its own folder fails. A workspace that is a folder inside a larger repository cannot commit sandboxed: that repository's `.git` lies outside the approved folder and is left unchanged (not writable). A CLI that cannot run sandboxed this
way shows its own error in the pane; it is never retried unsandboxed. What must be checked on Windows is listed in
[windows-v060-checklist.md](windows-v060-checklist.md) under MC-42.

## Limits and known gaps

- `sandbox-exec` is deprecated by Apple (still shipped and used by Codex, Claude Code's own sandbox and others). If it goes, a
  sandboxed workspace's runs are refused (`sandbox.warning.unavailable`); there is no replacement API for third-party apps.
- The keychain providers can read the login keychain's (encrypted) file and ask `securityd`; per-item access is macOS's.
- Processes an agent starts (sub-agents, MCP servers, build tools) inherit the sandbox. Those that honour `HTTPS_PROXY` go through
  the proxy (unlisted hosts raise cards); those that do not cannot connect at all.
- Plain-HTTP requests are forwarded as they came (absolute form); a server that refuses absolute-form requests answers 400.
- "이번만" allows the target for the rest of the run, not one connection (an HTTP client may open several).
- The reviewer's safe categories are tool-name based for Claude's tools; other providers' requests (Codex `commandExecution`, ACP
  `execute`) are always left to the person or denied by the reviewer.
