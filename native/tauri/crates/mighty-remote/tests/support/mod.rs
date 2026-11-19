//! What the remote checks share, one module per owner: [`lane`] (W8 Step 0) serializes them within a test binary; `fake_relay` (W8-A)
//! is an in-process tokio-tungstenite relay speaking `relay/src/protocol.ts`'s query checks and close codes with a scripted phone;
//! `desk` (W8-B) is the headless `PhoneDesk` over a real `RunService`; `fake_host` (W8-D) is C#'s `RemoteFakeHost`. No check contacts a
//! real relay or push server, binds anything but loopback (never `0.0.0.0`), or names the user's profile.
#![allow(dead_code)]

pub mod desk;
pub mod fake_host;
pub mod fake_relay;
pub mod lane;
pub mod pairing_relay;

use std::sync::Arc;

use mighty_platform::ProcessHost;

/// The platform's process host (Windows', or M-A's on the Mac) for the checks that run a real command or the fake agy.
pub fn processes() -> Arc<dyn ProcessHost> {
    #[cfg(windows)]
    {
        Arc::new(mighty_platform_windows::WindowsProcessHost::new())
    }
    #[cfg(target_os = "macos")]
    {
        mighty_platform_macos::process_host()
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        Arc::new(mighty_platform::unsupported::Unsupported)
    }
}
