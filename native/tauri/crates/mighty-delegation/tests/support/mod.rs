//! What the delegation checks share, one module per owner: [`lane`] (W7 Step 0) serializes them within a test binary; `host` (W7-A) is
//! C#'s `DelegationTestHost`; `place` (W7-B, published first) is C#'s `DelegationPlace`, the temporary repositories every git check
//! runs in; `launch` (W7-D) is C#'s `DelegationLaunch`, the real run path with the pipe, git and the fake Claude. No check names the
//! user's `%USERPROFILE%\.mightycode`, their global git config or any real repository.
#![allow(dead_code)]

pub mod host;
pub mod lane;
pub mod launch;
pub mod place;
