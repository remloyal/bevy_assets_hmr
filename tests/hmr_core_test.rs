//! End-to-end test of the HMR core pipeline.
//!
//! Verifies that:
//! 1. `App::register_config::<T>()` initializes all resources.
//! 2. First load (no prior snapshot) auto-initializes the snapshot and
//!    dispatches **no** refresh (no spurious "first load" event).
//! 3. Subsequent `AssetEvent::Modified` against the snapshot calls
//!    `T::diff(old, new)` and dispatches `ConfigRefresh<T>` with the
//!    correct `changed_ids`.
//! 4. `LastSnapshot<T>` is updated to the new version after dispatch.
//! 5. Empty diffs (content unchanged) are not dispatched.

use bevy::asset::{Asset, AssetId, Assets, Handle};
use bevy::ecs::message::{MessageReader, Messages};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{
    ConfigAsset, ConfigDiff, ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh,
    LastSnapshot, RefreshDebouncer,
};
// 注：`ConfigRefresh<TestDb>` 的泛型是 Config 类型（`ConfigAsset<TestDb>::Config = TestDb`），
// 所以仍是 `ConfigRefresh<TestDb>`。`RefreshDebouncer` / `LastSnapshot` 的泛型是
// Asset 类型，包装模式下用 `ConfigAsset<TestDb>`。
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

/// Test config: a small key->value table.
#[derive(
    Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default, ConfigDiff,
)]
#[config_diff(field = "items", id = "id")]
struct TestDb {
    items: Vec<Item>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
struct Item {
    id: String,
    n: u32,
}

impl TestDb {
    fn lookup(&self, id: &str) -> Option<&Item> {
        self.items.iter().find(|i| i.id == id)
    }
}

#[derive(Resource, Default)]
struct CapturedRefresh(Option<ConfigRefresh<TestDb>>);

fn capture_refresh_system(
    mut reader: MessageReader<ConfigRefresh<TestDb>>,
    mut captured: ResMut<CapturedRefresh>,
) {
    for refresh in reader.read() {
        captured.0 = Some(refresh.clone());
    }
}

/// Helper: build a minimal App with the HMR plugin + TestDb registered.
fn make_app(debounce_window: Duration) -> App {
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    // AssetServer::load 需要 IoTaskPool，测试环境默认没初始化，这里手动建一个。
    IoTaskPool::get_or_init(|| {
        TaskPoolBuilder::new().num_threads(0).build()
    });
    let mut app = App::new();
    app.add_plugins((bevy::asset::AssetPlugin::default(), bevy::time::TimePlugin));
    app.add_plugins(ConfigHmrPlugin {
        debounce_window,
    });
    app.setup_hmr_headless();
    app.register_config::<TestDb>("data/test.ron");
    app.insert_resource(CapturedRefresh::default());
    app.add_systems(Update, capture_refresh_system);
    app
}

fn make_id(seed: u128) -> AssetId<ConfigAsset<TestDb>> {
    AssetId::Uuid {
        uuid: Uuid::from_u128(seed),
    }
}

#[test]
fn register_config_initializes_all_resources() {
    let app = make_app(Duration::from_millis(50));
    let world = app.world();
    assert!(world.get_resource::<Assets<ConfigAsset<TestDb>>>().is_some());
    assert!(world
        .get_resource::<RefreshDebouncer<ConfigAsset<TestDb>>>()
        .is_some());
    assert!(world
        .get_resource::<LastSnapshot<ConfigAsset<TestDb>>>()
        .is_some());
}

#[test]
fn first_load_auto_initializes_snapshot_without_dispatch() {
    // First load via Assets::insert (triggers AssetEvent::Added):
    // the snapshot should be auto-initialized, and no refresh dispatched.
    let mut app = make_app(Duration::from_millis(0));

    let asset_id = make_id(100);
    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        let _ = assets.insert(
            asset_id,
            ConfigAsset {
                raw: TestDb {
                    items: vec![Item {
                        id: "a".into(),
                        n: 1,
                    }],
                },
                source_path: "data/test.ron".into(),
            },
        );
    }

