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
    ConfigAsset, ConfigBind, ConfigDiff, ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh,
    ConfigReloadFailed, ConfigRemoved, HandleEntityCache, LastSnapshot, RefreshDebouncer,
};
// 注：`ConfigRefresh<TestDb>` 的泛型是 Config 类型（`ConfigAsset<TestDb>::Config = TestDb`），
// 所以仍是 `ConfigRefresh<TestDb>`。`RefreshDebouncer` / `LastSnapshot` 的泛型是
// Asset 类型，包装模式下用 `ConfigAsset<TestDb>`。
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

/// Test config: a small key->value table.
#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default, ConfigDiff)]
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
    IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(0).build());
    let mut app = App::new();
    app.add_plugins((bevy::asset::AssetPlugin::default(), bevy::time::TimePlugin));
    app.add_plugins(ConfigHmrPlugin { debounce_window });
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
    assert!(
        world
            .get_resource::<Assets<ConfigAsset<TestDb>>>()
            .is_some()
    );
    assert!(
        world
            .get_resource::<RefreshDebouncer<ConfigAsset<TestDb>>>()
            .is_some()
    );
    assert!(
        world
            .get_resource::<LastSnapshot<ConfigAsset<TestDb>>>()
            .is_some()
    );
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
    assert!(
        snapshot.map.contains_key(&asset_id),
        "snapshot auto-initialized"
    );
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
        vec![
            "add".to_string(),
            "modify".to_string(),
            "remove".to_string()
        ],
        "changed_ids should match added+modified+removed"
    );

    let snapshot = app.world().resource::<LastSnapshot<ConfigAsset<TestDb>>>();
    let stored = snapshot.map.get(&asset_id).expect("snapshot should exist");
    assert_eq!(stored.items.len(), 3);
    assert!(stored.lookup("add").is_some());
    assert!(stored.lookup("remove").is_none());

    // Problem 8: ConfigRefresh should carry the source_path from the asset.
    assert_eq!(
        captured.source_path, "data/test.ron",
        "ConfigRefresh should carry the asset's source_path"
    );
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
    assert!(
        app.world()
            .get_resource::<Messages<ConfigRefresh<TestDb>>>()
            .is_some()
    );
}

#[test]
fn default_handle_can_be_constructed() {
    let _handle: Handle<ConfigAsset<TestDb>> = Handle::default();
}

