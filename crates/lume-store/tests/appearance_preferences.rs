#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]

use lume_store::{AppearancePreferences, SessionSnapshot, StoreHandle};

#[test]
fn first_run_defaults_to_officina_and_material() {
    let preferences = AppearancePreferences::default();
    assert_eq!(preferences.color_theme_id, "officina");
    assert_eq!(preferences.icon_theme_id, "material");
    assert_eq!(SessionSnapshot::empty().appearance, preferences);
}

#[test]
fn legacy_theme_name_migrates_to_stable_color_id() {
    let legacy = serde_json::json!({
        "workspace_root": null,
        "tabs": [],
        "active_tab": null,
        "theme_name": "Grafite",
        "panel_width": 220.0,
        "left_tools": [],
        "right_tools": [],
        "active_panel": null,
        "open_tools": [],
        "tool_heights": {},
        "environment_id": "local"
    });
    let migrated: SessionSnapshot = serde_json::from_value(legacy).unwrap();
    assert_eq!(migrated.appearance.color_theme_id, "grafite");
    assert_eq!(migrated.appearance.icon_theme_id, "material");

    let serialized = serde_json::to_value(migrated).unwrap();
    assert!(serialized.get("theme_name").is_none());
    assert_eq!(serialized["appearance"]["color_theme_id"], "grafite");
}

#[test]
fn appearance_ids_roundtrip_atomically_with_the_session() {
    let path = std::env::temp_dir().join(format!(
        "lume_appearance_{}_{}.sqlite3",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let store = futures::executor::block_on(StoreHandle::open_at(&path)).unwrap();
    let mut snapshot = SessionSnapshot::empty();
    snapshot.appearance = AppearancePreferences::new("ossido", "external:night");
    snapshot.dock_height = 340.0;
    snapshot.active_dock = Some("problems".into());
    futures::executor::block_on(store.save_session(snapshot.clone())).unwrap();
    let loaded = futures::executor::block_on(store.load_session()).unwrap().0;
    assert_eq!(loaded, snapshot);
}
