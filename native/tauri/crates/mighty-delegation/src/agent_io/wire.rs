//! The transport's framing (C# Core/AgentIO/AgentIOPipe.cs:167-179; M/AgentIOSocketServer.swift), chosen by mighty-platform's
//! `McpTransport`: Windows a 4-byte big-endian length then UTF-8 JSON (1..1,048,576; `0x7fffffff` refused), the Mac one JSON line.
//! Pure, tested on both OSes. Owner: W7-C.