    for _ in 0..3 {
        app.update();
    }

    let captured = app.world().resource::<CapturedRefresh>();
    assert!(
        captured.0.is_none(),
        "no refresh should fire on first load (snapshot auto-init)"
    );

    // Snapshot should now exist, so a subsequent modification will diff.
    let snapshot = app.world().resource::<LastSnapshot<ConfigAsset<TestDb>>>();
    assert!(snapshot.map.contains_key(&asset_id), "snapshot auto-initialized");
}

#[test]
fn modified_event_with_prior_snapshot_dispatches_refresh_with_changed_ids() {
    let mut app = make_app(Duration::from_millis(0));

    // Seed initial asset (triggers Added -> auto-inits snapshot, no dispatch).
    let old_db = TestDb {
        items: vec![
            Item {
                id: "keep".into(),
                n: 1,
            },
            Item {
                id: "modify".into(),
                n: 2,
            },
            Item {
                id: "remove".into(),
                n: 3,
            },
        ],
    };
    let asset_id = make_id(200);
    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        let _ = assets.insert(
            asset_id,
            ConfigAsset {
                raw: old_db,
                source_path: "data/test.ron".into(),
            },
        );
    }

    // Run once so the Added event flushes and the snapshot is initialized.
    app.update();
    app.update();

    // Replace the asset: keep "keep", modify "modify", remove "remove", add "add".
    let new_db = TestDb {
        items: vec![
            Item {
                id: "keep".into(),
                n: 1,
            },
            Item {
                id: "modify".into(),
                n: 20,
            },
            Item {
                id: "add".into(),
                n: 4,
            },
        ],
    };
    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        let _ = assets.insert(
            asset_id,
            ConfigAsset {
                raw: new_db,
                source_path: "data/test.ron".into(),
            },
        );
    }

    for _ in 0..3 {
        app.update();
    }

    let captured = app
        .world()
        .resource::<CapturedRefresh>()
        .0
        .clone()
        .expect("a refresh should have been dispatched");

    let mut changed: Vec<_> = captured.changed_ids.iter().cloned().collect();
    changed.sort();
    assert_eq!(
        changed,
        vec!["add".to_string(), "modify".to_string(), "remove".to_string()],
        "changed_ids should match added+modified+removed"
    );

    let snapshot = app.world().resource::<LastSnapshot<ConfigAsset<TestDb>>>();
    let stored = snapshot.map.get(&asset_id).expect("snapshot should exist");
    assert_eq!(stored.items.len(), 3);
    assert!(stored.lookup("add").is_some());
    assert!(stored.lookup("remove").is_none());
}

#[test]
fn empty_diff_is_not_dispatched() {
    // Re-inserting identical content should produce an empty diff and
    // therefore no refresh event.
    let mut app = make_app(Duration::from_millis(0));

    let db = TestDb {
        items: vec![Item {
            id: "a".into(),
            n: 1,
        }],
    };
    let asset_id = make_id(300);
    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        let _ = assets.insert(
            asset_id,
            ConfigAsset {
                raw: db.clone(),
                source_path: "data/test.ron".into(),
            },
        );
    }
    // Flush Added -> snapshot init.
    app.update();
    app.update();

    // Re-insert identical content.
    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        let _ = assets.insert(
            asset_id,
            ConfigAsset {
                raw: db,
                source_path: "data/test.ron".into(),
            },
        );
    }
    for _ in 0..3 {
        app.update();
    }

    let captured = app.world().resource::<CapturedRefresh>();
    assert!(
        captured.0.is_none(),
        "empty diff should not dispatch a refresh"
    );
}

#[test]
fn messages_resource_is_registered() {
    let app = make_app(Duration::from_millis(50));
    assert!(app
        .world()
        .get_resource::<Messages<ConfigRefresh<TestDb>>>()
        .is_some());
}

#[test]
fn default_handle_can_be_constructed() {
    let _handle: Handle<ConfigAsset<TestDb>> = Handle::default();
}