#[test]
fn despawned_entity_is_removed_from_cache() {
    // Regression test for the dangling-reference bug: when an entity with
    // `ConfigBind<A>` is despawned, `config_binding_cleanup_system` must
    // remove it from `HandleEntityCache<A>` so subsequent refreshes don't
    // include stale entity ids in `target_entities`.
    let mut app = make_app(Duration::from_millis(50));

    // Spawn an entity with a ConfigBind bound to a known asset id.
    let asset_id = make_id(42);
    let entity = {
        let world = app.world_mut();
        let entity = world
            .spawn(ConfigBind::<ConfigAsset<TestDb>>::with_id(asset_id))
            .id();
        // Run registry + cleanup so the binding is registered.
        entity
    };

    // Run a few frames so config_binding_registry_system picks up the Added.
    for _ in 0..3 {
        app.update();
    }

    // The entity should now be in the cache.
    {
        let cache = app
            .world()
            .resource::<HandleEntityCache<ConfigAsset<TestDb>>>();
        let bound: Vec<_> = cache
            .get_entities(&asset_id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        assert!(
            bound.contains(&entity),
            "entity should be cached before despawn, got {bound:?}"
        );
    }

    // Despawn the entity.
    app.world_mut().despawn(entity);

    // Run frames so config_binding_cleanup_system (RemovedComponents) fires.
    for _ in 0..3 {
        app.update();
    }

    // The entity must no longer be in the cache (no dangling reference).
    {
        let cache = app
            .world()
            .resource::<HandleEntityCache<ConfigAsset<TestDb>>>();
        let bound: Vec<_> = cache
            .get_entities(&asset_id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        assert!(
            !bound.contains(&entity),
            "despawned entity must be removed from cache, got {bound:?}"
        );
    }
}

// ---- Problem 10 tests: AssetEvent::Removed dispatches ConfigRemoved ----

#[derive(Resource, Default)]
struct CapturedRemoved(Option<ConfigRemoved<TestDb>>);

fn capture_removed_system(
    mut reader: MessageReader<ConfigRemoved<TestDb>>,
    mut captured: ResMut<CapturedRemoved>,
) {
    for removed in reader.read() {
        captured.0 = Some(removed.clone());
    }
}

/// Helper: build an app that also captures `ConfigRemoved` events.
fn make_app_with_removed_capture(debounce_window: Duration) -> App {
    let mut app = make_app(debounce_window);
    app.insert_resource(CapturedRemoved::default());
    app.add_systems(Update, capture_removed_system);
    app
}

#[test]
fn removed_asset_dispatches_config_removed_with_source_path() {
    // Problem 10: when an asset is removed via Assets::remove, subscribers
    // should receive a ConfigRemoved event carrying the source_path and the
    // bound target_entities.
    let mut app = make_app_with_removed_capture(Duration::from_millis(0));

    // Seed an asset (triggers Added -> auto-inits snapshot, no dispatch).
    let asset_id = make_id(500);
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
                source_path: "data/deleted.ron".into(),
            },
        );
    }
    // Run so the Added event flushes and the snapshot is initialized.
    for _ in 0..3 {
        app.update();
    }

    // No removed event yet.
    assert!(
        app.world().resource::<CapturedRemoved>().0.is_none(),
        "no ConfigRemoved should fire on initial load"
    );

    // Remove the asset (triggers AssetEvent::Removed).
    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        assets.remove(asset_id);
    }

    // Run so the Removed event is enqueued and flushed.
    for _ in 0..3 {
        app.update();
    }

    let captured = app
        .world()
        .resource::<CapturedRemoved>()
        .0
        .clone()
        .expect("a ConfigRemoved should have been dispatched");

    assert_eq!(
        captured.source_path, "data/deleted.ron",
        "ConfigRemoved should carry the source_path recorded before removal"
    );

    // The snapshot must be cleaned up.
    let snapshot = app.world().resource::<LastSnapshot<ConfigAsset<TestDb>>>();
    assert!(
        !snapshot.map.contains_key(&asset_id),
        "snapshot must be removed after asset removal"
    );
    assert!(
        !snapshot.source_paths.contains_key(&asset_id),
        "source_paths entry must be removed after asset removal"
    );
}

#[test]
fn removed_asset_without_prior_snapshot_does_not_dispatch() {
    // Removing an asset that was never loaded (no snapshot) should be a
    // silent no-op — no ConfigRemoved event.
    let mut app = make_app_with_removed_capture(Duration::from_millis(0));

    let asset_id = make_id(600);
    // Insert then immediately remove without running any frames, so the
    // Added event never flushes and no snapshot is created.
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
                source_path: "data/never.ron".into(),
            },
        );
        assets.remove(asset_id);
    }

    for _ in 0..5 {
        app.update();
    }

    assert!(
        app.world().resource::<CapturedRemoved>().0.is_none(),
        "no ConfigRemoved should fire for an asset that was never snapshotted"
    );
}

// ===========================================================================
// watch_asset tests: file-level hot-reload for arbitrary Asset types
// ===========================================================================

/// A simple Asset that does NOT implement ConfigDiff - used to verify
/// `watch_asset` works without the diff machinery.
#[derive(Asset, TypePath, Clone, Debug, PartialEq, Default)]
struct DummyTexture {
    width: u32,
    height: u32,
}

#[derive(Resource, Default)]
struct CapturedAssetChanged(Option<bevy_assets_hmr::AssetChanged<DummyTexture>>);

fn capture_asset_changed_system(
    mut reader: bevy::ecs::message::MessageReader<bevy_assets_hmr::AssetChanged<DummyTexture>>,
    mut captured: ResMut<CapturedAssetChanged>,
) {
    for evt in reader.read() {
        captured.0 = Some(evt.clone());
    }
}

fn make_watch_app() -> App {
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(0).build());
    let mut app = App::new();
    app.add_plugins((bevy::asset::AssetPlugin::default(), bevy::time::TimePlugin));
    app.add_plugins(ConfigHmrPlugin::default());
    app.setup_hmr_headless();
    app.init_asset::<DummyTexture>();
    app.insert_resource(CapturedAssetChanged::default());
    app.add_systems(Update, capture_asset_changed_system);
    // watch_asset normally loads a file in Startup; for testing we skip that
    // and insert assets directly, so pass a dummy path.
    app.watch_asset::<DummyTexture>("textures/dummy.png");
    app
}

