//! The role view (`?roles=1`): who each block is, a sub-agent's or a background command's compact row, the delegate line, the way back,
//! a request's progress and the session log (C# R\MobileFlow.cs; macOS MobileFlow.swift; `native/contracts/fixtures/mobile-roles.json`
//! holds what both hosts send). The rules are mighty-graph's (`view::mobile`, over `flow`), which the graph's own checks share; this
//! module names them as the m1 API reads them. Owner: W8-B.

pub use mighty_graph::view::mobile::{
    MobileBlockCard, MobileBlockRole, MobileCopyRef, MobileDelegateLine, MobileFlowLog, MobileFlowLogRow, MobileInkedText, MobileProgress,
    MobileProgressRow, MobileReturn, Roles, card_of as card, delegate, flow_log as log, lead, progress, returning, role,
};
