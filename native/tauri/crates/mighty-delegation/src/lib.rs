//! Mighty Code's delegation (W7, `.omc/research/tauri/w7-plan.md`): parent → child delegation between agent panes, Agent IO's MCP
//! servers and the task tree (C# MightyCode.Core Delegation*.cs and AgentIO/*.cs; the Mac's Delegation*.swift as the tie-breaker). No
//! Tauri and no `cfg(target_os)`: the OS is reached only through mighty-platform's ports (`ProcessHost`, `AgentIoListener`,
//! `AgentIoConnector`, `DiskSpace`), git only through mighty-run's runner.
//!
//! Each module names its C# source and its owner, one W7 part each (A: the delegation core; B: git and worktrees; C: the MCP servers,
//! the Agent IO transport and binding, the fake Claude; D: the desktop service's port). After Step 0 a module is changed only by its
//! owner (the plan's "Shared-file rules").
//!
//! | C# | here | owner |
//! |---|---|---|
//! | `Delegation.cs` (records, enums, reason codes, names) | [`request`] | Step 0 / A |
//! | `DelegationModel.cs` | [`model`] | A |
//! | `DelegationFileStore.cs` (`DelegationJson`, the file, the copies) | [`json`], [`store`], [`copy`] | A |
//! | `DelegationPruning.cs` | [`pruning`] | A |
//! | `DelegationProviders.cs:1-236` | [`providers`] | A |
//! | `DelegationDelivery.cs` | [`delivery`] | A |
//! | `DelegationReports.cs` (logic) | [`reports`] | A |
//! | `DelegationCoordinator.cs` (`IDelegationHost`) | [`coordinator`], [`host`] | A |
//! | `DelegationDelegateTool.cs`, `DelegationActionTools.cs`, `DelegationStatusTools.cs` | [`tools`] | A |
//! | `DelegationCards.cs`, `DelegationText.cs`, `DelegationSidebar.cs` | [`cards`], [`text`], [`sidebar`] | A |
//! | `DelegationWorktree.cs`, `DelegationMerge.cs`, `DelegationCleanup.cs`, `ChildAutoCommit`, `ChildReportFile` | [`git`] | B |
//! | `DelegationMcp.cs`, `AgentIO/AgentIOMcp.cs` | [`mcp`], [`modes`] | C |
//! | `AgentIO/AgentIOBinding.cs`, `AgentIOPipe.cs` | [`agent_io`] | C |
//! | `DelegationFakeClaude.cs` | [`fake_claude`] | C |
//! | `DelegationAppHost.cs` | [`app_host`] | D |
//! | `DesktopService.Delegation.cs` | [`app`] | D |
//! | W9: `WinUI\HelpDemoCli.cs` (the help capture's canned CLI) | [`help_demo_cli`] | W9-H |
//! | W9: `PaneWorktree.cs` (per-pane worktrees, `main` 0.4.60-0.4.61) | [`pane_worktree`] | W9-I |

// "Nothing below suspends until saved" (C#'s exclusive scheduler, W7 plan §1.9): a lock held across an await deadlocks against the 45 s
// answer race.
#![deny(clippy::await_holding_lock)]

pub mod agent_io;
pub mod app;
pub mod app_host;
pub mod cards;
pub mod compare;
pub mod coordinator;
pub mod copy;
pub mod delivery;
pub mod fake_claude;
pub mod git;
pub mod help_demo_cli;
pub mod host;
pub mod json;
pub mod machine;
pub mod mcp;
pub mod mighty_yaml;
pub mod model;
pub mod modes;
pub mod pane_worktree;
pub mod providers;
pub mod pruning;
pub mod reports;
pub mod request;
pub mod run_actions;
pub mod sidebar;
pub mod store;
pub mod text;
pub mod tools;
pub mod verify;
pub mod worktree_ports;