#[test]
fn watch_asset_dispatches_asset_changed_on_modified() {
    let mut app = make_watch_app();

    // Seed an asset (triggers Added -> AssetChanged dispatched).
    let asset_id = AssetId::Uuid {
        uuid: Uuid::from_u128(700),
    };
    {
        let mut assets = app.world_mut().resource_mut::<Assets<DummyTexture>>();
        let _ = assets.insert(
            asset_id,
            DummyTexture {
                width: 64,
                height: 64,
            },
        );
    }

    // Run frames so the Added event flushes.
    for _ in 0..3 {
        app.update();
    }

    // AssetChanged should have fired for the Added event.
    {
        let captured = app.world().resource::<CapturedAssetChanged>();
        assert!(captured.0.is_some(), "AssetChanged should fire on Added");
        let evt = captured.0.as_ref().unwrap();
        assert_eq!(evt.asset_id, asset_id);
        assert_eq!(evt.new_asset.width, 64);
    }

    // Clear the capture.
    app.world_mut().resource_mut::<CapturedAssetChanged>().0 = None;

    // Modify the asset (triggers Modified -> AssetChanged dispatched).
    {
        let mut assets = app.world_mut().resource_mut::<Assets<DummyTexture>>();
        let _ = assets.insert(
            asset_id,
            DummyTexture {
                width: 128,
                height: 128,
            },
        );
    }

    for _ in 0..3 {
        app.update();
    }

    let captured = app.world().resource::<CapturedAssetChanged>();
    let evt = captured
        .0
        .as_ref()
        .expect("AssetChanged should fire on Modified");
    assert_eq!(evt.asset_id, asset_id);
    assert_eq!(
        evt.new_asset.width, 128,
        "AssetChanged should carry the new asset value"
    );
}

#[test]
fn watch_asset_tracks_bound_entities() {
    // Entities with AssetBind<A> should appear in target_entities.
    use bevy_assets_hmr::AssetBind;

    let mut app = make_watch_app();

    let asset_id = AssetId::Uuid {
        uuid: Uuid::from_u128(800),
    };
    {
        let mut assets = app.world_mut().resource_mut::<Assets<DummyTexture>>();
        let _ = assets.insert(
            asset_id,
            DummyTexture {
                width: 32,
                height: 32,
            },
        );
    }

    // Spawn an entity bound to this asset via AssetBind.
    let entity = app
        .world_mut()
        .spawn(AssetBind::<DummyTexture>::with_id(asset_id))
        .id();

    // Run frames so registry picks up the binding + Added event fires.
    for _ in 0..3 {
        app.update();
    }

    let captured = app.world().resource::<CapturedAssetChanged>();
    let evt = captured
        .0
        .as_ref()
        .expect("AssetChanged should fire on Added");
    assert!(
        evt.target_entities.contains(&entity),
        "target_entities should contain the AssetBind entity, got {:?}",
        evt.target_entities
    );
}

