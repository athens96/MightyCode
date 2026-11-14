//! Uploads: chunks of 196,608 bytes, 5 MiB a file, 8 MiB a submit, 64 open, 16 per session, 10 minutes, `<dir>\uploads\<guid>.part` (stale
//! ones deleted at start), device-owned, the claim spent only after acceptance (C# R\MobileUploads.cs; plan §1.5). Owner: W8-B.
//!
//! At rest (W8 Step 0): empty.
