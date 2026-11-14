//! W8 Step 0: the companion at rest shows nothing and asks nothing, and its scene's JSON never carries the atlas's pixels.

use std::sync::Arc;

use mighty_companion::Companion;
use mighty_companion::scene::{PetBitmap, PetScene};
use mighty_platform::{PetEvent, PetPointer};

#[test]
fn the_companion_at_rest_draws_nothing() {
    let mut companion = Companion::default();
    companion.tick();
    assert_eq!(companion.scene(), PetScene::default());
    assert_eq!(companion.act(&PetEvent::Pressed(PetPointer::default())), None);
}

#[test]
fn a_scene_snapshot_names_the_atlas_without_its_pixels() {
    let atlas = PetBitmap { key: "mighty-raccoon@1".into(), width: 2, height: 1, bgra: Arc::new(vec![1, 2, 3, 4, 5, 6, 7, 8]) };
    let scene = PetScene { width: 258.0, height: 330.0, items: vec![], atlas: Some(atlas) };
    let json = serde_json::to_string(&scene).unwrap();
    assert_eq!(json, r#"{"width":258.0,"height":330.0,"items":[],"atlas":{"key":"mighty-raccoon@1","width":2,"height":1}}"#);
}
