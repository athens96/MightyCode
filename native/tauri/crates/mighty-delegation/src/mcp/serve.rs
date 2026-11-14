//! The stdio JSON-RPC loop both servers share (C# AgentIOMcp.cs:20-92, DelegationMcp.cs:38-97): newline-delimited, `\r` trimmed, a
//! line over 1,048,576 bytes gives `-32700`, up to 16 lines at once, each answer written whole on its line; `initialize` (the four
//! protocol versions), `ping`, the error codes of W7 plan section 1.3. Owner: W7-C.