#[test]
fn watch_asset_despawned_entity_removed_from_cache() {
    // Despawning an AssetBind entity should remove it from AssetBindCache.
    use bevy_assets_hmr::{AssetBind, AssetBindCache};

    let mut app = make_watch_app();

    let asset_id = AssetId::Uuid {
        uuid: Uuid::from_u128(900),
    };
    {
        let mut assets = app.world_mut().resource_mut::<Assets<DummyTexture>>();
        let _ = assets.insert(
            asset_id,
            DummyTexture {
                width: 16,
                height: 16,
            },
        );
    }

    let entity = app
        .world_mut()
        .spawn(AssetBind::<DummyTexture>::with_id(asset_id))
        .id();

    for _ in 0..3 {
        app.update();
    }

    // Entity should be in the cache.
    {
        let cache = app.world().resource::<AssetBindCache<DummyTexture>>();
        let bound: Vec<_> = cache
            .get_entities(&asset_id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        assert!(bound.contains(&entity));
    }

    // Despawn and run frames.
    app.world_mut().despawn(entity);
    for _ in 0..3 {
        app.update();
    }

    // Entity must be gone from the cache.
    {
        let cache = app.world().resource::<AssetBindCache<DummyTexture>>();
        let bound: Vec<_> = cache
            .get_entities(&asset_id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();
        assert!(
            !bound.contains(&entity),
            "despawned entity must be removed from AssetBindCache"
        );
    }
}

// ===========================================================================
// Rollback + ConfigReloadFailed tests
// ===========================================================================

#[derive(Resource, Default)]
struct CapturedReloadFailed(Option<ConfigReloadFailed<TestDb>>);

fn capture_reload_failed_system(
    mut reader: MessageReader<ConfigReloadFailed<TestDb>>,
    mut captured: ResMut<CapturedReloadFailed>,
) {
    for evt in reader.read() {
        captured.0 = Some(evt.clone());
    }
}

#[test]
fn rollback_on_asset_load_failure_dispatches_config_reload_failed() {
    // Scenario: an asset has a prior snapshot but the asset is removed from
    // Assets (simulating a loader failure on reload). flush_debounced_refresh
    // should:
    // 1. Reconstruct the asset from snapshot via HmrSource::from_config
    // 2. Re-insert it into Assets (rollback)
    // 3. Dispatch ConfigReloadFailed

    let mut app = make_app(Duration::from_millis(0));
    app.add_systems(Update, capture_reload_failed_system);
    app.insert_resource(CapturedReloadFailed::default());

    let asset_id = make_id(200);

    // Step 1: Insert initial asset (first load → snapshot auto-created)
    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        let _ = assets.insert(
            asset_id,
            ConfigAsset {
                raw: TestDb {
                    items: vec![Item {
                        id: "x".into(),
                        n: 42,
                    }],
                },
                source_path: "data/test.ron".into(),
            },
        );
    }

    // Run frames: snapshot should be auto-initialized.
    for _ in 0..3 {
        app.update();
    }

    {
        let snapshots = app.world().resource::<LastSnapshot<ConfigAsset<TestDb>>>();
        assert!(
            snapshots.map.contains_key(&asset_id),
            "snapshot should exist after first load"
        );
    }

    // Step 2: Remove the asset from Assets (simulate loader failure).
    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        assets.remove(asset_id);
    }

    // Step 3: Manually enqueue the id in the debouncer so flush runs.
    {
        let mut debouncer = app
            .world_mut()
            .resource_mut::<RefreshDebouncer<ConfigAsset<TestDb>>>();
        debouncer.pending.insert(asset_id, std::time::Instant::now());
    }

    // Step 4: Run frames → flush should detect missing asset, rollback,
    //         and dispatch ConfigReloadFailed.
    for _ in 0..5 {
        app.update();
    }

    // Verify ConfigReloadFailed was dispatched.
    let captured = app.world().resource::<CapturedReloadFailed>();
    let evt = captured
        .0
        .as_ref()
        .expect("ConfigReloadFailed should be dispatched on rollback");
    assert_eq!(evt.source_path, "data/test.ron");
    assert_eq!(
        evt.current_config.items[0].id, "x",
        "current_config should be the old snapshot value"
    );
    assert_eq!(evt.current_config.items[0].n, 42);
    assert!(
        evt.error.contains("rolled back"),
        "error message should mention rollback"
    );

    // Verify asset was re-inserted into Assets (rollback).
    let assets = app.world().resource::<Assets<ConfigAsset<TestDb>>>();
    let rolled_back = assets
        .get(asset_id)
        .expect("asset should be re-inserted by rollback");
    assert_eq!(
        rolled_back.raw.items[0].n, 42,
        "re-inserted asset should match snapshot value"
    );
    assert_eq!(rolled_back.source_path, "data/test.ron");
}

#[test]
fn no_rollback_when_no_snapshot_exists() {
    // If an asset id has no prior snapshot, assets.get(id) → None should
    // just clean up (no crash, no ConfigReloadFailed, nothing re-inserted).

    let mut app = make_app(Duration::from_millis(0));
    app.add_systems(Update, capture_reload_failed_system);
    app.insert_resource(CapturedReloadFailed::default());

    let asset_id = make_id(201);

    // Manually enqueue an id that has no snapshot (and no asset).
    {
        let mut debouncer = app
            .world_mut()
            .resource_mut::<RefreshDebouncer<ConfigAsset<TestDb>>>();
        debouncer.pending.insert(asset_id, std::time::Instant::now());
    }

    for _ in 0..3 {
        app.update();
    }

    // No ConfigReloadFailed should be dispatched.
    let captured = app.world().resource::<CapturedReloadFailed>();
    assert!(
        captured.0.is_none(),
        "no ConfigReloadFailed when no snapshot exists"
    );

    // Asset should NOT be in Assets (nothing was re-inserted).
    let assets = app.world().resource::<Assets<ConfigAsset<TestDb>>>();
    assert!(assets.get(asset_id).is_none());
}
