//! The six delegation tools (C# Core/DelegationCoordinator.cs:10-43 `DelegationToolManifest`, DelegationDelegateTool.cs,
//! DelegationActionTools.cs, DelegationStatusTools.cs): `delegate`, `list_children`, `child_status`, `merge`, `follow_up`, `discard`.
//! Exactly these six. Every argument is a string, and every one is required but delegate's `agent`. The words are the Mac's, byte for
//! byte: the agent reads them. Owner: W7-A.

pub mod actions;
pub mod delegate;
pub mod status;

/// `DelegationToolManifest.Argument`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Argument {
    pub name: &'static str,
    pub description: &'static str,
    pub required: bool,
}

/// `DelegationToolManifest.Tool`: its arguments are all strings, no others.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tool {
    pub name: &'static str,
    pub description: &'static str,
    pub arguments: &'static [Argument],
}

const CHILD: Argument = Argument { name: "child", description: "The child's id, as delegate or list_children gave it.", required: true };

pub const DELEGATE: Tool = Tool {
    name: "delegate",
    description: "Start a child pane that works on a task in its own git worktree, on the flat branch mighty/<child id> made from your current branch head. It runs your own agent CLI unless agent names another: claude, codex or antigravity, each only if installed and signed in on this PC. Use it only when the user asks for parallel or delegated work, or when the task clearly splits into independent parts that can run at the same time; never fan out speculatively, and do the work yourself otherwise. The child starts in the permission mode you ask for, which must be one of the child agent's modes and one in which it can change files. For your own agent it must be no wider than your own mode (Claude: plan < manual < acceptEdits < auto < fullAccess; Codex: manual < acceptEdits < onRequest < fullAccess). For another agent, the child's mode may do nothing yours may not: edit files without asking only if yours does (Claude acceptEdits and auto, Codex onRequest and acceptEdits, Antigravity acceptEdits, any fullAccess), edit after asking only if yours edits at all, run shell commands without asking only if yours does (Claude auto, Codex manual and acceptEdits, any fullAccess), and have full access only from your fullAccess. So from Claude acceptEdits a Codex child gets onRequest, not acceptEdits (which runs commands without asking), and from Claude plan or manual no Codex or Antigravity child can start. Answers as soon as the child exists; its run goes on in the background. The child writes its result to REPORT.md, and you get one notice per report. At most 3 open children per pane, of all agents together, and a child cannot delegate. The same call twice in one of your runs returns the same child.",
    arguments: &[
        Argument { name: "task", description: "What the child should do. It is saved as the child's TASK.md.", required: true },
        Argument {
            name: "mode",
            description: "The child's starting permission mode, one of the child agent's in which it can change files, no wider than yours: plan, manual, acceptEdits, auto or fullAccess for a Claude child; acceptEdits, onRequest or fullAccess for a Codex child; acceptEdits or fullAccess for an Antigravity child.",
            required: true,
        },
        Argument {
            name: "agent",
            description: "Optional: the agent CLI the child runs, claude, codex or antigravity. Leave it out for your own agent.",
            required: false,
        },
    ],
};

pub const LIST_CHILDREN: Tool = Tool {
    name: "list_children",
    description: "List your children with their id, agent, state and current report revision.",
    arguments: &[],
};

pub const CHILD_STATUS: Tool = Tool {
    name: "child_status",
    description: "Read one of your children: its agent, its state, its head and the body of its latest REPORT.md.",
    arguments: &[CHILD],
};

pub const MERGE: Tool = Tool {
    name: "merge",
    description: "Fast-forward your recorded branch to a reported child's head. Refused, changing nothing, unless the child has reported at exactly expected_head with no tracked changes, your recorded branch is checked out with no tracked changes and it can fast-forward. Follows your permission mode; a Codex pane may merge only when its run started in fullAccess and is otherwise refused with merge_human_only, so the human merges from the child's card.",
    arguments: &[
        CHILD,
        Argument { name: "expected_head", description: "The child's reported head commit, as child_status gave it.", required: true },
    ],
};

pub const FOLLOW_UP: Tool = Tool {
    name: "follow_up",
    description: "Send one of your children a follow-up instruction, at most 2 per child. It reaches the child exactly once, as its next run, and clears its reported state.",
    arguments: &[CHILD, Argument { name: "text", description: "The instruction for the child.", required: true }],
};

pub const DISCARD: Tool = Tool {
    name: "discard",
    description: "Discarding a child is for the human only, from the child's card in the app. This tool always refuses with discard_human_only and changes nothing.",
    arguments: &[CHILD],
};

/// `All`: the six, in this order.
pub const ALL: [&Tool; 6] = [&DELEGATE, &LIST_CHILDREN, &CHILD_STATUS, &MERGE, &FOLLOW_UP, &DISCARD];

/// `Named`.
pub fn named(name: &str) -> Option<&'static Tool> {
    ALL.into_iter().find(|tool| tool.name == name)
}
