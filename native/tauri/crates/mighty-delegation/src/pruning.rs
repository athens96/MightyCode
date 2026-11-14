//! Admission and pruning (C# Core/DelegationPruning.cs:43-80): oldest first, only past 4 MiB plus headroom: report copies of closed
//! and discarded children, then whole fully delivered closed and discarded children with no undoable merge; else `store_full`.
//! Owner: W7-A.
