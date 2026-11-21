# Mighty Code — Rust + Tauri 2 host

The Rust rewrite of the desktop app, Windows first. It opens one window, draws the same React page the C# app draws
(`native/windows/web`), loads the saved state and answers every call the page makes. It saves the state byte for byte as the C# app
does, under the write policy (W9, `startup.rs`): a release build installed in its folder writes, any other run **only with
`--allow-state-write`**, and no run with `--read-only`; without writing every change stays in memory. The C# app
(`native/windows/MightyCode.*`) is untouched and ships until 0.5, which the C# 0.4.x updater installs over it (W9: the hand-off).

## Layout

| Path | What |
|---|---|
| `Cargo.toml` | The workspace: `crates/mighty-core`, `crates/mighty-composer`, `crates/mighty-delegation`, `crates/mighty-files`, `crates/mighty-graph`, `crates/mighty-history`, `crates/mighty-native`, `crates/mighty-tools`, `crates/mighty-accounts`, `crates/mighty-updates`, `crates/mighty-remote`, `crates/mighty-companion`, `crates/mighty-platform`, `crates/mighty-platform-windows`, `crates/mighty-platform-macos`, `crates/mighty-run`, `crates/mighty-terminal`, `crates/mighty-workmap`, `crates/mighty-styles`, `src-tauri`. |
| `crates/mighty-core` | No Tauri, no OS code. JSON exactly as C# `Wire.Json` reads and writes it (`json.rs`), .NET's times (`time.rs`), the state models (`model.rs`, `graph.rs`: C# `Models.cs`, `MightyGraph.cs`, `ExecutionGraph.cs` and the records they hold), `StateStore.Normalize` with every rule it calls (`normalize.rs`), loading, saving and the save timing (`store.rs`, C# `StateStore.cs`, `SaveSchedule`, `DesktopService`'s save queue), the locale catalogues and `LocaleKeyLeak` (`locale.rs`), the pane layout rules (`layout.rs`, C# `PaneLayout.cs`), model labels (`model_label.rs`), the frame model (`shell.rs`, C# `MainWindow.WebShell.Shell.cs`), the demo state (`demo.rs`), the add-pane menu and a pane's place (`add_pane.rs`, C# `AddPaneMenu`, `SessionTemplate`, `AddToLayout`), renames and pane titles (`rename.rs`, `RenameSupport`, `PaneTitle`), the workspace create rules (`workspace_create.rs`, `WorkspaceCreate`), the text setting (`text_appearance.rs`). |
| `crates/mighty-composer` | No Tauri, no OS-specific code: the composer's C# Core classes and what the WinUI host builds from them for the page (W2-B). `slash` (`SlashCommands.cs`, `SlashCommandStrings.cs`; the scan reads files through `SlashFiles`), `next_actions`, `ask_targets` (`AskTargets`), `custom_model` (the custom model dialog's data, `PaneView.CustomModel`; W6 draws it), `queue` (the second send, 바로 실행, `QueuedInputBuffer`, `RunBlock`, `BackgroundQueuePolicy`), `pending` (`PendingDisplay`, `SubmitResponse`, `SubmitReceipt`, `SubmitScroll`), `attachments` (`AttachmentSupport`; staging is the run engine's), `images` (`AgentImages.cs`: header sniffing with TIFF, the SHA-256 cache, the path rule, Markdown pictures, the RTF picture), `svg` (`SafeSvgDocument`, the SVG rules of `FilePreviewClassifier`), `html` (`LocalHtmlDocument`), `text` and `files` (the decoding and path rules they read). Then the page's models as pure functions: `composer::composer_state` (`WebComposerState`), `menus::composer_menu`/`menu_action`/`run_settings_info`/`run_settings_from`/`slash_catalog`, `cards::cards` (`WebCards`) with `presentation` (`ToolPermissionPresentation`, `UserQuestionnaire`, `PlanCardSupport`). What later stages give them is typed (`composer::WindowRules`, `cards::CardInputs`) and listed (`composer::PENDING`, `cards::PENDING`). Its tests port the C# checks, read the shared fixtures, check the page's field names against `src/composer/*.ts` and `src/cards/types.ts`, and compare 35,597 vectors with C# itself (`tools/parity` `composer`). |
| `crates/mighty-files` | No Tauri, no OS-specific code (the OS through mighty-platform's files ports): the files pane, its previews and the 변경사항 view (W4-B). `listing` (C# `WorkspaceFiles.List`, `Sorted`, `NaturalComparer`), `classify` (`FilePreviewClassifier`, `TextEncodings`; the decoding and SVG rules are mighty-composer's), `source` (`SourceLines`, `SourceHighlighter`, `SourceFind`), `tree` (`FilePaneTree`, `FilePaneDrawing`), `preview` (`FilePreviewLoader`), `diff` (`FileDiff`, Myers), `patch` (`FilePatch`, `FileEdit` and the parser's `EditedOriginal` reading of a `tool_use_result`), `changes` (`FileOriginalStore`, `GitHeadFile` through `ProcessHost`, `FileChanges`), `reference` (`ReferencePreview`), `pane` (`FilePaneKind`), `frames` (the HTML frames' folders and what they serve), `raster` (an SVG drawn to PNG with resvg, as C#'s `NativeSvgRaster`; a bitmap measured and fitted with the image crate, as C#'s WIC decode) and `model` (the page's `FilesModel` and `FilePreview`: `WebFilesModel`, `WebFilePreview`, `WebFileImage`). Its tests port the C# checks under their names and read `file-diff.json` and `source-highlight.json`. |
| `crates/mighty-core/examples` | `seed_demo` (a test profile from the help capture's demo), `roundtrip` (any state file loaded and saved in memory, timed; read-only), `parity` (the Rust side of `tools/parity`). |
| `tools/parity` | The C# side of the parity checks (`Parity.csproj`, a .NET 10 console over `MightyCode.Core` with `WebDelta.cs` compiled in; `Composer.cs` answers mighty-composer's vectors), `run.ps1` (the corpus and the vectors compared byte for byte), `real-write.ps1` (the app's save of a real profile's copy against C#'s), `run-replay` (C#'s `DesktopService` running a recorded Claude run through mighty-run's fake CLI, the expectation of `crates/mighty-run/tests/service.rs`), `delegation` (`Delegation.cs`: the delegation records' byte round trip and vectors) and `agent-io` (`AgentIo.cs`: the binding's argv), and `mcp.ps1` (the two MCP servers' answers to one JSON-RPC transcript each, C#'s and the Rust exe modes' stdout compared). |
| `crates/mighty-platform` | No OS code. The ports (traits) between the shared logic and the OS, each with its contract, a fake of each (`fake.rs`) and `Unsupported` (`unsupported.rs`), and `PlatformProfile`, the deliberate Windows/Mac differences as named values (docs/tauri-architecture.md §2.1). |
| `crates/mighty-platform-windows` | The Windows ports, everything behind `cfg(windows)`: `Paths`, `RegistryPrefs` (HKCU `Software\MightyCode`, reads), `DpapiVault` (C# `DpapiRemoteKeyVault`'s files, read and written both ways), and the process layer (W1-C): `WindowsProcessHost` (C# `ChildProcess`: suspended start, kill-on-close job, hidden UTF-8 console), `command` (C# `WindowsCommand`: PATH × PATHEXT, `.cmd` through `cmd.exe /d /s /c`), `CliRunner`, `discovery`, `WindowsShellEnvironment` (C# `CliEnvironment` + the PowerShell profile capture), `user_environment`, `WindowsPtyHost` (C# `PseudoTerminal`, ConPTY; its `busy` reads `shell_activity`) and `shell_activity`, and `WindowsClipboard` (the `Clipboard` port, W4-A), `setup_probe` (C# `SetupProber.Executables`, `SetupScript.WellKnownFolders`: where this PC has a command, for the "send to" switch). Its tests run real processes against a stand-in child (`tests/fake/child.rs`, a `[[bin]]`) and compare with C# itself (`tests/parity`, a tiny .NET program over MightyCode.Core). The rest is `Unsupported` for now. |
| `crates/mighty-run` | No Tauri, no OS code: the run engine (W2-A). `manager.rs` (C# `RunManager`: a pane's CLI started through `ProcessHost`, its frames read, its stdin kept for steers and answers), `providers.rs` (C# `ProviderCatalog`: every provider's arguments, Claude's and Codex's model reads, the runtime and discovery), `permissions.rs` and `presentation.rs` (C# `ToolPermissions`: `ProviderInput`, `ClaudePermissionChannel`, the card's copy), `codex_approval.rs` (C# `CodexApprovalChannel`: a Codex `onRequest` run's app-server turn, its approvals as the pane's tool-permission requests), `parser.rs`, `parser/pi.rs` and `stream.rs` (C# `OutputParser` for Claude, Codex, pi, Antigravity and the ACP agents, `ClaudeStream`), `pi_dialogs.rs` (C# `PiDialogChannel`: pi's extension dialogs as the question card), `plan_mode.rs` (C# `ClaudePlanMode`, the checklist and background-task trackers), `acp.rs` with `acp/agents.rs`, `acp/elicitation.rs`, `acp/accounts.rs` and the parser's `parser/acp.rs` (W5-B: C# `AcpChannel`, `OpencodeAgent`, `HermesAgent`, `HermesQuestionTool`, `AcpElicitationForm`, `OutputParser.Acp`, `OpencodeAccounts`, `HermesAccounts` with `CliAccountStatus`: opencode and Hermes runs over the Agent Client Protocol, their cards, their sign-in states), `mods.rs` (C# `ModBridge` + `HttpHost` over hyper), `service.rs` (the run half of C# `DesktopService`, its events through a channel, with `UpdateDraft` and the window's `ClearToolPermissions`; `service/antigravity.rs` its Antigravity cards, DesktopService.Antigravity.cs), `graph.rs` (`GraphFactory`/`GraphSink`, where mighty-graph's tracker plugs in, with `ReadChildren`'s sub-agent files: a Codex run's session files, an Antigravity run's sub-agents' transcripts), `antigravity/` (W5-C, AntigravityPermissions.cs: `permissions` — the denied actions, the card, the rules' safety, the denial collector, the board, what an answer does —, `settings` — `AntigravitySettingsText`, the byte-exact edits of agy's `permissions.allow` —, `leases` — `AntigravityRuleLeases` with its ledger, retries and look-again —, `records` — `AntigravityRecords`' helpers), `parser/antigravity.rs` (the Antigravity half of `OutputParser`), `kept_lists.rs` (C# `KeptModelLists`, `ModelListNotices`: the last list each CLI gave, kept in the catalog's storage folder, and the model menu's line), `model_selection.rs` (C# `ModelSelectionSupport`, `ModelCatalogRefresh`, `ClaudeModelReset`), `phase_models.rs`, `phase_routing.rs` and `phase_section.rs` (C# `PhaseModelPreferences`, `PhaseModelRouting`, `PhaseModelSection`: a run's phase models and Settings › 페이즈별 모델's rules and rows, the screen W6's), `model_settings.rs` (C# `ModelSettingsFileStore`, `OmcAgentCatalog`: omc's `config.jsonc` and Ouroboros's `config.yaml`, under a home folder always given), and the smaller C# classes each in its own file. Its tests port the C# checks under their names and run a stand-in CLI (`tests/fake/cli.rs`, a `[[bin]]` that replays recordings, plays Codex's app-server approval turn, a pi answering from `pi-rpc-*.jsonl`, C#'s fake agy and an ACP agent answering from a recorded session) through the Windows `ProcessHost`; `tests/service.rs` compares a replayed run's pane with C#'s; `tests/antigravity.rs` and `tests/antigravity_permissions.rs` are AntigravityVerification's and AntigravityPermissionVerification's run-engine checks (the fixture `antigravity-permissions.json`, the settings edits on temporary files). |
| `crates/mighty-graph` | No Tauri, no OS code: the execution graph (W3). `tracker/` (C# `ExecutionGraphTracker`: a run's stream as graph blocks and graph runs, notice runs, Codex and Antigravity sessions), `run.rs` (the tracker as mighty-run's `GraphFactory`/`GraphSink`, `TrackerFactory`, and the `SessionWatcher` a Codex run's sub-agents are read through, both behind the sink's `read_children`; mighty-graph reads mighty-run, never the other way round), `antigravity.rs` (C# `AntigravitySubagentWatcher` and `AntigravityChildRecord`: an Antigravity run's sub-agents read from their own transcripts), the rules every Mighty view reads (`support`, `notice`), the views' models (`layout`, `camera`, `flow/` role view, `timeline`, `structure/`, `view_model`) and the host's `mighty` push (`view.rs`: `MightyPane`, `Sources`, `project`, `MightySent`, `MightyPushes`). Its tests port the graph suites under their names and read the shared fixtures; the checks that need the run engine replay recordings through mighty-run's stream parser (`tests/view_support` `replay`), and the ACP runs of `tests/acp_graphs.rs` go through mighty-run's runner over its stand-in CLI, built here too as `mighty-graph-fake-cli`; what is still left is `DEFERRED.md`. |
| `crates/mighty-terminal` | No Tauri, no OS code: the terminals (W4-A). `pane` (C# MainWindow.Terminal.cs and the terminal half of MainWindow.WebShell.Panes.cs: a terminal pane's shell started by the page's `ready` through `PtyHost`, its output kept (1 Mi UTF-16 units) and flushed each 32 ms as base64 UTF-8, input, size, title, working directory, copy and paste through the `Clipboard` port with C#'s limits, the exit, the restart, the shell's activity, the footer; an `agent-terminal` pane shows its owner's commands), `agent` (C# `AgentTerminal`: an agent pane's commands, each in a terminal of its own, what the agent reads, readers with their own cursor, Ctrl+C then the stop), `prompt` (C# TerminalPrompt.cs: the detector, the answers, the monitor), `shell_blocks` (C# ShellBlockRuns.cs, ShellBlockScan.cs: the key, the record, the scan of a reply as the page's markdown reads it, the output), `activity` (`ShellActivity.NextStatus`, `SetShellStatus`'s change), `history` (`ShowTerminalHistory`'s content), `cleaner` (`TerminalTextCleaner`). Its tests port the C# checks under their names, drive panes and agent terminals over the fake terminal (a loopback), and run the AgentTerminal half of "Agent IO real ConPTY process ownership" over ConPTY. |
| `crates/mighty-history` | No Tauri, no OS code: session history and resume (W4-C). `history` (C# `SessionHistory`: where each CLI keeps a session's record, the next older chunk read back from its end), `backward` (`JsonlBackwardReader`), `scan` (`HistoryScan`: which line opens a request the user typed, the walk that places the retained request), `replay` (`HistoryReplay`: Claude's and Codex's records, and Antigravity's transcripts through `antigravity::stream_events` and the live parser, the sub-agents' own transcripts read up to the next request, fed to mighty-graph's tracker), `codex_sessions` (`CodexSessionWatcher`, `CodexSessionThread`: a running Codex run's sub-agents read from its session files for the live graph, through mighty-graph's `TrackerFactory`), `state` (`SessionHistoryState`), `resumable` (`ResumableSessions` for Claude, Codex, Antigravity, pi, opencode and Hermes, `KnownSessionIDs`), `antigravity`, `pi`, `acp` (opencode's `opencode.db`, Hermes's `state.db`), `sqlite` (the databases read-only through a built-in SQLite: `rusqlite` bundled). Its tests port SessionHistoryVerification, CodexSessionWatcherVerification, the resume parts of the Antigravity, pi, ACP-session, add-pane and model-label checks, and the two AgentNotice checks mighty-graph left; what is left is `DEFERRED.md`. |
| `crates/mighty-delegation` | No Tauri, no `cfg(target_os)`: delegation (W7, `.omc/research/tauri/w7-plan.md`; C# `Delegation*.cs`, `AgentIO/*.cs`). `request` (the records, the 28 reason codes, `DelegationHandler`), `model`, `json`/`store`/`copy` (`delegation-state.json` byte for byte), `pruning`, `providers`, `delivery`, `reports`, `coordinator` (the six tools' gates and caps, the FIFO turns; every mutating seam runs to its end on a task of its own), `tools`, `cards`, `text`, `sidebar` (W7-A); `git` (worktrees, the tool's and the card's merges, undo, cleanup, discard, the app's commit of a Codex or Antigravity child, REPORT.md read the Windows way; every command through `mighty_run::git`, W7-B); `mcp` (the two stdio servers), `agent_io` (the binding, the framings, the pipe's server and client, the web-open prompts), `fake_claude`, `modes` (the exe modes `--agent-io-mcp`, `--agent-delegation-mcp`, `--fake-claude`, W7-C); `app_host` (`DelegationPanes`, the run ledger, the ordered run-event pump) and `app` (the desktop service's delegation over `RunService`: the coordinator's host and the run service's hooks, and the window's entry points, W7-D). Its tests port the C# Delegation*/AgentIO checks by name, run real git in temporary repositories only (`tests/support/place.rs`), the scenarios S1–S3 headless over the real run path and pipe (`tests/support/launch.rs`, Windows), and compare with C# (`tools/parity` `delegation`, `agent-io`, `mcp.ps1`). |
| `crates/mighty-remote` | No Tauri, no `cfg(target_os)` (W8, `.omc/research/tauri/w8-plan.md`; C# `Remote\*.cs`): the phone, remote workspaces v2 and screen share's protocol. `relay` (C# `RelayCrypto`, `RelayLink`, `MobileRelayHost`: the X25519/HKDF/ChaCha20-Poly1305 frames, the link's policy and backoff, a connection's hello, auth and requests over tokio-tungstenite with native-tls through the system proxy's CONNECT tunnel, the 20 s/10 s keepalive), `identity` (`MobileIdentity`: the host id and token, paired devices, the pairing URL and its QR, push registrations; `mobile-identity.json` through mighty-platform's `pairing::identity_file`), `push` (`MobilePush`, `MobilePushHub`: what changed, who gets it, the sealed message, the HTTPS POST), `phone` (`MobileDesktopRouter` over the `PhoneDesk` trait: every `/m1/` route with C#'s statuses, codes and JsonOut bodies, the long polls, uploads, file previews, the app update's 409), `direct` (`MobileDirect*`: the Tailscale-only listener, admission and upgrade, its status and words) and `tailscale` (`TailscaleAddresses`), `remote` (`Remote*.cs`: the wire's strict JSON, the events both ways, validation, the strict HTTP/1.1 server and client transport, the sharing host, the client as the run service's `RemoteRunner`, `remote-connections.json`, the features a remote workspace lacks), `screen` (`ScreenShare*`: policy, wire, the clipboard's zstd frames through ruzstd, the hub over a `ScreenSharePlatform`, the reference scene; `ENABLED` is false and the app builds no hub). Its tests port the C# checks by name over an in-process fake relay (`tests/support/fake_relay.rs`), a headless desk over a real `RunService` (`tests/support/desk.rs`) and a fake sharing host (`tests/support/fake_host.rs`), all on loopback port 0, and compare with C# (`tools/parity` `push`, `pairing-url`, `routes`, `remote`). |
| `crates/mighty-companion` | No Tauri (W8-E; C# `Companion*.cs`, `CompanionOverlay.cs`): the companion pet. `prefs` (`companion-settings.json` byte for byte), `pet` (the catalog, manifests, the install and the atlas decode), `animation`, `carousel`, `card` and `bubble` (the bubble's buttons and question stepping), `plan` (the plan's Markdown blocks), `layout`, `follow` (the most pressing Claude pane, a paged one, a finished run's six seconds, the SHA-256 card key) and `Companion` (`lib.rs`: CompanionOverlay's state, its presses, carry, swipe, wheel, resize, double-click and DPI handling), drawn as a `PetScene` (`scene`, mighty-platform's display list that the Windows pet window draws with Direct2D and DirectWrite). Its tests port C#'s six companion checks by name, the overlay's measurements over the fake window and a golden scene of each row and card kind. |
| `crates/mighty-workmap` | No Tauri: the work map (W4-C, docs/work-map.md). `lib` (C# `WorkMap`: subjects to platform lanes, release tags, VERSION, release state, dot sizes, the git log by version, the user's requests attached to the commits that followed them; the model JSON keeps its nulls), `git` (re-exports `mighty_run::git`, C#'s `DelegationGit.Run` through the platform's `ProcessHost`, shared with delegation), `summary` (`WorkMapSummary`: the prompt, the answer's reading, one headless Claude run; `WorkMapSummaryStore`), `service` (`WorkMapService`: one summary at a time per workspace), `originals` (what it reads of the file originals), `dashboard` (the rest of `WorkDashboard`). Its tests read `work-map.json`, run real git and a scripted Claude (`mighty-workmap-fake`, C#'s `--workmap-fake`) through the Windows `ProcessHost`. |
| `crates/mighty-styles` | No Tauri, no `cfg(target_os)` (W9-D; C# `Core\Styles\*`, docs/mighty-styles.md): the style engine — `manifest`, `decoder` (the raw-key prescan as `Utf8JsonReader` meets it), `validator`, `registry`, `trust` (the approvals store with C#'s owner and permission checks), `evaluator`, `state` (the plan stage), `files` (workspace files read through the checked paths), `projection`, `presentation`, `prerequisites`, `permissions`, `watcher`; `bundled` holds the frozen style files by `include_bytes!`, which it reads in place and never writes (`scripts/check-style-freeze.sh`). Its tests read `styles/golden`, `styles/conformance` and `styles/fixtures`; `tools/parity/styles.ps1` compares its projection with C#'s. |
| `crates/mighty-platform-macos` | The Mac ports in Rust, everything behind `cfg(target_os = "macos")`: `Paths`, `cli_copies` (a command along the PATH), `process` (`MacProcessHost`), `pty`, `files` (file handles, PDF, Finder), `clipboard`, `secrets` (Keychain), `notify`, `fonts`, `pet`, `update`, `net`/`proxy`, `menu`, `input_method` and more (`src/lib.rs`); the terminal surface is native (Ghostty, `terminal_surface`). There is no in-app browser: web pages open in the default browser. |
| `src-tauri` | The app. `main.rs` (window, startup), `host.rs` (the bridge, the state's changes and saves, the run service's events), `delta.rs` (C# `WebBridge.Flush` and `WebDelta`), `methods.rs` (the page methods), `runs.rs` (the run engine: one `RunService` with mighty-graph's tracker and the picture cache, the CLIs' runtime), `models.rs` (the model lists kept right: C# MainWindow.ModelRefresh.cs), `ask.rs` (the phone's "send to" switch: C# MainWindow.AskTargets.cs), `panes.rs` (the never-shown pane views: composer and cards models and pushes), `composer.rs`, `cards.rs`, `mighty.rs` (their page methods; `mighty.rs` also the `mighty` push), `terminal.rs` (terminal panes, agent terminals, terminal prompts, shell block runs), `files.rs` (the files pane, the reference bubble, HTML frames and the picture viewer's actions, the file originals), `history.rs` (session history and the resume list's data), `antigravity.rs` (the Antigravity card's words and answer, agy's settings and the ledger for the run service), `workmap.rs` (the work map's methods, pushes and pane), `frame.rs` (the frame's remaining methods: close, focus, rename, remove, the add menu, panes, the folder picker, Explorer, the text size, the resume flow), `dialogs.rs` (the page-drawn dialogs), `workspace_create.rs` (the create sheets, git through `ProcessHost`), `platform.rs` (the one place the OS crate is chosen), `surface.rs` (`SurfaceManager`, the NativeSlot protocol), `mobile.rs` (the phone's relay host, identity, push hub and their 250 ms tick), `phone.rs` (the phone's desk under the m1 router), `direct.rs` (the Tailscale direct path and the shared firewall rule), `remote.rs` (remote workspaces: the client and the sharing host), `companion.rs` (the pet), `smoke.rs`, `smoke_frame.rs`, `smoke_terminal.rs`, `smoke_files.rs`, `smoke_history.rs`, `smoke_acp.rs`, `smoke_runs.rs`, `smoke_direct.rs`, `smoke_remote.rs`, `smoke_phone.rs` and `smoke_companion.rs` (`--smoke-test`), `args.rs`, `logfile.rs`. |
| `scripts/env.ps1` | Loads the MSVC environment and `~/.cargo/bin` into the PowerShell session (dot-source it before any cargo or tauri command). |
| `scripts/smoke.ps1` | Seeds a demo profile and runs the smoke; `-Profile <folder>` runs it read-only against an existing profile. |
| `scripts/screenshot.mjs` | A picture of the running page over WebView2's DevTools protocol. |

## Build and run

```powershell
. .\native\tauri\scripts\env.ps1          # once per PowerShell session
cd native\tauri
npm install                               # the Tauri CLI (@tauri-apps/cli)
npm run tauri dev                         # Vite dev server (native/windows/web, --mode tauri) + the app
npm run tauri build -- --debug            # target\debug\MightyCode.exe with the page built into it (no installer: bundle.active is false)
npm run tauri build                       # the same, optimized: target\release\MightyCode.exe
npm run tauri build -- --no-default-features  # slim production binary without `dev-harness` (smoke, help-capture, perf, fake CLI modes)
cargo test -p mighty-core
cargo clippy --workspace -- -D warnings
cargo clippy -p mightycode --no-default-features --all-targets -- -D warnings
```

`tauri build` first runs `npm run build -- --mode tauri` in `native/windows/web`, which writes `dist-tauri/` (never the `dist/`
the C# build ships). A plain `cargo build` makes a *dev* binary (Tauri's `custom-protocol` feature off) that loads the Vite dev
server at `http://localhost:5173` instead of the built page; build with the Tauri CLI to get one that runs on its own. By default
`mightycode` enables the `dev-harness` Cargo feature (`smoke_*`, `--help-capture`, `--perf-scenario`, `--verify-resources`, fake CLI
modes, and `tauri/unstable` for `get_webview`); pass `--no-default-features` to strip all test and smoke harnesses for production.

The VS 2022 Build Tools on the development PC are not registered with vswhere, so rustc cannot find `link.exe` unless
`scripts/env.ps1` has imported `vcvars64.bat`'s environment.

### Command line

| Switch | |
|---|---|
| `--profile <absolute folder>` | The state folder instead of `%APPDATA%\MightyCode` (C# `Program.cs`). An explicit profile never reads the pre-rename folders. |
| `--smoke-test`, `--smoke-exit` | The smoke (below); `--smoke-exit` quits with 0 (passed) or 1. Needs `--profile`. |
| `--smoke-out <file>` | The smoke's result (default `<profile>\smoke-result.json`). |
| `--log-dir <folder>` | The host's logs (default `<profile>\tauri-logs`). |
| `--remote-debugging-port <n>` | Opens WebView2's DevTools protocol (`scripts/screenshot.mjs`). |
| `--allow-state-write` | Lets the host write the profile's state as C# does: every change saved, the damaged-file recovery, the legacy copy. Off by default. |
| `--smoke-fake-cli <exe>` | The smoke's run steps against mighty-run's stand-in CLI (`mighty-fake-cli --replay-cli claude-stream-subagents` for Claude, `--fake-cli pi … --pi-fixture pi-rpc-question.jsonl --pi-dialogs` for pi, `--fake-cli gemini … --agy-denied` for agy), and its ACP step's opencode (`--fake-acp` replaying `opencode-acp-question`); every other provider reads as missing. Needs `--smoke-test`. |
| `--smoke-real-claude` | The smoke sends one real request with the installed `claude` CLI instead (below). Needs `--smoke-test`. |
| `--read-only` | W9: nothing is written, whatever the build (wins over `--allow-state-write` and the installed release's default). |
| `--help-capture`, `-language ko\|en\|zh\|ja` | W9-H: the help site's pictures (C#'s `--help-capture`), a smoke run on an empty profile; `-language` defaults to `ko`. |
| `--perf-scenario` | W9-H: C#'s timing scenario (`scripts/perf-scenario.ps1 -Engine tauri`); needs `--smoke-test` and an empty profile. |
| `-Embedding`, `----AppNotificationActivated:…` | W9-F: Action Center's activation of the toast's COM class; the run serves it. |
| `--update-helper …` | W9-B: the update helper in C#'s grammar (`--staged`, `--install`, `--package`, `--sha256`, `--size`, `--pid`, `--backup`, `--relaunch`), served before any profile, log or window. |

## The bridge

The page talks to its host only through `HostPort` (`native/windows/web/src/bridge/bridge.ts`). In Tauri, `hostPort()` returns
`tauriPort()` (`src/bridge/tauriPort.ts`), checked before WebView2's `chrome.webview` because Tauri's own IPC runs over that:

- page → host: `invoke('host_post', { message })`, one at a time and in order, all held until the `host-message` listener is
  registered, so no answer can arrive before the page listens. A post that cannot be delivered fails its call instead of leaving
  it waiting.
- host → page: every message — a call's result and every push — is a `host-message` event to the `main` window, emitted by one
  thread in the order the host posted them (C# `WebBridge` keeps the same single ordered channel).

`host.rs` keeps C#'s contract: a call `{kind:"call", id, method, args?}` is answered `{kind:"result", id, ok, value?, error?}`
with null `value`/`error` left out; calls are handled one at a time; pushes are dropped until the page has called `ready`, which
pushes the whole `snapshot` before its own result; a page that loads again starts over. A change reaches the page as C#'s
`WebBridge` sends it (`delta.rs`): a `delta` at most 16 ms after the call (coalesced), a 100 ms poll for changes made elsewhere,
each pane as `{id, session}` when new or `{id, set, unset, logs?, graphRuns?}` (only the fields whose JSON changed, the two long
lists by id), `removed`, `order` when the ids or their order changed, `state` when its JSON text changed. `shell` (the frame model)
is pushed when its JSON changed, after every flush and on a 250 ms clock (its "3 min ago" texts move on).

A method the host does not have answers `ok:false, error:"unimplemented: <method>"`, is logged once to `tauri-logs/host.log`, and
is added to `tauri-logs/unimplemented.json` when the app quits (every run's names together, with the implemented ones): the
porting checklist.

### Implemented methods

Each is the C# handler's rule (named beside it). A change is C#'s `UpdateAsync` (normalized, then queued for saving); an action is C#'s
`Act` (the banner cleared first, what it fails with shown there, the call answering null). The layout actions (`SelectLayoutSession`,
`ActivatePane`, `SelectWorkspace`, `ApplyLayoutPreset`) live once, in `frame.rs`, and the layout saves in mighty-core's `add_pane`; every
module uses those. The host answers 150 methods (`tauri-logs/unimplemented.json`'s `implemented` lists them).

- **W9** (`.omc/research/tauri/w9-plan.md`): the app update (`app_update.rs`, `settings/app_update.rs`, the phone's `/m1/app-update`; `postponeUpdate`, `restartForUpdate`: C# MainWindow.AppUpdate.cs over mighty-updates' `app_update`), styles (`styles.rs`, `settings/styles.rs`, the phone's style parts; `guidedAction`: C# MainWindow.Styles.cs over mighty-styles), per-pane worktrees (`pane_worktree.rs`; `paneWorktreeStatus`, `paneWorktreeAction`, `paneWorktreeDiff`, `workspaceWorktree`: C# MainWindow.PaneWorktree.cs), the remote mirror (`mirror.rs`; `serverConnect`, `serverDisconnect`: C# MainWindow.RemoteMirror.cs), the background output tails, custom agents' definitions and Ouroboros executions (`tails.rs`, `ouroboros.rs`), the help capture and perf scenario (`help_capture.rs`, `perf.rs`) and the toast activator (`notifications.rs`).
- **The phone** (W8-A/B/C: `mobile.rs`, `phone.rs`, `direct.rs`, `settings/mobile.rs`, `settings/mobile_direct.rs`; C# MainWindow.MobileRemote.cs, MainWindow.MobileDirect.cs, MobileRelayHost, MobileDesktopRouter): with `--allow-state-write` the relay link (the profile's `mobile-identity.json`, made at first use; the same host token and push key the C# app keeps, so a phone paired there reconnects without pairing again), pairing by QR, the paired devices with revoke, rotate; every `/m1/` route answered by the desk over the run service, delegation's send, the composer's queue, `ask.rs`'s targets, terminal prompts, Antigravity cards, the files and work map panes (a busy pane queues, an idle one starts, `NotStarted` is 409 `remote.error.dropped`, the S-L3 continuation revoke); push for a finished run, a waiting card, a lost sign-in and a shell that began, suppressed while that phone views the pane; the Tailscale direct path on this PC's Tailscale addresses only (exclusive binds, never a wildcard) with the shared firewall rule's row (add, update, remove, re-check through one UAC prompt). Without the flag nothing is loaded, written, dialled or bound and the box says verification mode (D2, D3).
- **Remote workspaces** (W8-D: `remote.rs`, `settings/remote.rs`; C# DesktopService.Remote.cs, MainWindow.RemoteWorkspaces.cs): `connectRemote` (and the add menu's connect) opens Settings › 원격 연결: sharing this PC (Start shares the picked workspaces on its Tailscale addresses, a new key each start, masked with show and a secret copy cleared after 60 s, full access, stop), the firewall rows (the phone's rule, both ports), the connect form, the connected computers with refresh, disconnect, remove and their workspaces to open. A remote workspace's pane runs on its host through the client (`RemoteRunner`), its composer held by `remote_block`; keys only in the DPAPI vault with `--allow-state-write`, in memory otherwise.
- **The companion** (W8-E: `companion.rs`, `settings/companion.rs`): `companionStatus` (the `companion` popover: the agents' cards and the pet switch), `companionToggle` (the pet on or off), the status bar's paw and capsule; the pet window itself (a NOACTIVATE layered window, the bubble's Allow once / Deny, the question's taps, the plan's Cancel and approvals through the run service; review, written answer and plan focus the pane's own card, D10) opens only with `--allow-state-write`; Settings' box (switches, picker with its preview, import, reload, reset) writes `companion-settings.json` only with the flag.
- **Screen share** (W8-F, `settings/screen.rs`): the paused box, as C#'s; the protocol is mighty-remote's `screen` (D11), the routes answer 503 while it is off.
- **Delegation** (`delegation.rs`, `agent_io.rs`; C# MainWindow.Delegation.cs, MainWindow.WebShell.Shell.cs:89-94, MainWindow.AgentIO.cs): `childAction` (merge with its merged-discard offer, undo, discard with its confirmation in a re-ask loop; ignored while the card is busy or a dialog is open; without `--allow-state-write` it says "verification mode"), `webOpenChoose` (an agent's open_url prompt answered; remember writes the state, gated as every state write is). The composer's `send{second:true}`, a busy pane's `send` with 실행 중 보내기 on an agent, `queueRunNow`'s worktree, agent-menu and held rows, and the worktree menu's agents (the composer state's `worktreeAgents`) go through delegation (W7).
- **The frame** (`methods.rs`; C# MainWindow.WebShell.cs, MainWindow.Layout.cs, MainWindow.Navigation.cs, MainWindow.Sidebar.cs):
  `getLocale` (the saved `languagePreference`; "system" reads the OS's preferred UI languages in order, C# `Locale.ChosenLanguage`),
  `ready`, `shellState`, `log` (to `tauri-logs/host.log`), `selectSession`, `activatePane`, `selectWorkspace`, `setTheme` (the open
  terminals take their colours again, C# `Retheme`), `setSidebarCollapsed`, `setSidebarWidth`, `toggleDisclosure`, `applyLayoutPreset`,
  `resizeSplit`, `resetSplit`, `dockSession`, `showDashboard`, `hideDashboard`, `dismissError`, `renameCheck` (`RenameSupport`), `openUrl`
  (http, https, mailto through `ShellExecuteW`, as C#'s `UseShellExecute`).
- **The frame's actions** (`frame.rs`; C# MainWindow.WebShell.Shell.cs, MainWindow.Navigation.cs, MainWindow.ResumeSession.cs,
  MainWindow.SmallParity.cs, MainWindow.TextAppearance.cs): `closeSession` (the run stopped and ended, then the pane gone), `focusPane`
  (`toggle` back to custom), `textSize` (Ctrl+= / Ctrl+- / Ctrl+0 over the saved setting, pushed as `text`, and every open terminal takes
  its size; saved per user only with `--allow-state-write`, never in a smoke), `dismissResource`, `renameSession` and `renameWorkspace` (the page's rename sheet; Automatic for an agent pane),
  `removeWorkspace` (the page's danger confirmation, every pane's run stopped), `showInExplorer` (the folder's real path, ShellExecute's `explore` verb),
  `pickFolder` (the OS's folder picker owned by the main window; the folder approved for saving and added, a Claude pane in a workspace
  without one), `addMenu` (the entries in the Mac's words, the remote menu, Connect), `addPaneEntry` (terminal, files — files.rs's
  pane through `frame::OPEN_FILES` —, an agent with the resume flow below, the three ways to add a workspace, another workspace's entry
  made active first), `newPane`.
- **The page-drawn dialogs** (`dialogs.rs`; C# `OpenWebDialog`, `CloseWebDialog`): `dialogAnswer`. A flow pushes `dialog{id, dialog}`
  and waits for the answer; the host closes one (`dialogClose`) or updates a resume choice's checking state (`dialogUpdate`). Every dialog
  still open is sent again, oldest first and as it stands, when the page calls `ready` (C# never sends one again after a reload, and its
  flow then waits for ever).
- **Resume, end to end** (`frame.rs`'s flow over `history.rs`'s `HistoryResume`, set as `frame::RESUME` at start; C# `WebAddAgentPane`,
  `WebPickResumedSession`, `WebResumeRows`): an agent pane from the add menu looks up the folder's newest session of that agent
  (`history::look_up`, head only, off the workers); with one no open pane continues, `resumeChoice` asks Start new / Resume… (checking
  while the look-up outlasts 300 ms); Resume… opens `resumeList` (`frame::open_resume` names its agent and folder, `history::open_resume_list`
  reads it afresh), whose rows are `resumeRows` (the page's search, the folder's sessions read again when "모든 세션 보기" changes;
  `hidden`, `empty`, `reason` as C# words them); the row picked is checked against the panes open now and remembered as the app's own
  (`history::close_resume_list`, `pick`, `remember_session`: `known-sessions.json`, written only with `--allow-state-write`), and the new
  pane continues it (`mighty_history::resumable::apply`), its older requests read back from the record into the diagram.
- **Workspace create** (`workspace_create.rs`; C# MainWindow.WorkspaceCreate.cs, `GitTool`, `GitCloneJob`; the rules are mighty-core's
  `workspace_create`, held to native/contracts/fixtures/workspace-create.json): `workspaceCreate`, `createCheck`, `pickLocation`,
  `makeFolder` (`git init` when asked), `repoName`, `gitProbe` (`git ls-remote -- <url> HEAD`, a newer one stops the older), `gitCreate`
  (`git clone --progress`, pushed as `gitProgress`; a second press waits for the running clone's answer, and a finished clone's answer is kept while its sheet is open, so a page loaded again mid-clone gets it on its next press) and `gitCreateCancel` (git and everything it started
  ended, what it wrote removed). git is the user's own, through `ProcessHost`, never prompting (an empty `GIT_ASKPASS`,
  `GCM_INTERACTIVE=never`, BatchMode ssh unless the user has an ssh command); the sheets start at the last location, else the active
  workspace's parent, else home.
- **Native views** (`surface.rs`; C# MainWindow.WebShell.Panes.cs): `surfaceBounds`, `surfaceAction`. There is no browser pane (0.5
  removed the in-app browser): every web page, a link in the page or an agent's `open_url`, opens in the user's default browser.
- **The composer** (`composer.rs`, C# MainWindow.WebShell.Composer.cs over the never-shown `PaneView`): `composerState`, `updateDraft`
  (`DesktopService.UpdateDraft`: the pane alone, saved on the schedule), `send` (`PaneView.Submit`: a busy pane queues the draft, an idle
  one takes the draft and its attachments, waits for the model list as the CLI has it now (`models.rs`) and starts the request through
  the run service, a start that did not happen puts them back; a "send to" choice wraps the words once, before they are queued or started,
  and leaves the files in the composer; answered with `SubmitReceipt.SendAnswer`: outcome, user entry, graph run, the page's client id,
  the composer state), `stop`
  (`StopActiveRun`: the queue cancelled unless the turn only waits on background work), `queueRemove`, `queueRunNow` (바로 실행: `Here`
  starts the row now, `Steer` hands it to the running turn; a worktree row's `agent` is the row's agent pick), `setBusySend` (실행 중
  보내기: null for the queue, or an agent of the worktree menu that can run now, whose new worktree a busy pane's Enter then runs the
  words in; kept on the pane view while the app runs), `installCli`, `pickAttachments` (the OS's picker through
  Tauri's dialog plugin), `addAttachments`, `removeAttachment`, `dismissAttachmentError`, `attachmentPreview`, `slashCommands` (the
  built-ins and the scanned catalogue), `slashAction` (new conversation, help, model, permission, and rename: the pane's rename sheet),
  `composerMenu`, `composerMenuChoose` (the row the page showed: provider, model, effort, permission, Fast, models read again, new
  conversation), `runSettings`, `applyRunSettings`, `toggleFast`, `composerToggle` (status line), `newConversation`.
- **The phone's "send to" switch** (`ask.rs`, C# MainWindow.AskTargets.cs; the desktop composer has none and never wraps its sends): a
  Claude agent pane of this computer offers the installed target CLIs
  (`AskTargetsFor`: codex, agy, gemini, grok, cursor-agent found along the CLI environment's PATH with PATHEXT and the installers' folders,
  read off the page's thread at most every 30 s, 2 s after a read that failed) once its slash catalogue (scanned when the phone opens the
  pane and by the slash palette) has `oh-my-claudecode:ask`. A phone send with a saved choice not yet known to stand waits up to 2 s for
  both. A saved choice no longer offered, judged only once both were read conclusively, falls back to Claude and is cleared.
- **The cards** (`cards.rs`, C# MainWindow.WebShell.Cards.cs): `cards`, `permissionAnswer` (Allow once / Deny for the request the card
  shows: Claude's, a Codex `onRequest` run's approval, a pi dialog's Cancel), `questionnaireAnswer` (`QuestionnaireAnswers`,
  `ValidateAnswers`; the refusal's words back; Claude's AskUserQuestion or a pi extension's dialog), `planAnswer` (`AnswerPlanCard`: one
  answer at a time, a revise Core would refuse never leaves the card), `nextActionFill`, `backgroundWork`, `statusLineTrust`, and
  `antigravityAnswer` (antigravity.rs, C# `WebAnswerAntigravity` → `AnswerAntigravityPermission`: 거절 closes the card; 이번만 허용 / 항상 허용
  first read agy's settings as an allow would (a file that cannot take the rule is the card's reason), then send the continuation on the
  pane's conversation with the card's rules — held in agy's settings while that run goes, or kept — or queue it behind a busy pane, its
  rules put in place when it starts; a start that fails brings the card back with the reason). The card itself (`WebAntigravityCard`:
  each action's kind, name, target, rule and once-only words, 항상 허용 off with its reason) comes with the other cards.
- **The Mighty views** (`mighty.rs`, C# MainWindow.WebShell.Mighty.cs): `mightyVisible`, `mightySetPresentation`, `mightySaveSize`,
  `mightyExpand`, `mightyResultFiles`, `mightyLoadOlder` (the next ten older requests of the session record, read off the page's thread
  and drawn above the retained ones: `history.rs`), `mightyImages` (a step's tiles as data URLs, older runs' too), `mightyTimelineFlip`,
  `mightyLogCollapsed`, `mightyOpenDashboard`, `imageData` (a log picture from the cache by its hash; an SVG drawn to PNG at most 2,048
  on its long side, as C#'s `WebImageUrl` draws it, and so are `mightyImages`' tiles).
- **The terminals** (`terminal.rs`, C# MainWindow.Terminal.cs, MainWindow.WebShell.Panes.cs, MainWindow.TerminalPrompts.cs,
  MainWindow.ShellBlockRuns.cs, over mighty-terminal): `terminalOpen` (the footer; from then on a `footer` push whenever it changes),
  `terminalMessage` (ready starts the shell — the platform's default shell in the workspace's folder at the page's grid, with the CLI
  environment —, input, resize, title, directory, copy, paste, pressed), `terminalRestart`, `terminalHistory` (the pane's saved entries in the
  native panel `terminalHistory`, dialog D14), `terminalPromptAnswer`, `terminalPromptShow`, `shellBlockRun` (only the command the reply shows, in
  the agent pane's terminal, its `agent-terminal` pane opened beside it, the record streamed at most every 200 ms), `shellBlockStop`.
  Pushes `{kind:'terminal', paneId, message}` (output each 32 ms, paste, reset, font, theme, footer); the font is the text setting's
  (12 pt at 100% to the nearest half point, the code font when one is chosen), given again to every open terminal when the theme or the
  text setting changes (C# `RefreshTerminalTheme`). A shell pane's status follows its shell through `RunService::update_pane` (C#
  `SetShellStatus`: idle once attached, running while a process of the shell's job sits on its console — read every 500 ms through
  `PtyControl::busy` —, completed, error); a terminal ends when its pane leaves the state (looked at every quarter second) and all of
  them when the app quits (each close on its own thread, the quit waiting at most 5 s). An agent pane's terminal (`Terminals::agent`) is
  followed for prompts every 400 ms while one of its commands runs, and a waiting one is the pane's terminal prompt card. In a smoke
  only the smoke's own shell pane starts a shell (C# `smokeTerminalEnabled`), without the user's profile.
- **The files pane, the reference bubble, HTML frames and the picture viewer** (`files.rs`, C# MainWindow.WebShell.Panes.cs's files
  part and MainWindow.WebShell.cs's conversation methods, over mighty-files): `openFiles` (C# `OpenFilePane`: the workspace's one files
  pane, left of the current pane; `open_slim_pane` places the work map's pane too), `filesState`, `filesToggle`, `filesReveal`,
  `filesFilter`, `filesSelect`, `filesRefresh` (each workspace's `FilePaneTree`, kept while the app runs; folders read off the page's
  thread), `filesPreview` (text cut to 256 KB with its colours as flat `[kind, start, length]` triples, pictures as data URLs: an SVG
  drawn by resvg, a PDF's first page by `Windows.Data.Pdf`, a bitmap as it is or fitted), `filesShowInExplorer`, `filesOpen`,
  `referencePreview`, `referenceChanges` (against the original the pane's run kept, else git's HEAD), `referenceOpen`, `referenceReveal`,
  `referenceSize`, `htmlFrame` (the file's folder served under `h<random>.html.mighty.invalid` through the `mightyhtml` protocol, at most
  eight folders, for the page's frame sandboxed to scripts) and `imageAction` (copy, save, reveal, open with the default app; a diagram
  tile of an older run too). A Claude run's Edit, Write and MultiEdit results keep the files' originals (`FileOriginalStore`: the
  profile's `file-originals` with `--allow-state-write`, else `%LOCALAPPDATA%\dev.mightycode.native\file-originals\<profile>`; the work map
  reads the same folder), pruned at the start to what the state names, a closed pane's dropped and a run's when its graph lets it go.
- **편집기에서 열기 and the review queue** (`editor.rs` over mighty-tools' `editor`, `review.rs`): `openInEditor{source: files |
  reference | worktree, paneId | id, path, line?}` resolves the path inside the files pane's workspace, the pane's run folder or the
  worktree (refusing anything else) and starts the editor Settings › 일반 › 에이전트 창's 외부 편집기 picks (자동, an editor found along
  the login shell's PATH or in its app bundle / install folder, or 시스템 기본) detached, its program and arguments as they are (named in
  the CI guard's detached-start list). `reviewQueue{refresh?}` answers the review queue (worktree panes and delegated children that
  finished with changes, each pane's newest finished request with captured changes), read again when a run ends (debounced), when the
  panel opens or refreshes and after its actions, and pushed as `review{queue}` when it changed; `reviewChanges{id}` (a child's changes
  sheet) and `reviewDismiss{key}`.
- **The work map** (`workmap.rs`, C# MainWindow.WorkMap.cs): `openWorkMap` (the workspace's `workmap:<id>` pane, placed as the files
  pane is, never saved), `workMapState` (the model, nulls kept), `workMapSummarize` (one at a time per workspace; the model is pushed
  again as `workmap` when it ends), `workMapOpenFile` (files.rs's `open_file_in_pane`: the file revealed, selected and previewed in the
  workspace's files pane, then `files{paneId, path}` pushed). The summaries are kept in `<profile>/work-map` with
  `--allow-state-write`, else in this machine's local folder for the profile.
- **The native forms** (W6: `native.rs` over mighty-native, C# MainWindow.WebShell.Settings.cs `WebNativeSurface`): `nativeState`,
  `nativeAction`, `nativeNav`, `nativeButton`, `nativeClose`. Each open surface (Settings, the setup screen, the plugin window, a dialog,
  a panel) is read into the page's NativeNode JSON by mighty-native's port of C#'s `WebNativeReader` and pushed as `native{id, place,
  view}` when it opens, after an input or a nav and on the frame's 250 ms clock, only when its text changed; `text{…}` carries the text
  setting. `--native-dump <file>` writes every push as sent (the parity capture below).
- **Settings and the screens it opens** (W6): `openSettings` (`settings/`: nine categories, seventeen boxes in the Mac's order, the
  saved `SettingsPane`), `openHelp` (the guide in the user's browser; a smoke logs it), `openSetup` (`setup.rs`: the setup screen, its
  probes, the install script in a terminal window of its own through `TerminalLauncher`, its sign-in row awaiting the CLI's own sign-in
  and saying its refusal), `openPlugins` (`plugins.rs`: the plugin window over mighty-updates), `sessionInfo` (`composer.rs`: the pane's
  numbers), `loginAction` (`accounts.rs`: the sign-in card and login recovery), `usageRefresh` and `usageDirect` (`usage.rs`: the status
  bar's usage chips, the popover and the dashboard's bars; Claude's direct lookup only to `api.anthropic.com`). The status line
  (`status_line.rs`), the completion toast (`notifications.rs`), the CLI and plugin updates and their holds (`updates.rs`) come with them.
  Anything that changes the machine (installs, sign-ins and sign-outs, updates, the toast's registration, the user's registry) runs only
  with `--allow-state-write` and never in a smoke; otherwise the row says "verification mode".

### The run engine in the app

One `RunService` (mighty-run) owns the state once the host has started it (`runs.rs`): `View::update` goes through
`RunService::update` (normalized under the service's lock, then saved), and each `ServiceEvent` takes the page's copy from the service
again. Its runner makes each request's graph with mighty-graph's tracker (`TrackerFactory`: the graph run published every second and at
the end, with the notice runs) and keeps tool pictures in the picture cache (the profile's `image-cache` with `--allow-state-write`, else
`%LOCALAPPDATA%\dev.mightycode.native\image-cache\<profile>`). Claude runs load the mighty-bridge plugin (`claude-mods` beside the exe, else
the repository's `mods/mighty-bridge`). A Codex pane in `onRequest` runs Codex's `app-server` through `CodexApprovalChannel`: each
approval is the pane's `permission` card, a turn not started in 15 s or a protocol failure ends the run in an error, a stop cancels the
waiting cards. A Codex run's sub-agents are read from its session files under `$CODEX_HOME` (else `~/.codex`) every second and at its end
(mighty-history's `CodexSessionWatcher`, read only). pi runs `pi --mode rpc` with the app's `ask_user` extension (`Agents` beside the exe,
else the repository's `native/macos/Sources/MightyCore/Resources/Agents`, C# `AgentFiles`); its dialogs are question cards, a session pi
has no record of (`~/.pi/agent/sessions`, read only) is started anew. Antigravity runs (W5-C) read agy's stream through the parser, leave a card for what headless agy
was refused (`antigravityPermission`, then the run service's board), and have their sub-agents' own transcripts read about once a second
(`~/.gemini/antigravity-cli/brain`, read-only). The rules an allowed card puts in agy's own settings
(`%USERPROFILE%\.gemini\antigravity-cli\settings.json`, `permissions.allow`, written only when the user presses 이번만 허용 or 항상 허용) are
recorded in the ledger `antigravity-temporary-rules.json` — the profile's with `--allow-state-write`, else
`%LOCALAPPDATA%\dev.mightycode.native\antigravity\<profile>` — and what a crash left in either place is taken out as the run service
starts, before any agy run may start (the other place's ledger pruned too, the profile's never written without the flag). With the flag
the ledger is the same file the C# app keeps in the profile; the local one is this port's alone and the C# app never reads it, so the
C# app's own leftovers are its own launch's to take out. A file rule names only a plain local path (`X:\…` or `X:/…` on a fixed drive,
no further `:`, no `~`; `/…` but not `//…` on the Mac), read as text before the disk is asked (a UNC name is never handed to the file
system), and never reaches agy's `.gemini` folder, the ledger's folder, the profile or the app's local folder — compared as written,
with its links read (one leading to a share refuses it) and as the file system names the longest part that exists (mighty-platform
`PathProbe`: `GetDriveTypeW`, `GetFinalPathNameByHandleW`). A card's verdicts are worked out once, off the parser's thread, and kept on
it; an allow is judged again before its rules go in. A command rule is kept for good only when its line is plain ASCII, and a URL
rule names only a plain host. A send to an Antigravity pane whose model agy's live list lacks starts
on agy's default (the pane's model set back with a note). The CLIs are read once at start (`ProviderCatalog::get_runtime`) and a pane's provider again when
its models are asked for; until then the composer says it is checking the runtime.

The model lists keep themselves right (`models.rs`, C# MainWindow.ModelRefresh.cs over mighty-run's `ModelCatalogRefresh`): selecting a
pane reads its list again, a provider switch reads the new agent's, the model menu's refresh row and a new conversation read it again at
once (C# `RefreshPaneModels`), and a send or a queued request waits up to 3 s for the list as the CLI has it now (the editor says it
checks the models; a queued request removed, stopped or closed meanwhile does not start); a newer list becomes the provider's runtime, and
an idle pane whose model or effort the list no longer has takes the default with a line in its transcript
(`ModelSelectionSupport.Adjusted`). At quit nothing is read or reconciled any more before the last save. The last list each CLI gave is
kept (`KeptModelLists`: `model-lists.json` in the profile with `--allow-state-write`, else in
`%LOCALAPPDATA%\dev.mightycode.native\provider-catalog\<profile>`), so a read that fails, now or after a restart, shows that list, and the
model menu says which list it is (`ModelListNotices.Notice`: the built-in one, or the kept one and its age). A smoke reads and reconciles
nothing, as C#'s.

opencode and Hermes run over the Agent Client Protocol (`opencode acp`, `hermes acp`; mighty-run `acp`, W5-B), with C#'s rules: the
pane's mode as OpenCode 2's own rules for the run (`OPENCODE_CONFIG_CONTENT`, with the shipped question instructions: `Agents` beside the
exe, else the repository's `native/macos/Sources/MightyCore/Resources/Agents`) or as Hermes's edit policy and `HERMES_YOLO_MODE`; OpenCode 1
refused; Hermes's clarify switched on once per launch through its own `config`; a Hermes session continued only where its `state.db` says
it may (read-only through mighty-history's `acp`, `runs.rs` `hermes_session`); the initialize given 45 s (Hermes 90 s), the server ended
2 s after the prompt's answer, a stop sending Hermes `session/cancel` a second before it ends the process. Their approvals are the pane's
permission cards (`permissionAnswer`), their `elicitation/create` forms and Hermes's clarify the question card (`questionnaireAnswer`; the
card's Cancel declines the form), and a lost sign-in ends the run with the `authentication` reason. The picker a Hermes session offered is
Hermes's model list from then on (`HermesAgent.CachedCatalog`: `hermes-models.json` in the catalogue's storage folder, the profile with
`--allow-state-write`, else `%LOCALAPPDATA%\dev.mightycode.native\provider-catalog\<profile>`), read again when a Hermes pane's run ends (C#
`RefreshModelsAfterHermesRun`). The two agents' sign-in states (`mighty_run::acp::accounts`: `opencode_status` over `opencode auth list`,
`hermes_probe` and `hermes_status`, as `CliAccountStatus`) are ready for the accounts section and the sign-in card (W6), which do not ask
them yet.

Pushes, with C#'s rules and cadence: `event` (each run event; a log line and a graph run without their record, which the next delta
carries), `permission` (each tool-permission change; a pane's waiting requests end with its run, as the window's
`ClearToolPermissions`), `composer` and `cards` (`panes.rs`: a pane whose record changed or that a method or a request marked, looked at
every 30 ms and sent when its JSON changed; the cards again each second while an open background strip has a task running), `mighty`
(every 120 ms for the panes the page shows, a pane projected when its record changed or a method marked it and every one each eighth
tick; only the blocks whose copy changed).

### Pending inputs

None since W9: the style row, the guide panel, guided questions, a style's prompt and plan mode (W9-D), a command typed into a new
shell pane (`RunTerminalCommandAsync`, W9-D), the Ouroboros dashboard, background output tails and custom agents' definitions (W9-E)
reach the composer, the cards and the Mighty views.

Settings › 페이즈별 모델's form (W6) checks every model it saves with `wire::model` (the composer's `ChangeModel` rule) before it
reaches mighty-run's `model_settings` writes, which take what they are given, as C#'s `ModelSettingsFileStore` does: a value with a line
break adds lines to Ouroboros's `config.yaml`. The writes keep C#'s other limits too, for W6 to settle: the comments in omc's
`config.jsonc` are dropped, a rewritten YAML line loses its CRLF, a symlinked config is replaced by a plain file, the file's permissions
are not kept, and the temporary file is not flushed before it replaces the config.

### Unimplemented page methods

None. What the page calls that the host would answer `unimplemented` (the page's `bridge.call`s against `implemented`) is empty since
W9's integration: smoke.rs's `ALLOWED_UNIMPLEMENTED` is `[]`, so a full smoke passes `unimplementedAllowed` only with
`unimplemented: []` (plan D20).

W9's C# baseline is `origin/main` 90bf8ace (0.4.61), merged into W9's base in c4cd19db: its page builds the C# `dist/` byte for byte
as `main` does; the methods it added (`paneWorktree*`, `workspaceWorktree`, `serverConnect`, `serverDisconnect`) are W9-I's and W9-J's.

W7's `webOpenChoose` was missing from this list before W7: a card method whose card never appeared is not called, so the smoke's list could not show it (W7 plan D11).

### W9: the update hand-off, the release and the write policy

What may change the machine (plan §4; "release" is a build stamped `MIGHTY_RELEASE=1` with the update key and address, as
`scripts/build-tauri-windows.ps1` makes it; its default applies only when it runs from `%LOCALAPPDATA%\Programs\MightyCode`, compared
canonically, so a candidate opened from Downloads beside C# 0.4.x writes nothing unless asked):

| Action | Dev, no flag | Dev `--allow-state-write` / installed release | `--read-only` | Smoke |
|---|---|---|---|---|
| Save the state and the profile's own files | no | yes | no | its own profile |
| `may_change_machine`: sign-ins, CLI and plugin updates, installs, HKCU, `~/.claude`, omc/Ouroboros configs, the firewall on click | no | yes | no | never |
| App update check, download, stage, the helper's start | never (no key) | the release only | no | never; `appUpdateSection` draws fixture phases |
| Startup housekeeping (restore, prune backups, `DisplayVersion`, renamed shortcuts), RenameMigration (default profile) | no | yes | no | never |
| Toast AUMID + CLSID registration | no | the installed release on the default profile registers class and exe; other writing runs the AUMID's name | no | never |
| Relay, push, listeners; the pet | no | per saved switches | no | fake relay, loopback port 0; its own overlay |
| WebView2 folders | local | release, default profile: C#'s, with C#'s options | as the run would | local |

A run that writes nothing (and is no smoke) logs under `<local>\logs\<profile>`, never creating `%APPDATA%\MightyCode\tauri-logs`
(a smoke keeps the profile's `tauri-logs`; `--log-dir` wins).

The app update (W9-A/B/C): the manifest is the 0.4.x envelope, Ed25519-verified (never `verify_strict`, as .NET's check), its
version compared as C#'s; the client refuses a signed manifest older than the staged update, or older than a kept update on its check
(stricter than C#); a package download stops after 60 s without data (C#'s `HttpClient` bounds the headers, not the body); the zip is
staged as C#'s stager does (one `MightyCode.exe`, no link, the PE machine checked) and installed by `MightyCode.exe --update-helper`
with C#'s grammar, mutex, `install.log` line and rollback, started detached outside the app's job with no process group (Ctrl+C keeps
working in the relaunched app's terminals). Accepted deviations: the release notes and the kept error are cut to C#'s 8 and 3 lines by
text line (a long wrapped line counts once; C# counts each wrapped line); `DisplayVersion` is the helper's three-part version (C#
writes the informational version for prereleases). The host's tests are in `src-tauri/src/app_update/tests.rs`.

Packaging and release (W9-G): `scripts/build-tauri-windows.ps1 -Architecture x64|arm64 [-Installer] -Version <v>` makes the stamped
release build (static C runtime, `RUSTFLAGS` refused), stages the exe with `claude-mods`, `Agents`, `pets`, `Assets\MightyCode.ico`
(the toast's icon), `LICENSE.txt` and `build-info.json`, refuses what the 0.4.x stager would refuse, checks the imports and writes the
zip with its `.sha256`, with `-Installer` the Inno Setup program, and the 0.4.x stager's verdict on the zip. `scripts/update-rehearsal.ps1` (a throwaway machine only,
`MIGHTY_REHEARSAL_DISPOSABLE=1`) installs a C# v0.4.x built with a throwaway key, serves the Rust zip and manifest on `localhost` and
checks the update end to end. The switch of the 0.4.x release workflow to the Tauri build at the 0.5 merge:
`git apply native/tauri/release/native-windows.yml.0.5.patch`.

The help capture and perf (W9-H): the smoke's help step (`help`: `helpShotsTaken`, `helpShotSize`, `helpManifest`; Windows only, its
pictures beside the smoke's result unless the run writes); `--help-capture` and `--perf-scenario` need an empty profile.

Left after W9:
- The manual checklist (plan §7) on a disposable Windows 11 x64 VM and an arm64 device, among it a real Action Center click
  relaunching the app with `-Embedding` on a writing build.
- Screen share's native half; Authenticode signing; a first-good-start rollback. (The Mac stage that was left here is in; see
  "Mac deviations".)
- RenameMigration's finishing of an interrupted migration (W9-F L6, beyond parity).
- A deviation seen by the smoke: a settings form is drawn again on the next 250 ms shell tick after a change it caused, so a second
  press on the theme picker within that tick lands on the segment still drawn as on and is ignored (the page's segmented control acts
  only on a segment drawn off); whether C#'s form answers the second press at once is not checked.
- The load flake `mighty-delegation` `the_merge_tool_takes_turns_and_follow_ups_stop_at_two` (a fixture `git rev-parse` exiting
  non-zero with empty stderr under ~35 parallel rustc) has no fix: 50 runs under load passed, the cause looks like the machine's.
- CI watch: `a_link_is_judged_before_its_target_is_opened` (the tails, W9-E) skips locally without the symlink privilege and asserts
  only on CI (`CI` set).
- A stray `%SystemDrive%\ProgramData\{Microsoft\Windows\Caches,NVIDIA Corporation\umdlogs}` folder appears under the working folder
  of some local runs since W8: a process that loads DirectWrite or the GPU driver started without `SystemDrive`; not found yet.

W8's deviations from C#:
- D10: the pet's review, written-answer and plan buttons focus the pane and its own card in the main window; C#'s separate approval and plan windows (`ShowCompanionApproval`, `ShowCompanionPlan`) are not ported (the page has no standalone card window).
- C-L5: without `--allow-state-write` the firewall rule is not read at all (not even the unelevated `netsh show`), so both firewall rows read unknown; C# reads it in any run.
- A-L7: a relay that answers 4409 (another host holds the token) is dialled again and in turn supersedes it, the two hosts trading the socket with no backoff, as C#'s do; kept for parity.
- Screen share's zstd frames are written and read by ruzstd (pure Rust, MIT, with twox-hash, MIT) instead of the plan's zstd, whose C build cannot cross-compile for the Mac check; each frame declares its content size (the phone's reader requires it) and a declared size must match what it decoded to, as ZstdSharp checks.
- The firewall rule needs an update when it names another program than this exe (compared without case), which C#'s `WindowsFirewallRule` does not check yet (a C# item).
- A send right after a run's end is admitted: the run manager no longer refuses a pane whose run's last status is out while its task still closes, but the new run starts only once that task has ended (at most 10 s), so two runs of one pane never overlap (C#'s `RunManager` keeps the run listed until its `finally` and refuses the send with `alreadyRunning` there).

Left after W8:
- Screen share's native half (D11): capture (`ScreenCapture`), input (`RemoteInput`), the WebRTC transport, the tap marker, the reference scene's window and the live settings box; `ENABLED` stays false and the smoke keys `screenCapture`, `screenTransport` and `screenTapMarkerNoFocus` wait with it.
- W9: L476/L477's style parts on the phone (no style is registered: every pane says `cli`, `guided` is refused, settings offer the CLI alone), the app update on the phone (`POST /m1/app-update` answers 409 `appUpdate.mobile.unavailable`), background output tails.
- The Mac stage: `FileHandles` (with `single_link`: `fstat` `st_nlink == 1` and `S_IFREG`), the firewall rows gated by `PlatformProfile::direct_connection_firewall`, the Unix socket transport of Agent IO, the pet's NSPanel, pairing's four files, `utun` pinning.
- W7-E/W8-G after `origin/main` (0.4.61) is merged into `feat/tauri`: per-pane worktrees and the remote mirror (`RemoteMirror`, the servers section, `MirrorStrip`), with the phone's pane-worktree create.

Left after W7:
- Per-pane worktrees (`PaneWorktree`, the page's worktree bar and dialogs) and the mirror's close are C# 0.4.60–0.4.61 on `origin/main`: W7-E, after `origin/main` is merged into `feat/tauri`; they reuse `mighty-delegation`'s git layer (`remove_own_worktree`, `place.rs`).
- RenameMigration's worktree half (moving the worktrees, `git worktree repair`) goes with the rest of the pre-UI migration: W9.
- `Host::may_touch_repo` compares paths as written (absolute, no `..`, under the profile in a smoke); delegation.rs's `may_touch` also allows the smoke's own folder. Comparing B's `canonical_place` paths would need the profile folder canonicalized too (an 8.3 `%TEMP%` such as CI's `RUNNER~1` would otherwise refuse the smoke's own fixture), a host.rs change deferred until a check needs it.
- A Codex run's `-c` lists escape a non-BMP character as two `\uXXXX` surrogates, as C#'s `Wire.Json` does: kept for parity with C#.

Left after W6:
- A click on a completion toast reaches the running app only (the toast's own handler). Windows' Action Center keeps a toast after the
  app quit; bringing the app back from there needs a COM activator registered for the app (C# review M3), a change to the machine
  deferred to W9 with the other installer work.
- Settings › 페이즈별 모델's add field takes its model with the 추가 button; Enter in the field adds it only once the page sends the key
  (C#'s field handles Enter itself; the page's native field does not report it yet).
- The styles box shows C#'s host-absent state until W9; screen share's is C#'s paused box (W8 made the mobile remote, remote
  workspaces and companion boxes live).

## Platform ports and native surfaces

The host reaches the OS only through the `mighty-platform` ports; `platform.rs` picks `mighty-platform-windows` or
`mighty-platform-macos` by target (the only place either is named) and hands the rest of the host a `Platform` and a
`SurfaceFactory` for the main window. The data folder and the pre-rename folders come from `Platform::paths`.

Native views over the page (docs/tauri-architecture.md §3): when the page calls `ready`, before the snapshot, the host pushes
`{kind:'host', platform, surfaces}` (Windows: the terminal `web`; the Mac: the terminal `native`, Ghostty). From then on the page
reports each native slot with `surfaceBounds{paneId, surface, rect (device px), visible, occluded}` and the `SurfaceManager` opens,
places, shows and hides the platform's view there; a page reload hides every view until it is reported again, and a pane that leaves
the state closes its view. There is no browser view: web pages open in the default browser.

The files ports (`mighty_platform::files`, chosen in `platform.rs` with the others; fakes in `mighty_platform::fake`): `FileHandles` (an
open file's handle asked where it is: `GetFileType`, `GetFinalPathNameByHandleW`; every reader of the user's files uses it, mighty-composer's
`read_under` included, so a folder swapped for a link between the walk and the open is caught as C# catches it on Windows), `PdfRaster` (a
PDF's first page through `Windows.Data.Pdf`), `Desktop` (show in Explorer through `SHOpenFolderAndSelectItems`, open with the default app
through `ShellExecuteExW` refusing a type `AssocIsDangerous` or `PATHEXT` names, as Windows' launcher refuses programs, and a picture on
the clipboard as `CF_DIBV5` and `PNG`) and `SaveDialog` (Tauri's dialog plugin, owned by the window). On the Mac they are implemented too
(`crates/mighty-platform-macos/src/files.rs`: `MacFileHandles` through `fcntl(F_GETPATH)`, `MacPdf` through `CGPDFDocument`,
`MacDesktop` for show in Finder, open with the default app and a picture on the pasteboard). Beside them `PathProbe` (`platform::path_probe`)
answers the Antigravity rule safety's questions about a path agy named, only once it passed the text check: a fixed drive
(`GetDriveTypeW`), a link read without following it, what the file system calls a path (`GetFinalPathNameByHandleW`; the Mac's is real:
`realpath`, no drive letters); its fake `RecordingPathProbe` records every question.

The C# host never sends `host`, and every page change for it sits behind `import.meta.env.MODE === 'tauri'`, so the C# page build
(`dist/`) is byte for byte what it was.

### Child processes

Every child process the app starts goes through the platform's `ProcessHost` (on Windows `WindowsProcessHost`: created
suspended, put in a kill-on-close job, only its three pipe ends inherited). A `std::process::Command` or `tokio::process::Command`
spawn passes `bInheritHandles = TRUE` with no handle list, so one that runs while `WindowsProcessHost` launches a child inherits
that child's pipe ends and keeps its stdin, stdout and stderr open after the child has closed them. `clippy.toml` therefore
disallows `std::process::Command::new` and `tokio::process::Command::new`, and `cargo clippy -- -D warnings` fails on either. Only
tests, the fakes and the tests' stand-in children (`mighty-fake-child`, `mighty-fake-cli`) start processes directly, each with an
`#[allow(clippy::disallowed_methods, reason = "…")]`; the app has no exception. A `SpawnRequest` names the variables the child
must not inherit (`env_remove`, `without`): `host.rs`'s `read_git` (the header's git status) takes the app's `GIT_*` variables out
of git's environment that way, as C# `WorkspaceGitInfo.Command` does (on the Mac through `MacProcessHost`, `crates/mighty-platform-macos/src/process.rs`).
`ProcessHost::cli_request` and `shell_request` say how an agent CLI and a shell pane's command start on each OS (Windows: a
`.cmd` shim and the command through `cmd.exe /d /s /c`).

The frame model (`shell.rs`) ports C# `WebShellModel` whole, field for field: the panes (status word, tone, agent mark, the
"· 20 days ago" / clock / context meta, the waiting words, the dashboard meta with the model's label), the counts, the sidebar with
the delegation tree (children under their parent, a closed parent's node), the header with the active workspace's git (read with
`git status --porcelain=v2` as C# reads it, off the UI thread) and the work-map and files buttons, the layout (saved tree, else the
mode's preset, kept stable between models as C#'s `layoutDefaults`; focus shows the active group), the dashboard, the banners and
the status bar. What a later stage feeds it is listed in `shell::Pending` (attention counts, delegation rows, runtime check,
usage, app update, setup, resource banner, the CLI's model catalogue).

## State

- Data folder: `%APPDATA%\MightyCode`, or `--profile`. A default profile without a state file takes the one in
  `%APPDATA%\MightyClaude` or `mighty-claude` (copied once with `--allow-state-write`, read where it is without).
- `workspace-state.json` version 1, read and written as C# does, byte for byte (`json.rs`, `model.rs`, `graph.rs`): property names
  matched without regard to case, an absent field taking the C# initializer's value (a new id, the time now), numbers from
  strings, the lenient converters' damage read as nothing, a wrong type anywhere else failing the load; written in C#'s property
  order, nulls left out, strings escaped as `JavaScriptEncoder.Default` escapes them (every non-ASCII character as `\uXXXX`,
  `+ < > & ' "` too), doubles in .NET's shortest form, times in `"O"`. C# keeps no field it does not model, so neither does this:
  a save drops unknown fields.
- The load is C#'s `LoadAsync` (`store::Store::load`): 8 MB, version 1 as an integer, `workspaces` and `sessions` arrays, the
  remote-reference filter of `KeptWorkspaces`, the once-per-profile `AntigravityMigration`, then the full `Normalize` as a restore.
  A file holding a NUL byte (what a power cut leaves) is moved aside to `workspace-state.json.unreadable-<time>-<id>` and its
  `.bak` loaded (or an empty state), with C#'s notice in the OS's language; any other unreadable file stops loading and every
  save, so it is never overwritten.
- Saving is C#'s `SaveAsync` and `AtomicWriteAsync`, as a split state (docs/superpowers/plans/2026-11-20-session-state-files.md):
  `workspace-state.json` is an index (`"sessionFiles": 1`) and each pane's body (`logs`, `graphRuns`, `planHistory`,
  `todoProgress`, `backgroundWork`, `shellRuns`) is `sessions/<pane id>.json`, each at most 8 MiB and owner-only on Unix. A
  save normalizes only the panes that changed (each with a text budget of its own), writes their bodies, then the index, then
  removes closed panes' bodies; the first save of an older one-file state keeps it as `workspace-state.json.before-session-files-
  <time>`. A damaged body is set aside with its pane alone restored from its `.bak` or opened empty. Each index entry
  keeps a summary of its body (`digest`: the log's length, its last entries cut short with the latest request, the diagrams'
  request ids). A launch reads the active workspace's bodies only; another pane holds its summary (`body_on_disk`) until its
  workspace opens, a run starts on it, it is changed, or the phone or a paired computer looks at it (`bodies.rs`,
  `RunService::ensure_bodies`). The store keeps its own record of those panes, so a save never writes what one holds in place of
  its file; something added to one before it is read is laid over its file by id. Every file is written to a
  temporary file flushed to the disk and swapped in with `ReplaceFileW` (the old file kept as `.bak`), a refused swap tried 40
  times 50 ms apart. A change the user makes is saved at once; a stream of changes waits
  0.22 s and at least 1.5 s after the last save began (`SaveSchedule`); quitting saves once more.

**Writing is off unless `--allow-state-write` is given.** Without it nothing writes the state file, its backup or anything else in
the profile except `tauri-logs/` and the smoke's result (both can be pointed elsewhere with `--log-dir` and `--smoke-out`).

### Parity with C#

`tools/parity/run.ps1` builds the C# tool and compares, byte for byte, what C# and this port save (and send the page) for a
corpus of state files — the synthetic edge cases and the repository's fixtures (`parity make-corpus`: unknown and wrong-typed
fields, NUL files with and without backups, an 8 MiB file, every enum value, non-ASCII, emoji and surrogates, timestamps with 7
and 9 fractional digits, missing and null fields, the delegation records, the remote references, the demo state), plus copies of
the real profile and of `release/perf` and `release/help-capture-final` — and 12,038 doubles, 490 strings and 223 times.
`cargo test` runs the same corpus and vectors when the .NET 10 SDK is there (`tests/parity.rs`; `MIGHTY_PARITY=require` makes a
missing SDK a failure), and `delta.rs`'s tests compare 600 random deltas with C#'s `WebDelta` byte for byte.
`tools/parity/real-write.ps1` runs the app on a copy of the real profile with writing on, changes the theme through the page and
quits, and compares the file with C#'s save of the same change.

The native forms (W6) cannot be rendered by the C# tool (C# builds them from live WinUI controls), so they are captured from the C#
app itself: `tools/parity/native.ps1` runs C#'s web smoke (`MIGHTY_SMOKE_WEB=1`, an empty temporary profile, WebView2's DevTools on
`-Port`) while `tools/parity/native-capture.mjs` (Node's built-in WebSocket) taps every `native` and `text` message its page gets into
`cs.jsonl`; C#'s smoke is stopped once its automatic-updates step reported, before its completion notification would register the
build with Windows and show a toast. The Tauri smoke then runs with `--native-dump rs.jsonl` (`smoke.ps1 -NoRuns -NativeDump`), and
`cargo run -p mighty-native --example native-diff -- cs.jsonl rs.jsonl --mask <profile>` compares each view's first push byte for byte
after mighty-native's `parity` normalization (keys sorted — WebView2 hands C#'s page sorted keys, so the order is the one thing it cannot
compare —, ids renumbered, the profile paths masked, font lists emptied) and prints the JSON paths that differ. HKCU Software\MightyCode,
the notification registrations and `%APPDATA%\MightyCode` (its logs aside) are compared before and after. The last run (dark, Korean):
the same as C# — Settings › 모델, CLI and 앱 정보, the setup screen over fixed probes, the terminal history panel and the model-id dialog;
differing by the two smokes' states — 일반 (the demo profile's light theme and status line), the plugin window (the active workspace in
its subtitle), 도구 (the toolkit's entries come from a read off the builder, so its first push has none); differing by stage — the
later-stage boxes (W7–W9) and the companion popover (W8). `--write-cs <folder>` writes C#'s views normalized:
`crates/mighty-native/tests/golden/` keeps those of machine-independent views, each compared in `cargo test` with the Rust side drawn
headlessly in the same state (mighty-accounts `tests/golden.rs`: the CLI tab's providers, accounts and Claude Mods boxes; mighty-tools
`tests/golden.rs`: the setup screen; the app's `terminal.rs` and `smoke_settings.rs`: the history panel and the model-id dialog).
Capture again whenever the C# side changes.

W9's parity: `run.ps1` ends with `tools/parity/styles.ps1` (every golden case of every style projected by C#'s `MightyCode.Core` and
by mighty-styles, 642 compared, and the conformance corpus; a relative `-Work` is taken from where it is called); mighty-updates' `tests/app_update_parity.rs`
and mighty-core's `tests/rename_migration.rs` compare `tools/parity/Update.cs`'s modes (`ed25519`, `manifest`, `version`, `plan-args`,
`mutex-name`, `stage`, `install-log`, `rewrite`) when the .NET 10 SDK is there.

W8's parity: `run.ps1` also runs C#'s `RemoteHttpListener` head parser over `native/contracts/fixtures/remote-heads` (pipelined
requests, bodies shorter and longer than their length, 8192- and 8193-byte heads, the sharing host's own body limit) against the answers
mighty-remote's `tests/remote_parity.rs` keeps (`remote`); `cargo test` compares, when the .NET 10 SDK is there, C#'s push messages and
their byte counts (`push`), the pairing URL (`pairing-url`) and C#'s m1 router over the nine delegation-record profiles with real window
extras, as JSON and byte for byte (`routes`, `tests/phone_parity.rs`).

What is not reproduced (each a file no app writes): a number literal beyond ±1.8e308 (C# reads infinity; the load fails here);
a lone surrogate escape inside a field C# does not model (C# skips it; the load fails here — inside a modelled field both fail);
an `-0.0` literal where C# wants an integer (read as 0 here; C# refuses it); a date C#'s culture parser reads but ISO 8601 does
not (`11/13/2026`; read as no date here); `"theme"` written twice with a differently cased key in between (the last of each
spelling wins here, the last of all in C#).

## Identity and WebView2

- `identifier` is `dev.mightycode.native` (W9-F): the toast registration's AppUserModelId (with the activator's CLSID
  `{E9E66915-2599-4971-9584-4825CAC5DEEE}`) and the root of this machine's local folders, `%LOCALAPPDATA%\dev.mightycode.native\…`.
- Product name "Mighty Code", main binary `MightyCode.exe` (the 0.4.x updater replaces that name), window 1440×920 with a
  940×650 minimum, as the C# window.
- WebView2 data: `%LOCALAPPDATA%\dev.mightycode.native\webview\<default | the profile path as a folder name>`; a release build that
  writes, on the default profile, takes C#'s own `<data>\web-profile` (`paths::web_folders`) and opens it with C#'s environment options
  (no browser arguments), making the page again for about 10 s while C#'s WebView2 still holds the folder
  (0x8007139F, right after the update swap).
- CSP: `tauri.conf.json` sets `app.security.csp` (`default-src 'self'; style-src 'self' 'unsafe-inline'; img-src 'self' data:;
  frame-src http://*.html.mighty.invalid mightyhtml:; object-src 'none'; base-uri 'none'; form-action 'none'; connect-src 'self' ipc:
  http://ipc.localhost`) alongside the page's build-time meta policy (`vite.config.ts`), with `devCsp: null` for Vite HMR in dev and
  `dangerousDisableAssetCspModification: ["style-src"]` so Tauri's asset CSP rewriter preserves `'unsafe-inline'` for runtime inline
  styles; the C# build's policy is unchanged. HTML previews' frames come from the `mightyhtml` protocol (`http://mightyhtml.h<random>.html.mighty.invalid/…`
  on Windows, wry's form of a custom scheme; `mightyhtml://h<random>…` on the Mac), so the Tauri build's `frame-src` is
  `http://*.html.mighty.invalid mightyhtml:` where C#'s is `https://*.html.mighty.invalid` (its WebView2 virtual hosts); a frame's own
  navigations are held to it as well.

## Smoke

```powershell
.\native\tauri\scripts\smoke.ps1                                   # a new demo profile in %TEMP%
.\native\tauri\scripts\smoke.ps1 -Profile "$env:APPDATA\MightyCode" # the real profile, read-only
```

`MightyCode.exe --smoke-test --smoke-exit --profile <folder>` waits until the page has called `getLocale` and `ready` and the host
has pushed the snapshot and the frame model, then runs a probe in the page: the sidebar's pane titles, the selected pane, the theme,
tabs, groups, header and status bar, then a click on another pane's row (the page must show it and the host's state must take it).
The frame steps follow (`smoke_frame.rs`, C#'s `RunWebShellFrameSmoke`, `checks.frame*`): the sidebar lists the active workspace's
panes with the header and the status bar, a row click selects a pane in the host and the page, the add-pane menu's terminal entry adds a
tab, a tab click and back switch panes (timed, `frame.tabSwitch*Ms`), and the columns preset draws two groups.
Where the page draws terminals (Windows), the terminal step follows (`smoke_terminal.rs`, the terminal part of C#'s
`RunWebPanesSmoke`): the host adds a shell pane in a workspace of its own (a temporary folder, in memory) and lets it start a shell
(without the profile); the probe waits for the shell idle at its prompt (`terminalStarted`), types through the page's xterm a
command that writes a marker in two halves and waits for the joined marker in the terminal's buffer (`terminalEcho`), reads the
footer (`terminalFooter`: the engine and the grid), types a command that runs a while (ping) and waits for the pane's status to
read running and then idle (`terminalStatus`), and closes the pane: its shell ends (`terminalClosed`). Details under `terminal`.
The files steps follow (`smoke_files.rs`, the C# smoke's `RunWebPanesSmoke` files part, `RunWebReferenceSmoke` and `RunWebPictureSmoke`, and
an HTML frame): the host adds a workspace of its own (a temporary folder with fixture files), a Claude pane whose 기록 view holds a reply
naming `src/a.ts:3` and one whose diagram holds a PNG and an SVG picture; the probe opens the workspace's files pane from its add menu
(`filesAddPane`: `addPaneEntry` through `frame::OPEN_FILES`), opens `smoke-folder` in the tree and previews `hello.ts` with line numbers and colour (`filesTree`, `filesFolderOpened`, `filesPreview`), opens
`page/index.html` as a page in the sandboxed frame, which the host must have served with its style sheet and script (`htmlFrame`), clicks
the reply's `src/a.ts:3` where it is drawn and the reference bubble shows the file at line 3 and closes (`referenceBubble`), and both
picture tiles come as PNG data URLs, the first opens the picture viewer with focus inside and Esc closes it (`pictureViewer`).
The history step follows (`smoke_history.rs`, the C# web smoke's work map steps and the resume flow end to end): the host makes a fixture
repository in a temporary folder (a tag and one commit past it), adds it as an active workspace of its own (in memory) and writes a fixture
home whose Claude records belong to it (the history reads that home only); the probe clicks the header's work map button once the folder
reads as a repository (`workMapButton`), waits for the overview's cells (`workMapOverview`) and one card per version with a summarize
button (`workMapVersions`), turns the Mac chip off and on (`workMapChips`); then it opens the workspace's add menu and its Claude entry, and
the folder's earlier session makes the frame's flow ask Start new / Resume… (`resumeChoice`); Resume… opens the list, which shows the
session with its first request and the nested run as hidden, says a search with no match, lists both with "모든 세션 보기"
(`resumeList`); a click on the session's row closes the list and adds a pane that continues it, and the page draws its record's three
requests above in the Mighty diagram, read back from the record (`resumeLoaded`).
With git on PATH the gitCreate step follows: a bare repository in a temporary folder (the fixture's own git), the sheets' location that
folder, then the add menu's "Create from Git…": the address typed, the check reachable, the name `remote`, 생성, `gitProgress` pushed,
and the clone listed as a workspace (`gitCreate*`). The reload step then opens a workspace's rename sheet from the host, loads the
page again, and checks the sheet is drawn again with its name and that its answer (Esc) ends the waiting flow (`dialogResentAfterReload`).
With `--smoke-fake-cli` the ACP step follows (`smoke_acp.rs`, W5-B; C#'s smoke has none): opencode is the stand-in replaying a recorded
session that asks one question (`--fake-acp`, opencode-acp-question); the host adds an opencode pane in a workspace of its own, the probe
sends a request from the composer, the agent's form is drawn as the page's question card with its three options (`acpQuestionCard`), the
probe picks Red and submits, and the answer reaches the agent as the form's content while the card goes (`acpAnswered`), the run completes
and the diagram draws the recording's answer (`acpDiagram`).
The delegation step follows (`smoke_delegation.rs`, C#'s `RunDelegationSmoke`; `checks.delegationPassed`, its error as `delegation.error`): in the smoke's own folder (its profile, or a folder of its own when the profile is read-only) a fixture repository `Delegation Smoke\repo` (`git init -b main`, a README committed as `Mighty Smoke <smoke@example.invalid>`), Claude the app's own exe `--fake-claude`, the switch in memory, worktrees under `worktrees` with unlimited free space; three children made the way delegate makes them and reported, C's notice pending, and delegation restarted from the file. Its keys are C#'s: `childrenListed`, `sidebarTree`, `heldRowPinned`, `windowCloseMergedChild`, `phoneCloseMergedChild`, `phoneSendReleasedHeldNotice`, `heldNoticeReceipt`, `secondSendNewWorktree`, `removeRefusedOpenChild`, `windowParentCloseAsks`, `phoneParentCloseKeepsChild`, `removeRefusedKeptWorktree`, `removedAfterDiscard`, `otherAgentChildStarted` (the `phone*` ones call what the phone router will call, W7 plan D8), and the Rust-only `delegateTool`: the fake Claude parent calls `mcp__mighty-delegation__delegate` through the real exe mode and the real pipe (the listener started for it; Agent IO is otherwise off in a smoke), and the child appears. Every git step stays in the smoke's own folder (`delegation::may_touch`).
The run steps follow (`smoke_runs.rs`, the C# smoke's `RunWebComposerSmoke`, `RunWebCardsSmoke`, `RunWebMightySmoke` and
`RunWebStepsSmoke` over a real child). With `--smoke-fake-cli` (what `smoke.ps1` passes unless `-NoRuns`): the host adds a Claude pane
in a workspace of its own (a temporary folder, in memory) shown in the Mighty diagram; the probe types a request into the composer and
presses Enter (`checks.composerSend`: the draft reached the state, the send started); the stand-in asks one Bash permission, allowed from
the page's card (`permissionCard`), and one plan, approved from the page's plan card (`planCard`: both answers reached the CLI); it then
replays claude-stream-subagents and the diagram draws the request, the sub-agents and the result with the recording's answer
(`mightyDiagram`); a second request is sent, and while its permission waits the progress panel draws a three-step plan on it (1/3: done,
current, pending) and none without it (`progressSteps`); the request is stopped from the composer (`stopRun`: the run ends stopped and its
card goes); then a pi pane in the same workspace sends a request to the stand-in pi, whose `ask_user` question is drawn as the question
card, its first choice is sent and reaches pi as the dialog's value, and the run completes with pi's reply (`piQuestionCard`). Then an Antigravity pane (the stand-in as C#'s fake agy over the smoke's own agy settings beside its logs, never the user's):
its request is refused `echo hi`, the page draws the Antigravity card with the command and its rule (`antigravityCard`), and its 이번만 허용
continues the conversation with the rule in those settings for that run — the stand-in answers "allowed" — and the settings are as they
were once it ended (`antigravityAllowOnce`). With `--smoke-real-claude` (`smoke.ps1 -RealClaude`) the probe sends "Reply with exactly the word: ok" to the installed
`claude` CLI instead (the pane's default permission mode, writes off) and checks the run completed, the result block shows "ok" and the
CLI reported its usage (`real*`).

The W6 steps follow the reload step (`smoke_settings.rs`, `smoke_setup.rs`, `smoke_accounts.rs`, `smoke_updates.rs`, from C#'s
`RunWebSettingsSmoke` and `RunWebLiveSmoke`): Settings opened from the sidebar, every category's boxes, the theme switched and back, the
terminal history panel and a model-id dialog; the setup screen over fixed probes, the components box, the toolkit and the status line
card; the sign-in card, the CLI accounts box and the usage chips over fixtures; the CLI update rows, the plugin window over a fake reader
and the update holds. A smoke reads no account, probes nothing of the PC and changes nothing on it. Last, as C#'s, `localeKeyLeakScan`:
every W6 box's words as the page would get them (each category's boxes and the setup list, at least 50) and none a raw locale key
(mighty-core's `key_leaks`, C#'s `LocaleKeyLeak.Detect`), reported as `{scanned, leaks}`.

The W8 steps follow the updates step. The remote step runs its parts in order, each reported under `remote`: the direct path
(`smoke_direct.rs`, `mobileDirect*`: `rows`, `status`, `firewallRow`, `addCommand`, `removeCommand`, `outcomes` over a recording netsh that
starts nothing, and `listener` on loopback port 0 through the fake network), remote workspaces (`smoke_remote.rs`, `remoteWorkspaces*`:
`settingsPage` with its firewall buttons off, `sharing` on loopback port 0 in this process, `connected`; nothing left behind) and the phone
(`smoke_phone.rs`, `phone*`: `info`, `state`, `created`, `session`, `files`, `submitted`, `entries`, `stopped`, `notified`, `closed`,
`unknownRoute`, through the m1 router in process with a smoke device; with `--smoke-fake-cli` only, so no real CLI starts). The companion
step follows (`smoke_companion.rs`, `companion*`, under `companion`: C#'s companion smoke over its own NOACTIVATE test overlay, the front
window compared around each step, the bubble measured as C# measures it, `statusPopover` opened from the page and closed with Esc). A smoke
never dials a relay, binds anything but loopback port 0, reads or changes the firewall, writes the pairing identity or opens the real pet.

It writes `smoke-result.json` (`passed`, `checks`, each step's report — `probe`, `frame`, `terminal`, `files`, `history`,
`gitCreate`, `reload`, `settings`, `setup`, `accounts`, `updates`, `localeKeyLeakScan`, `acp`, `delegation`, `runs`, `remote`, `companion`, and W9's
`styles`, `appUpdate`, `mirror`, `help`, `paneWorktree` —, the calls it saw, the unimplemented methods) and quits with
0 when it passed, else 1 (Tauri's event loop would end with 0 either way; `main.rs` keeps the code). The demo
profile is made by `cargo run -p mighty-core --example seed_demo -- <folder> [ko|en|zh|ja]` from
`native/contracts/fixtures/demo-profile.json`, as the C# help capture seeds it (without playing the runs). With `-Profile`, the
script compares `workspace-state.json`'s SHA-256 and time before and after and lists any file added to the profile; it also compares `%USERPROFILE%\.mightycode\worktrees`' listing, the profile's `delegation-state.json` hash and `git worktree list` of each local workspace the profile names (delegation must leave them alone), and W8's places: `companion-settings.json`, `remote-connections.json`, `screen-devices.json`, the `mobile-remote` and `remote-keys` folders and the Windows Firewall rule `Mighty Code (Tailscale)` as netsh reads it unelevated.

On the Mac (`macos-latest`, the stage's CI) the same smoke runs in WKWebView with Ghostty, the native panels and the pet. Its exit code is 0 when every check outside smoke.rs'
`MAC_PENDING_STEPS` passed (`passedOutsidePending`, `failedOutsidePending`; `passed` stays every check's verdict); the Mac's limit is
400 s with the stand-in CLI (645 s on Windows; the Mac's run took 120–150 s at the integration).

## CI

`.github/workflows/native-tauri.yml` runs on pushes to `feat/tauri` and on pull requests touching `native/tauri/**` or the page,
on `windows-latest` and `macos-latest` (arm64): the page's typecheck and unit tests, `cargo fmt --all --check`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `npm run tauri build -- --debug`, and on Windows
the smoke against the seeded demo profile (its result is kept as the `tauri-smoke-windows` artifact). W9-G added the source guards
(the `start_detached*` callers, no `verify_strict`, the frozen style files), the `package` job (x64 and arm64 release builds with a
throwaway key, `check-imports.ps1`, the smoke and `helper-e2e.ps1` on the packaged exe) and the `rehearsal` job (the update from
v0.4.59 and v0.4.61). It is independent of the 0.4.x workflows. The Mac build bundles `assets/icons/mightycode.png` and `MightyCode.icns`, which tauri-codegen needs there.

Mac stage M-I added:
- `package-macos`: `scripts/build-tauri-macos.sh` with a throwaway update key and the rehearsal's loopback address, then the same app
  under the hardened runtime with a throwaway self-signed identity, its MCP servers and the smoke. The
  candidate is the `MightyCode-native-macos-candidate` artifact.
- `rehearsal-macos`: `scripts/update-rehearsal-macos.sh` from v0.4.59 and v0.4.61, then the relaunch path and a Rust → Rust update.
- The guards' check that `native/tauri/release/native-macos.yml.0.5.patch` applies to `origin/main`.
- The notarization dry run (D20): `notarize-dry-run-build` (no secrets), then `notarize-dry-run` (environment `release`, SHA-pinned
  actions only). It runs only when the user dispatches the workflow **on `main`** with `notarize_dry_run`, which is after the 0.5
  merge, since the `release` environment allows `main` only. Do not open that environment to `feat/tauri`, which agents push to (or
  open it for one dispatch and close it again). The repository is public, so only the verdicts are kept (notarytool's status and log,
  `stapler validate`, `spctl --assess`), never the signed app or dmg. A push never cancels a dispatched run.

## Mac deviations

What the 0.5 Mac app (this host on macOS) does differently from Windows on purpose, and what it does not do that the 0.4.x Swift app
did. Mac stage M-J; the integrator folds this into the update notes (D12 is the user's sign-off).

### The profile (`mighty_platform::PlatformProfile`)

Every field is read where its behaviour is; the guards job of native-tauri.yml fails on a field nothing reads (its pending list is
empty since the Mac stage's integration).

| Field | Windows | Mac | Read at |
|---|---|---|---|
| `plugin_list_while_reading` | a list asked for during a read joins it (C#) | answered `busy` at once (Swift) | `PluginOperations::with_while_reading` (updates.rs) → the plugin readers' gate |
| `cli_updater` | winget, npm, native Claude, agy in `%LOCALAPPDATA%\agy\bin` | Homebrew (`<prefix>/bin/brew upgrade --cask/--formula <package>`, cleanup and dependents off), npm with the Node beside npm, native Claude, agy in `~/.local/bin` (Swift `CLIUpdateService`) | `CliUpdateService::with_updater` (updates.rs) |
| `settings_store` | the state file (and HKCU for the text and delegation switches) | UserDefaults `dev.mightycode.native` under Swift's keys and types: `language` (and `AppleLanguages`), `appUpdate.automatic/lastCheck/manifestURL`, `settingsPane`, `statusLine.enabled/trusted`, `usage.claudeKeychain`, `mighty.referenceBubble.width/height` — read at start, written back on save where the app may change the machine (Swift's `agentTerminal.webOpenChoices` and `browser.engineEnabled` are left as they are: never read, never written) | `settings_place.rs` (main.rs, the host's saver) |
| `agent_sign_in` | a console window (`TerminalLauncher`) | a shell pane of a local workspace with the login command typed and run (Swift `startCLILogin`) | `sign_in_pane.rs` (accounts.rs' coordinator) |
| `m1_platform` | `win32` | `darwin` | phone.rs, remote.rs |
| `update_package` | `MightyCode-windows.zip`, swapped by the staged exe's `--update-helper` | the manifest's `macos` package, unpacked by `ditto`, swapped by `install.sh` (Swift's `installScript`) | mighty-updates' coordinator and service |
| `update_needs_public_key` | a keyless build's box says the check is off (`noPublicKeyNoticeWindows`, the address hint with "leave blank") | Swift's words (`noPublicKeyNotice`, `manifestUrlHint`); the check itself refuses (`appUpdate.error.noPublicKey`) | mighty-updates' service, `app_update::words` |

Taken out of the profile (they described no difference): `pairing_layout` and `mcp_transport` (each the OS port's own: the pairing
store's files, the Agent IO listener's `transport()`), `auto_update_keys` (C#'s snapshot carries `autoUpdateCLIs`/`autoUpdatePlugins`
too) and `saves_files_pane` (neither app saves a files pane).

Also on the Mac: the Claude usage read takes the Claude CLI's sign-in from the Keychain first (`usage.claudeKeychain` on; never in a
smoke), then its file, on a blocking thread. Only a read the user asks for (turning the switch on, the popover's refresh) may show the
Keychain's dialog; an automatic read stays silent (bounded to 5 s: a locked keychain waits for its unlock) and, where the Keychain
would have asked, the card says `permission` ("Keychain access needed…", `usage.keychain.accessNeeded`) and is not read again for an
hour unless the user refreshes (Swift's `keychainPermission`). The number, date and time formats come from CoreFoundation's current
locale (`formats.rs`).

### Where the Mac differs from Swift on purpose, or cannot do what it did

- **Proxy credentials** (M-C): a system proxy that answers the `CONNECT` with 407 fails the request; URLSession would have answered
  with the proxy password in the Keychain. The Rust HTTPS (the usage read, the push, the app update) sends no proxy credentials: it
  fails closed. A system proxy setting that cannot be read is no proxy: the request goes straight, as URLSession does. TLS is 1.2 or
  later.
- **Reduced motion** (M-G): the help capture's `reduce_motion` does nothing on the Mac (WKWebView has no media emulation); the shots
  are taken as the page draws them.
- **Opening a file with its app** (M-G, stricter than Swift): the files pane opens a file only by what it is — never a Finder alias, a
  link, a package, an app, an executable, a script, a type that conforms to code (or an unknown type), and never when its default app
  lives outside `/Applications`, `/System` or `~/Applications`; the extension and launcher lists are a second layer.
- **Deep links** (M-F, a deliberate deviation): a `mightycode://` link's text is trimmed, its blank-line runs collapsed and control or
  bidi characters refused; when the pane's draft is not empty the app asks first, showing the link's text; every confirmation a link
  asks for has Cancel as its default. Swift wrote into the draft without asking. The confirmation's words are inline in deep_link.rs in the four
  languages, not in the locale files shared with C# (whose keys stay as they are).
- **Update signatures** (M-H, stricter than Swift, Sparkle's rule): a staged bundle must pass `SecStaticCodeCheckValidity` (strict, all
  architectures, nested code) against the running app's team when the app is signed with one, at staging and again just before
  `install.sh` is written; an ad-hoc or unsigned running app (development, CI) checks strict validity only. Swift checked none.
- **The login shell's environment** (M-A, a deliberate deviation): captured in the home folder, as on Windows, never in the workspace
  (Swift ran `zsh -ilc` in the workspace, which can run a repository's own hooks and leak one workspace's environment into another).
- **The pet** (M-F): clicks pass through its clear pixels by the window server's alpha test, not by toggling `ignoresMouseEvents` per
  hit as Swift did; checked by hand (plan §9).
- **The Keychain** (M-B; bounded at the integration review): a locked keychain in the search list makes a silent Claude read wait for
  its unlock with no limit inside the Security framework, holding the process-wide Keychain lock. The usage probe gives up on it after
  5 s, no other silent read starts while it is stuck (each answers "Keychain access needed" at once), a read the user asks for waits
  at most 5 s for the lock and 120 s in all, and a remote key's read waits 2 s for the lock, then reads as missing (the connection
  fails and says so).
- **Writing by default** (R6): a release Mac build in `/Applications` or `~/Applications` writes its state, settings and stores; anywhere
  else (a release candidate opened from `~/Downloads` beside 0.4.x) only with `--allow-state-write`.
- **Two copies at once** (stricter than Swift, which relied on LaunchServices activating a running copy): the Swift 0.4.x app and this
  one share the bundle identifier `dev.mightycode.native` and every store, so a run that would write stays read-only while another
  copy of that identifier runs (`NSRunningApplication`); it says so on stderr. A smoke never asks.
- **An HTML preview** (both OSes): its frame carries C#'s `LocalHtmlDocument.ContentPolicy` (no connection of its own, no plugins,
  base or forms) with the frame hosts allowed for its own frames, and the host's nonce is defined in the page's top frame on the app's
  origin only (wry runs initialization scripts in every frame on Windows; the smoke's files step checks the preview sees no nonce).

- **Quitting for a logout, restart or shutdown** (M-F, `kAEQuitReason`'s replacement): tao owns `applicationShouldTerminate:`, so
  Swift's `kAEQuitReason` is not read; `NSWorkspaceWillPowerOffNotification` marks the session's end instead (`session.rs`), and a
  staged update does not install on that quit. Open modal sessions and sheets are not ended before the quit, as Swift ended them; the
  16 s quit watchdog covers a quit they would hold.
- **No in-app browser**: Swift's browser pane (CEF) is gone; every web page, a link or an agent's `open_url`, opens in the default
  browser, and a saved browser pane is dropped on load. A change for the update notes.
- **`mightyclaude://` links** (D11): accepted beside `mightycode://` (Swift dropped the legacy scheme at run time).
- **The state's Mac-only fields**: none is lost. Every key Swift's `StateRepository` writes survives a Rust load and save (the
  `state-corpus` round trip, `crates/mighty-core/tests/state_mac.rs` with `parity-mac state-check`), and Windows' saves stay byte for
  byte.
- **Agent IO on the Mac** (M-C's decisions): one JSON message per line on the Unix socket (Swift's and the plan's framing); a
  follow-up to a busy child waits in C#'s `held` lane, where Swift used `pending`; a forged terminal token gets C#'s one refusal text.
- **An agent terminal's stop** (Swift `LongRunningProcessTests.swift:173`): the shared `AgentTerminal::stop` waits for the command
  after closing it without Swift's bound, as on Windows.
- **Phone client ids**: Swift accepted 16–64 characters, the shared rule (C#'s) 22; the phone app always makes 22, so a phone Swift
  paired is accepted.
- **Settings words**: the settings say the Mac's words where Swift did (`mighty-core` `settings_words`: ⌘ keys,
  "Mac notification", "this Mac"); Windows' note on keys and PowerShell is not shown, and the notification button opens
  System Settings › Notifications.

### Swift-only features the 0.5 Mac app does not have (D12)

Audited from the Swift report's (a′) list; the page or the Rust host already has `PaneHero` (the pane's header figures),
`ProviderBeta`, `PhaseModelMacReplaced` (the phase models) and the reset link; `TranscriptCopy` is the WebView's own selection and
Edit menu.

- **GitWorkbench**: no Commit & PR sheet (commit chosen files, push, see or open a GitHub PR / GitLab MR, its CI checks, "ask to fix").
- **CompareRun**: no compare runs (one request run by several agents in separate worktrees, compared side by side, one merged).
- **ReviewNotes**: no notes on diff lines sent back as a follow-up request.
- **TrustTools**: no secret scan before a commit, no "What this request did" list, no Help › Export diagnostics.
- **Completion webhook** (`notifications.webhookURL`): finished requests are not posted to Slack/Discord; a URL set in 0.4 is ignored.
- **Usage report and daily budget** (`usage.dailyBudgetUSD`): no usage report and no budget notification.
- **ResourceWarning**: the missing-resources banner is drawn by the page but never fed, and `--verify-resources` is M-H's.
- **StateBudget** (partial): saved transcript space is first come, first served across panes (C#'s rule), not Swift's fair share.
- **LocaleFill** (partial): placeholders are filled one name after another (C#'s and the page's rule), not in one pass, so a value
  that itself looks like `{name}` can be filled again.
- **ComposerCaret** (unchecked): Swift's caret fix after a Korean IME composition is not ported; whether WKWebView needs it is the
  §9 manual checklist's.
- **"Allow Keychain access and check"**: Swift's popover has this button on a `permission` card; here the popover's refresh button
  does the same (an interactive read), as the card's sentence says. The button itself would be a page change (D9, M-F's).
- **The sign-in pane**: opened as Swift opens it (Settings and the setup screen close, the active local workspace or the first local
  one, refused past 128 panes), but when no local workspace exists the setup screen's own sign-in terminal window (Swift
  `signInFromSetup`) is not there: the sign-in is refused with "Open a project folder first".
- **Dialog sizes** (`dialog.size.<id>.*`): the page keeps its own panel sizes; sizes saved by 0.4 are not carried over.
- **The login-shell fallback note**: when the login shell's environment cannot be read, Swift adds "the app's launch environment is
  used" to the CLI statuses and skips the Bedrock settings conflict check (CLIAccounts.swift:216-264, BedrockAuthDiagnosticsTests.swift
  :139); the shared Rust accounts code (C#'s) does neither, on both OSes.
