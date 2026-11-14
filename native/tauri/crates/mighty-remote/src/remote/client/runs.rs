//! The client's runs (C# `RemoteClientService.Runs`, R\RemoteClientService.Runs.cs; plan §1.6): the start, the poll loop (100 ms
//! while more waits, else 250 ms + 150 ms × quiet up to 4), transient failures retried every 1 s for 10 s, the route probe before each
//! request, steer, answers, stop. Owner: W8-D.
//!
//! At rest (W8 Step 0): empty.
