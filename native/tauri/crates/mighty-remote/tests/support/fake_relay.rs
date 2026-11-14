//! The in-process relay (W8-A, D15): a tokio-tungstenite server on loopback port 0 speaking `relay/src/protocol.ts`'s query validation
//! and close codes, the control socket's `connected` notice, and a scripted phone (hello → ready → auth → requests). Owner: W8-A.
