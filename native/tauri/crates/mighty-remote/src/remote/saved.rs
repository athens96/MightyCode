//! `remote-connections.json` (`{"version":2,"connections":[{id,name,address}]}`, at most 256 KiB and 16 entries, no keys; the keys in the
//! vault, read at first use; written only with `--allow-state-write`, else in memory) (C# R\RemoteConnectionFile.cs; plan §1.6). Owner:
//! W8-D.
//!
//! At rest (W8 Step 0): empty.
