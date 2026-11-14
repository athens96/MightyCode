//! `DelegationJson` (C# Core/DelegationFileStore.cs:215-268; Mac-identical): the delegation file's own writer, not `Wire.Json` and not
//! serde: camelCase, nulls omitted, keys sorted ordinal at every level, no whitespace, `\/` escaped, other code points below 0x20 as
//! lowercase `\u00xx`, non-ASCII raw; `EscapedBytes` (the 64 KiB copy and 48 KiB follow-up limits are measured in it). Owner: W7-A.
