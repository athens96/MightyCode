//! Cleanup and discard (C# Core/DelegationCleanup.cs; the fix branch's `RemoveOwnWorktree(id, path, branch, ...)`): close cleanup never
//! forces (`worktree remove`, then `branch -d`); the human's discard unlinks junctions first, forces nested worktrees deepest first,
//! clears read-only leftovers and `branch -D`. Never `worktree prune`, `reset --hard`, push or fetch. Owner: W7-B.
