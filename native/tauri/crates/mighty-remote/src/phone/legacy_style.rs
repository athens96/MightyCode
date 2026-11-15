//! Older phones' Ouroboros payload (C# `MobileLegacyStyleAdapter`, `Core\Remote\MobileMighty.cs:433-543`; plan §1.7 "The phone", W9-D).
//! Owner: W9-D.
//!
//! Only the bundled `ouroboros` style is projected into the legacy `ouroboros` object an older phone reads (L477 "Mobile host: older
//! phones still get the Ouroboros payload"); every other style reaches the phone as `panel` and `styleId` only.
//!
//! Step 0: empty; W9-D writes it.
