//! The Swift app's pet checks the Mac stage ports onto the shared layout (M-F; plan §3 M-F "Swift checks to port").

use mighty_companion::layout;

/// Swift `ResizeEdgesTests.swift:38` "thePetWindowFitsItsBubble": the panel around the bubble, the default, a sized one, one held to
/// the largest bubble, the approval's own height with the bubble's width, and a narrow bubble (the approval never below the default).
#[test]
fn the_pet_window_fits_its_bubble() {
    assert_eq!(layout::panel_size(None, None, false), (282.0, 330.0));
    assert_eq!(layout::panel_size(None, None, true), (282.0, 494.0));
    assert_eq!(layout::panel_size(Some(400.0), Some(300.0), false), (424.0, 453.0));
    assert_eq!(layout::panel_size(Some(9000.0), Some(9000.0), false), (664.0, 633.0));
    assert_eq!(layout::panel_size(Some(400.0), Some(300.0), true), (424.0, 494.0));
    assert_eq!(layout::panel_size(Some(220.0), None, false), (244.0, 330.0));
    assert_eq!(layout::panel_size(Some(220.0), None, true), (282.0, 494.0));
}
