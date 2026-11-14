//! `DelegationPlace` (C# MightyCode.Core.Tests/DelegationGitVerification.cs:10-115): a temporary `Base/{repo, worktrees, profile}`,
//! the maker with 50 GB of fake free space (mighty-platform's `FixedDiskSpace`), and the fixture's git — every `GIT_*` removed,
//! `GIT_CONFIG_GLOBAL=<Base>/fixture.gitconfig` (empty; `/dev/null` on the Mac), `GIT_CONFIG_NOSYSTEM=1`, `GIT_OPTIONAL_LOCKS=0`,
//! `GIT_TERMINAL_PROMPT=0`, author and committer `Fixture <fixture@example.invalid>`, 120 s per call; `Repository(commits, folder,
//! autocrlf)` (`git init -q -b main`, commits adding `file-<i>.txt`), `Child`, `ReportedChild`, `Commit`, `Snapshot`, `MightyBranches`,
//! `Disk`. It never touches `%USERPROFILE%\.mightycode`, the user's global git config or a real repository. Owner: W7-B (published
//! first: W7-A's and W7-D's git checks run in it).
