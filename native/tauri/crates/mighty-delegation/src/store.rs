//! `delegation-state.json` (C# Core/DelegationFileStore.cs:139-208, `DelegationFile`, `DelegationFileStore`): version 1 beside
//! `workspace-state.json`; load (missing: empty; a directory, a link or over 4 MiB: Unreadable; 40 x 50 ms retries; BOM sniff) and
//! save (prune first, `<path>.<guid>.tmp` then replace, 40 x 50 ms; 0600 on Unix; no fsync; `SaveOrKeep`). Owner: W7-A.
