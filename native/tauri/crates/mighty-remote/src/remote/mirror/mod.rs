//! The remote mirror (C# `Core\Remote\RemoteMirror.cs` (pure, 380 lines), `RemoteMirrorDesktopHost.cs`, `RemoteHostService.Mirror.cs`,
//! `RemoteClientService.Mirror.cs`, `Core\DesktopService.Mirror.cs`, from `main` 0.4.61; `w8-plan.md` §3 W8-G, plan §3 W9-J; shared
//! fixtures `native/contracts/fixtures/remote-mirror/`). Owner: W9-J.
//!
//! The host serves `/v2/mirror` (the shared workspaces' agent panes, a revision for cheap polling, details bounded as the phone's) through
//! the phone router's handlers; the client polls it and shows the host's panes, never saved, with the Mac's ids, sends, stops and
//! answers going to the host. The app's half (the servers section, `serverConnect`, `serverDisconnect`, the pane strip and the status
//! bar) is src-tauri's `mirror.rs`.
//!
//! Step 0: empty; W9-J writes it.
