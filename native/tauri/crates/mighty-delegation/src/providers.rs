//! Which agent panes take part and how (C# Core/DelegationProviders.cs:1-236; M/DelegationProviders.swift): agents, modes, capabilities,
//! the widest child mode, the human's choices, `CanParentByAgent`, `AttachesDelegation`, `AgentName`, and the gate
//! (`DelegationIOHandler.Gate`) and the second send's choice (`ComposerSecondSends.Choose`, `MenuOnly`) over them. Reuses
//! mighty-composer's `queue::{is_agent_pane, child_can_write, ...}` and mighty-run's permission modes. Owner: W7-A (`ChildAutoCommit`,
//! :237-347, is W7-B's `git::auto_commit`).
