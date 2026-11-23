# The agent sandbox (MC-34)

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
| Checks | `mighty-run` `sandbox::*` unit tests, `tests/sandbox_mac.rs` (real `sandbox-exec`), `mighty-composer` `page_models.rs`, `mighty-remote` `phone.rs`, `src-tauri` `sandbox::tests`, web `sandbox/sandbox.test.ts` |

## What is sandboxed

An **agent run** (`kind != "shell"`) of a **local** workspace whose sandbox is on, **on the Mac**. Not sandboxed:

- shell panes (the person's own commands) and terminal panes;
- SSH workspaces (MC-33): the CLI runs on the far machine; the switch refuses to turn on there and the box says so;
- remote (mirrored) workspaces: their runs are on their host;
- Windows: the switch shows "Windows에서는 아직 지원하지 않습니다" and cannot be saved on (see the research below);
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

## Windows (research only; nothing implemented)

The switch is shown off with "Windows에서는 아직 지원하지 않습니다" and `sandboxSave` refuses to turn it on.

| Option | What it gives | What it costs | Verdict |
|---|---|---|---|
| **AppContainer** (`CreateProcess` with `SECURITY_CAPABILITIES`, an AppContainer SID; LPAC for less) | Strongest native isolation: the process reaches only objects whose ACL names its SID (or `ALL APPLICATION PACKAGES`); network only with capabilities (`internetClient`…) | The workspace, the CLI's install and state folders, `node`, `git` and every tool must get ACEs for the SID (changing the person's ACLs); loopback to our proxy is blocked unless the container is exempted (`NetworkIsolationSetAppContainerConfig`, admin); many CLIs (node, Python, Git for Windows) misbehave inside; per-run setup and clean-up of ACEs | Possible later, as an opt-in with an explicit "grant this folder" step; risky to retrofit |
| **Restricted token + low integrity + Job Object** (`CreateRestrictedToken`, `SetTokenInformation(TokenIntegrityLevel, Low)`, `AssignProcessToJobObject`) | Writes only where the label is Low (no write-up); the job limits process count, memory, UI and kills the tree on close | The workspace must be relabelled Low (`icacls /setintegritylevel`), which every other low process could then write; reads are not limited (no way to hide `~/.ssh` without deny ACEs); no network control (WFP filters need admin); tools that write to `%APPDATA%` break | Write containment only; not the same promise as the Mac's |
| **Windows Sandbox** (`.wsb`, Hyper-V) | A throwaway VM with mapped folders; full isolation | Pro/Enterprise only, a feature to enable, tens of seconds to start, the CLIs and their sign-ins installed inside every time, one instance at a time | Not for per-run use |
| **WSL2 + bubblewrap** (Linux namespaces, seccomp; Anthropic's `sandbox-runtime` and Happy use it on Linux) | The Linux equivalent of this design: a read-only bind of `/`, writable workspace, `--unshare-net` and the proxy through a socket | Needs WSL2 and the CLIs installed in it; the Windows file system across `/mnt/c` is slow | The most promising path to parity: reuse the profile rules as bwrap arguments and the same proxy and gate |

Recommendation: no Windows sandbox until WSL2 + bubblewrap is prototyped; AppContainer only with an explicit folder grant and a
loopback exemption the person approves.

## Limits and known gaps

- `sandbox-exec` is deprecated by Apple (still shipped and used by Codex, Claude Code's own sandbox and others). If it goes, the
  runs degrade to unsandboxed with the warning; there is no replacement API for third-party apps.
- The keychain providers can read the login keychain's (encrypted) file and ask `securityd`; per-item access is macOS's.
- Processes an agent starts (sub-agents, MCP servers, build tools) inherit the sandbox. Those that honour `HTTPS_PROXY` go through
  the proxy (unlisted hosts raise cards); those that do not cannot connect at all.
- Plain-HTTP requests are forwarded as they came (absolute form); a server that refuses absolute-form requests answers 400.
- "이번만" allows the target for the rest of the run, not one connection (an HTTP client may open several).
- The reviewer's safe categories are tool-name based for Claude's tools; other providers' requests (Codex `commandExecution`, ACP
  `execute`) are always left to the person or denied by the reviewer.
