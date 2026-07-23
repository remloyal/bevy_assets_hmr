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

use bevy::asset::{
    Asset, AssetId, AssetLoadError, AssetLoadFailedEvent, AssetPath, Assets, Handle,
};
use bevy::ecs::message::{MessageReader, Messages};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{
    ConfigAsset, ConfigBind, ConfigDiff, ConfigHandle, ConfigHmrAppExt, ConfigHmrPlugin,
    ConfigRefresh, ConfigReloadFailed, ConfigRemoved, HandleEntityCache, HmrMetrics, HmrSource,
    LastSnapshot, RefreshCause, RefreshDebouncer,
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

fn estimate_test_db_bytes(config: &TestDb) -> usize {
    std::mem::size_of::<TestDb>()
        + config.items.capacity() * std::mem::size_of::<Item>()
        + config
            .items
            .iter()
            .map(|item| item.id.capacity())
            .sum::<usize>()
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
    make_app_at(debounce_window, "data/test.ron")
}

fn make_app_at(debounce_window: Duration, path: &str) -> App {
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    // AssetServer::load 需要 IoTaskPool，测试环境默认没初始化，这里手动建一个。
    IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(1).build());
    let mut app = App::new();
    app.add_plugins((
        bevy::app::TaskPoolPlugin::default(),
        bevy::asset::AssetPlugin::default(),
        bevy::time::TimePlugin,
    ));
    app.add_plugins(ConfigHmrPlugin { debounce_window });
    app.setup_hmr_headless();
    app.register_config::<TestDb>(path);
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
    assert_eq!(captured.asset_id, asset_id.untyped());
    assert_eq!(captured.delta.added, ["add".to_string()].into());
    assert_eq!(captured.delta.removed, ["remove".to_string()].into());
    assert_eq!(captured.delta.modified, ["modify".to_string()].into());
    assert_eq!(captured.cause, RefreshCause::Direct);

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
fn refresh_metrics_report_diff_time_and_config_clone_pressure() {
    let mut app = make_app(Duration::from_millis(0));
    let asset_id = make_id(205);
    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        let _ = assets.insert(
            asset_id,
            ConfigAsset {
                raw: TestDb {
                    items: vec![Item {
                        id: "entry".into(),
                        n: 1,
                    }],
                },
                source_path: "data/metrics.ron".into(),
            },
        );
    }
    for _ in 0..3 {
        app.update();
    }
    *app.world_mut()
        .resource_mut::<HmrMetrics<ConfigAsset<TestDb>>>() = HmrMetrics::default();
    app.register_config_size_estimator::<TestDb>(estimate_test_db_bytes);

    let _ = app
        .world_mut()
        .resource_mut::<Assets<ConfigAsset<TestDb>>>()
        .insert(
            asset_id,
            ConfigAsset {
                raw: TestDb {
                    items: vec![Item {
                        id: "entry".into(),
                        n: 2,
                    }],
                },
                source_path: "data/metrics.ron".into(),
            },
        );
    for _ in 0..3 {
        app.update();
    }

    let metrics = app.world().resource::<HmrMetrics<ConfigAsset<TestDb>>>();
    assert_eq!(metrics.diff_runs, 1);
    assert_eq!(metrics.refreshes_dispatched, 1);
    assert!(metrics.total_diff_time_ns > 0);
    assert_eq!(metrics.config_clone_count, 2);
    assert_eq!(metrics.unestimated_clone_count, 0);
    let refreshed = TestDb {
        items: vec![Item {
            id: "entry".into(),
            n: 2,
        }],
    };
    assert_eq!(
        metrics.estimated_clone_bytes,
        (2 * estimate_test_db_bytes(&refreshed)) as u64
    );
}

#[derive(Resource, Default)]
struct CapturedRefreshes(Vec<ConfigRefresh<TestDb>>);

fn capture_all_refreshes_system(
    mut reader: MessageReader<ConfigRefresh<TestDb>>,
    mut captured: ResMut<CapturedRefreshes>,
) {
    captured.0.extend(reader.read().cloned());
}

#[test]
fn same_config_type_multiple_assets_keep_distinct_asset_ids() {
    let mut app = make_app(Duration::from_millis(0));
    app.insert_resource(CapturedRefreshes::default());
    app.add_systems(Update, capture_all_refreshes_system);
    let first_id = make_id(210);
    let second_id = make_id(211);

    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        for (id, path) in [(first_id, "data/first.ron"), (second_id, "data/second.ron")] {
            let _ = assets.insert(
                id,
                ConfigAsset {
                    raw: TestDb {
                        items: vec![Item {
                            id: "entry".into(),
                            n: 1,
                        }],
                    },
                    source_path: path.into(),
                },
            );
        }
    }
    for _ in 0..3 {
        app.update();
    }
    app.world_mut()
        .resource_mut::<CapturedRefreshes>()
        .0
        .clear();

    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        for (id, path, value) in [
            (first_id, "data/first.ron", 2),
            (second_id, "data/second.ron", 3),
        ] {
            let _ = assets.insert(
                id,
                ConfigAsset {
                    raw: TestDb {
                        items: vec![Item {
                            id: "entry".into(),
                            n: value,
                        }],
                    },
                    source_path: path.into(),
                },
            );
        }
    }
    for _ in 0..3 {
        app.update();
    }

    let captured = &app.world().resource::<CapturedRefreshes>().0;
    assert_eq!(captured.len(), 2);
    assert!(
        captured
            .iter()
            .any(|event| event.asset_id == first_id.untyped())
    );
    assert!(
        captured
            .iter()
            .any(|event| event.asset_id == second_id.untyped())
    );
}

#[test]
fn same_config_type_registrations_keep_deduplicated_paths_and_handles() {
    let mut app = make_app(Duration::from_millis(0));
    app.register_config::<TestDb>("data/first.ron")
        .register_config::<TestDb>("data/second.ron")
        .register_config::<TestDb>("data/first.ron");

    // Startup should be installed only once, but it must load every unique
    // path registered for this asset type.
    app.update();

    let registry = app
        .world()
        .resource::<bevy_assets_hmr::ConfigPathRegistry>();
    assert_eq!(
        registry.paths.get(ConfigAsset::<TestDb>::type_path()),
        Some(&vec![
            "data/test.ron".to_string(),
            "data/first.ron".to_string(),
            "data/second.ron".to_string(),
        ])
    );

    let holder = app.world().resource::<ConfigHandle<ConfigAsset<TestDb>>>();
    assert_eq!(holder.handles.len(), 3);
    let ids: std::collections::HashSet<_> =
        holder.handles.iter().map(|handle| handle.id()).collect();
    assert_eq!(ids.len(), 3);
}

#[test]
fn same_config_type_assets_remove_and_fail_independently() {
    let mut app = make_app(Duration::from_millis(0));
    app.insert_resource(CapturedRemoved::default());
    app.insert_resource(CapturedReloadFailed::default());
    app.add_systems(
        Update,
        (capture_removed_system, capture_reload_failed_system),
    );
    let removed_id = make_id(212);
    let failed_id = make_id(213);

    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        for (id, path, value) in [
            (removed_id, "data/removed.ron", 1),
            (failed_id, "data/failed.ron", 2),
        ] {
            let _ = assets.insert(
                id,
                ConfigAsset {
                    raw: TestDb {
                        items: vec![Item {
                            id: "entry".into(),
                            n: value,
                        }],
                    },
                    source_path: path.into(),
                },
            );
        }
    }
    for _ in 0..3 {
        app.update();
    }

    app.world_mut()
        .resource_mut::<Assets<ConfigAsset<TestDb>>>()
        .remove(removed_id);
    app.world_mut()
        .resource_mut::<Messages<AssetLoadFailedEvent<ConfigAsset<TestDb>>>>()
        .write(AssetLoadFailedEvent {
            id: failed_id,
            path: AssetPath::from("data/failed.ron"),
            error: AssetLoadError::EmptyPath(AssetPath::from("")),
        });
    for _ in 0..3 {
        app.update();
    }

    let removed = app
        .world()
        .resource::<CapturedRemoved>()
        .0
        .as_ref()
        .expect("the removed file should dispatch ConfigRemoved");
    assert_eq!(removed.asset_id, removed_id.untyped());
    assert_eq!(removed.source_path, "data/removed.ron");

    let failures = &app.world().resource::<CapturedReloadFailed>().0;
    let failed = failures
        .iter()
        .find(|event| event.asset_id == failed_id.untyped())
        .expect("the failed file should dispatch ConfigReloadFailed");
    assert_eq!(failed.source_path, "data/failed.ron");
    assert_eq!(failed.current_config.as_ref().unwrap().items[0].n, 2);
    assert!(
        app.world()
            .resource::<Assets<ConfigAsset<TestDb>>>()
            .get(failed_id)
            .is_some()
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

#[test]
fn changed_handle_updates_both_cache_directions() {
    let mut app = make_app(Duration::from_millis(0));
    let old_id = make_id(43);
    let new_id = make_id(44);
    let entity = app
        .world_mut()
        .spawn(ConfigBind::<ConfigAsset<TestDb>>::with_id(old_id))
        .id();
    app.update();

    app.world_mut()
        .entity_mut(entity)
        .insert(ConfigBind::<ConfigAsset<TestDb>>::with_id(new_id));
    app.update();

    let cache = app
        .world()
        .resource::<HandleEntityCache<ConfigAsset<TestDb>>>();
    assert!(cache.get_entities(&old_id).is_none());
    assert!(cache.get_entities(&new_id).unwrap().contains(&entity));
    assert_eq!(cache.entity_to_handle[&entity], new_id);
}

#[test]
fn removed_binding_component_is_removed_from_cache() {
    let mut app = make_app(Duration::from_millis(0));
    let asset_id = make_id(45);
    let entity = app
        .world_mut()
        .spawn(ConfigBind::<ConfigAsset<TestDb>>::with_id(asset_id))
        .id();
    app.update();

    app.world_mut()
        .entity_mut(entity)
        .remove::<ConfigBind<ConfigAsset<TestDb>>>();
    app.update();

    let cache = app
        .world()
        .resource::<HandleEntityCache<ConfigAsset<TestDb>>>();
    assert!(!cache.entity_to_handle.contains_key(&entity));
    assert!(cache.get_entities(&asset_id).is_none());
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
    assert_eq!(captured.asset_id, asset_id.untyped());

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

/// A simple Asset that implements neither `Clone` nor `ConfigDiff`. This
/// verifies that `watch_asset` dispatches zero-copy notifications.
#[derive(Asset, TypePath, Debug, PartialEq, Default)]
struct DummyTexture {
    width: u32,
    height: u32,
}

#[derive(Resource, Default)]
struct CapturedAssetChanged(Option<bevy_assets_hmr::AssetChanged<DummyTexture>>);

#[derive(Resource, Default)]
struct CapturedAssetRemoved(Option<bevy_assets_hmr::AssetRemoved<DummyTexture>>);

fn capture_asset_changed_system(
    mut reader: bevy::ecs::message::MessageReader<bevy_assets_hmr::AssetChanged<DummyTexture>>,
    mut captured: ResMut<CapturedAssetChanged>,
) {
    for evt in reader.read() {
        captured.0 = Some(evt.clone());
    }
}

fn capture_asset_removed_system(
    mut reader: bevy::ecs::message::MessageReader<bevy_assets_hmr::AssetRemoved<DummyTexture>>,
    mut captured: ResMut<CapturedAssetRemoved>,
) {
    for evt in reader.read() {
        captured.0 = Some(evt.clone());
    }
}

fn make_watch_app() -> App {
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(1).build());
    let mut app = App::new();
    app.add_plugins((bevy::asset::AssetPlugin::default(), bevy::time::TimePlugin));
    app.add_plugins(ConfigHmrPlugin::default());
    app.setup_hmr_headless();
    app.init_asset::<DummyTexture>();
    app.insert_resource(CapturedAssetChanged::default());
    app.insert_resource(CapturedAssetRemoved::default());
    app.add_systems(
        Update,
        (capture_asset_changed_system, capture_asset_removed_system),
    );
    // watch_asset only registers infrastructure; user loads assets themselves.
    app.watch_asset::<DummyTexture>();
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
        let assets = app.world().resource::<Assets<DummyTexture>>();
        assert_eq!(evt.asset(assets).unwrap().width, 64);
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
    let assets = app.world().resource::<Assets<DummyTexture>>();
    assert_eq!(
        evt.asset(assets).unwrap().width,
        128,
        "AssetChanged should resolve the current asset without cloning it"
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

#[test]
fn watch_asset_removal_dispatches_then_clears_cached_path() {
    use bevy_assets_hmr::AssetBindCache;

    let mut app = make_watch_app();
    let asset_id = AssetId::Uuid {
        uuid: Uuid::from_u128(901),
    };
    app.world_mut()
        .resource_mut::<AssetBindCache<DummyTexture>>()
        .record_path(asset_id, "textures/removed.png".into());

    {
        let mut assets = app.world_mut().resource_mut::<Assets<DummyTexture>>();
        let _ = assets.insert(
            asset_id,
            DummyTexture {
                width: 8,
                height: 8,
            },
        );
    }
    app.update();
    app.world_mut()
        .resource_mut::<Assets<DummyTexture>>()
        .remove(asset_id);
    for _ in 0..3 {
        app.update();
    }

    let removed = app
        .world()
        .resource::<CapturedAssetRemoved>()
        .0
        .as_ref()
        .expect("AssetRemoved should be dispatched before path cleanup");
    assert_eq!(removed.source_path, "textures/removed.png");
    assert!(
        app.world()
            .resource::<AssetBindCache<DummyTexture>>()
            .get_path(&asset_id)
            .is_none()
    );
}

// ===========================================================================
// Asset load failure + ConfigReloadFailed tests
// ===========================================================================

#[derive(Resource, Default)]
struct CapturedReloadFailed(Vec<ConfigReloadFailed<TestDb>>);

fn capture_reload_failed_system(
    mut reader: MessageReader<ConfigReloadFailed<TestDb>>,
    mut captured: ResMut<CapturedReloadFailed>,
) {
    for evt in reader.read() {
        captured.0.push(evt.clone());
    }
}

#[test]
fn asset_load_failure_dispatches_config_reload_failed_and_keeps_old_asset() {
    let mut app = make_app(Duration::from_millis(0));
    app.add_systems(Update, capture_reload_failed_system);
    app.insert_resource(CapturedReloadFailed::default());

    let asset_id = make_id(200);

    // Insert the initial valid asset and let HMR initialize its snapshot.
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

    // Send the same typed failure message emitted by Bevy's AssetServer when
    // ConfigLoader returns an error. The already-loaded asset remains valid.
    app.world_mut()
        .resource_mut::<Messages<AssetLoadFailedEvent<ConfigAsset<TestDb>>>>()
        .write(AssetLoadFailedEvent {
            id: asset_id,
            path: AssetPath::from("data/test.ron"),
            error: AssetLoadError::EmptyPath(AssetPath::from("")),
        });

    for _ in 0..3 {
        app.update();
    }

    let captured = app.world().resource::<CapturedReloadFailed>();
    let evt = captured
        .0
        .iter()
        .find(|event| event.asset_id == asset_id.untyped())
        .expect("ConfigReloadFailed should be dispatched for Bevy load failure");
    assert_eq!(evt.asset_id, asset_id.untyped());
    assert_eq!(evt.source_path, "data/test.ron");
    let current_config = evt
        .current_config
        .as_ref()
        .expect("failed reload should expose the last valid config");
    assert_eq!(
        current_config.items[0].id, "x",
        "current_config should be the last valid snapshot"
    );
    assert_eq!(current_config.items[0].n, 42);
    assert!(
        evt.error.contains("empty path"),
        "the original Bevy load error should be preserved"
    );

    // No artificial reinsert is needed: Bevy retains the last valid asset.
    let assets = app.world().resource::<Assets<ConfigAsset<TestDb>>>();
    let retained = assets
        .get(asset_id)
        .expect("the last valid asset should remain available");
    assert_eq!(retained.raw.items[0].n, 42);
    assert_eq!(retained.source_path, "data/test.ron");
}

#[test]
fn initial_load_failure_dispatches_without_current_config() {
    let mut app = make_app(Duration::from_millis(0));
    app.add_systems(Update, capture_reload_failed_system);
    app.insert_resource(CapturedReloadFailed::default());

    let asset_id = make_id(201);
    app.world_mut()
        .resource_mut::<Messages<AssetLoadFailedEvent<ConfigAsset<TestDb>>>>()
        .write(AssetLoadFailedEvent {
            id: asset_id,
            path: AssetPath::from("data/missing.ron"),
            error: AssetLoadError::EmptyPath(AssetPath::from("")),
        });

    for _ in 0..3 {
        app.update();
    }

    let captured = app.world().resource::<CapturedReloadFailed>();
    let event = captured
        .0
        .iter()
        .find(|event| event.asset_id == asset_id.untyped())
        .expect("initial load failures should also be observable");
    assert_eq!(event.asset_id, asset_id.untyped());
    assert_eq!(event.source_path, "data/missing.ron");
    assert!(event.current_config.is_none());
}

#[test]
fn invalid_ron_asset_emits_config_reload_failed_end_to_end() {
    let mut app = make_app_at(Duration::from_millis(0), "data/invalid_test.ron");
    app.add_systems(Update, capture_reload_failed_system);
    app.insert_resource(CapturedReloadFailed::default());

    for _ in 0..100 {
        app.update();
        if !app.world().resource::<CapturedReloadFailed>().0.is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(1));
    }

    let captured = app.world().resource::<CapturedReloadFailed>();
    let event = captured
        .0
        .iter()
        .find(|event| event.source_path == "data/invalid_test.ron")
        .expect("invalid RON should flow through AssetServer failure handling");
    assert_eq!(event.source_path, "data/invalid_test.ron");
    assert!(event.current_config.is_none());
    assert!(
        event.error.contains("ron parse error"),
        "loader parse error should be preserved, got: {}",
        event.error
    );
}

// ===========================================================================
// Debounce tests: final state must win without dropping later saves
// ===========================================================================

#[test]
fn subsequent_save_within_500ms_reaches_final_state() {
    let mut app = make_app(Duration::from_millis(0));

    let asset_id = make_id(700);
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
                        n: 2,
                    }],
                },
                source_path: "data/test.ron".into(),
            },
        );
    }
    for _ in 0..3 {
        app.update();
    }

    assert_eq!(
        app.world()
            .resource::<CapturedRefresh>()
            .0
            .as_ref()
            .expect("first modification should dispatch")
            .new_config
            .items[0]
            .n,
        2
    );

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
                        n: 3,
                    }],
                },
                source_path: "data/test.ron".into(),
            },
        );
    }
    app.world_mut().resource_mut::<CapturedRefresh>().0 = None;

    for _ in 0..5 {
        app.update();
    }

    let captured = app.world().resource::<CapturedRefresh>();
    assert_eq!(
        captured
            .0
            .as_ref()
            .expect("a second save within 500ms must not be dropped")
            .new_config
            .items[0]
            .n,
        3
    );
    let snapshots = app.world().resource::<LastSnapshot<ConfigAsset<TestDb>>>();
    assert_eq!(snapshots.map[&asset_id].items[0].n, 3);
}

#[test]
fn removed_then_added_in_same_window_refreshes_final_asset() {
    let mut app = make_app_with_removed_capture(Duration::from_millis(0));

    let asset_id = make_id(800);
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

    app.world_mut().resource_mut::<CapturedRefresh>().0 = None;

    {
        let mut assets = app
            .world_mut()
            .resource_mut::<Assets<ConfigAsset<TestDb>>>();
        assets.remove(asset_id);
        let _ = assets.insert(
            asset_id,
            ConfigAsset {
                raw: TestDb {
                    items: vec![Item {
                        id: "a".into(),
                        n: 3,
                    }],
                },
                source_path: "data/test.ron".into(),
            },
        );
    }
    for _ in 0..5 {
        app.update();
    }

    let captured = app.world().resource::<CapturedRefresh>();
    assert_eq!(
        captured
            .0
            .as_ref()
            .expect("Removed followed by Added should refresh the final asset")
            .new_config
            .items[0]
            .n,
        3
    );
    assert!(
        app.world().resource::<CapturedRemoved>().0.is_none(),
        "the superseded Removed event must not be dispatched"
    );
    let snapshots = app.world().resource::<LastSnapshot<ConfigAsset<TestDb>>>();
    assert_eq!(snapshots.map[&asset_id].items[0].n, 3);
}

#[test]
fn modified_then_removed_in_same_window_dispatches_only_removed() {
    let mut app = make_app_with_removed_capture(Duration::from_millis(0));
    let asset_id = make_id(801);

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
                        n: 2,
                    }],
                },
                source_path: "data/test.ron".into(),
            },
        );
        assets.remove(asset_id);
    }
    app.world_mut().resource_mut::<CapturedRefresh>().0 = None;

    for _ in 0..3 {
        app.update();
    }

    assert!(
        app.world().resource::<CapturedRefresh>().0.is_none(),
        "the superseded Modified event must not be dispatched"
    );
    assert!(
        app.world().resource::<CapturedRemoved>().0.is_some(),
        "a final Removed event must be dispatched"
    );
    let snapshots = app.world().resource::<LastSnapshot<ConfigAsset<TestDb>>>();
    assert!(!snapshots.map.contains_key(&asset_id));
}

#[test]
fn cache_validation_system_corrects_drift() {
    let mut app = make_app(Duration::from_millis(50));

    let asset_id = make_id(900);
    let entity = app
        .world_mut()
        .spawn(ConfigBind::<ConfigAsset<TestDb>>::with_id(asset_id))
        .id();

    for _ in 0..3 {
        app.update();
    }

    {
        let cache = app
            .world()
            .resource::<HandleEntityCache<ConfigAsset<TestDb>>>();
        assert!(cache.get_entities(&asset_id).is_some());
    }

    let wrong_id = make_id(901);
    {
        let mut cache = app
            .world_mut()
            .resource_mut::<HandleEntityCache<ConfigAsset<TestDb>>>();
        cache.remove(entity);
        cache.insert(entity, wrong_id);
        assert_eq!(cache.entity_to_handle.len(), 1);
        assert_eq!(cache.entity_to_handle[&entity], wrong_id);
    }

    // Run the validator directly so the test does not wait for its normal
    // 30-second safety-net timer.
    app.add_systems(
        Update,
        bevy_assets_hmr::cache_validation_system::<ConfigAsset<TestDb>>,
    );
    app.update();

    let cache = app
        .world()
        .resource::<HandleEntityCache<ConfigAsset<TestDb>>>();
    assert_eq!(cache.entity_to_handle[&entity], asset_id);
    assert!(cache.get_entities(&wrong_id).is_none());
}

// ===========================================================================
// Dependency chain (cascade) tests
// ===========================================================================

/// A child config: simple String-keyed table. No dependencies.
///
/// We implement HmrSource manually instead of going through HmrAsset
/// (which requires DeserializeOwned) because this test inserts assets
/// directly via Assets::insert and doesn't need serde.
#[derive(Asset, TypePath, Clone, Debug, PartialEq, Default, ConfigDiff)]
#[config_diff(field = "items", id = "id")]
struct ChildDb {
    items: Vec<ChildItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ChildItem {
    id: String,
    v: u32,
}

impl HmrSource for ChildDb {
    type Config = ChildDb;
    fn config(&self) -> &ChildDb {
        self
    }
    fn source_path(&self) -> &str {
        ""
    }
}

/// A parent config that holds a Handle<ConfigAsset<ChildDb>>. The
/// #[derive(Asset)] auto-generates VisitAssetDependencies which walks
/// Handle<*> fields and feeds them to the dependency graph.
#[derive(Asset, TypePath, Clone, Debug, PartialEq, Default, ConfigDiff)]
#[config_diff(field = "items", id = "id")]
struct ParentDb {
    items: Vec<ParentItem>,
    /// `#[dependency]` marks this `Handle<ChildDb>` field as an asset
    /// dependency for the `VisitAssetDependencies` derive macro.
    /// Without it, `ParentDb::visit_dependencies` would not visit this
    /// handle, and the dependency graph would miss the edge.
    #[dependency]
    child_handle: Handle<ChildDb>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ParentItem {
    id: String,
}

impl HmrSource for ParentDb {
    type Config = ParentDb;
    fn config(&self) -> &ParentDb {
        self
    }
    fn source_path(&self) -> &str {
        ""
    }
}

fn make_cascade_app(debounce_window: Duration) -> App {
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(1).build());
    let mut app = App::new();
    app.add_plugins((bevy::asset::AssetPlugin::default(), bevy::time::TimePlugin));
    app.add_plugins(ConfigHmrPlugin { debounce_window });
    app.setup_hmr_headless();
    app.init_asset::<ChildDb>();
    app.init_asset::<ParentDb>();
    app.register_asset::<ChildDb>("data/child.ron");
    app.register_asset::<ParentDb>("data/parent.ron");
    app.insert_resource(CapturedRefresh::default());
    app
}

#[derive(Resource, Default)]
struct CapturedParentRefresh(Option<ConfigRefresh<ParentDb>>);

/// Capture ConfigRefresh<ParentDb> events.
fn capture_parent_refresh_system(
    mut reader: MessageReader<ConfigRefresh<ParentDb>>,
    mut captured: ResMut<CapturedParentRefresh>,
) {
    for refresh in reader.read() {
        captured.0 = Some(refresh.clone());
    }
}

/// Helper: insert a child asset into the world and return its strong
/// handle. The returned handle's `AssetId` is used as the dependency edge
/// key in `DependencyGraph`.
fn insert_child_and_handle(app: &mut App, child: ChildDb) -> Handle<ChildDb> {
    app.world_mut().resource_mut::<Assets<ChildDb>>().add(child)
}

#[test]
fn dependency_graph_is_populated_on_parent_load() {
    use bevy_assets_hmr::DependencyGraph;

    let mut app = make_cascade_app(Duration::from_millis(0));
    app.insert_resource(CapturedParentRefresh::default());
    app.add_systems(Update, capture_parent_refresh_system);

    let _child_id = AssetId::<ChildDb>::Uuid {
        uuid: Uuid::from_u128(1100),
    };
    let child_handle = insert_child_and_handle(
        &mut app,
        ChildDb {
            items: vec![ChildItem {
                id: "c1".into(),
                v: 1,
            }],
        },
    );
    let actual_child_id = child_handle.id();
    let parent_id = AssetId::<ParentDb>::Uuid {
        uuid: Uuid::from_u128(1101),
    };
    {
        let mut assets = app.world_mut().resource_mut::<Assets<ParentDb>>();
        let _ = assets.insert(
            parent_id,
            ParentDb {
                items: vec![ParentItem { id: "p1".into() }],
                child_handle: child_handle.clone(),
            },
        );
    }

    for _ in 0..10 {
        app.update();
    }

    let graph = app.world().resource::<DependencyGraph>();
    let parents = graph.parents_of(actual_child_id.untyped());
    assert!(
        !parents.is_empty(),
        "DependencyGraph should record child -> parent edge"
    );
    let has_parent = parents.iter().any(|(p, _)| *p == parent_id.untyped());
    assert!(has_parent, "parent asset id should appear in parents_of");
}

#[test]
fn cascade_triggers_config_refresh_on_parent() {
    let mut app = make_cascade_app(Duration::from_millis(0));
    app.insert_resource(CapturedParentRefresh::default());
    app.add_systems(Update, capture_parent_refresh_system);

    let _child_id = AssetId::<ChildDb>::Uuid {
        uuid: Uuid::from_u128(1200),
    };
    let child_handle = insert_child_and_handle(
        &mut app,
        ChildDb {
            items: vec![ChildItem {
                id: "c1".into(),
                v: 1,
            }],
        },
    );
    let actual_child_id = child_handle.id();
    let parent_id = AssetId::<ParentDb>::Uuid {
        uuid: Uuid::from_u128(1201),
    };
    {
        let mut parent_assets = app.world_mut().resource_mut::<Assets<ParentDb>>();
        let _ = parent_assets.insert(
            parent_id,
            ParentDb {
                items: vec![ParentItem { id: "p1".into() }],
                child_handle: child_handle.clone(),
            },
        );
    }

    for _ in 0..5 {
        app.update();
    }
    app.world_mut().resource_mut::<CapturedParentRefresh>().0 = None;

    {
        let mut child_assets = app.world_mut().resource_mut::<Assets<ChildDb>>();
        let _ = child_assets.insert(
            actual_child_id,
            ChildDb {
                items: vec![ChildItem {
                    id: "c1".into(),
                    v: 999,
                }],
            },
        );
    }

    for _ in 0..10 {
        app.update();
    }

    let captured = app.world().resource::<CapturedParentRefresh>();
    let evt = captured
        .0
        .as_ref()
        .expect("cascade should have fired ConfigRefresh<ParentDb>");
    assert!(
        evt.changed_ids.is_empty(),
        "cascade-fired ConfigRefresh should have empty changed_ids"
    );
    assert_eq!(evt.asset_id, parent_id.untyped());
    match &evt.cause {
        RefreshCause::Dependency { triggered_by } => {
            assert_eq!(
                triggered_by,
                &[actual_child_id.untyped()].into_iter().collect()
            );
        }
        cause => panic!("expected dependency cause, got {cause:?}"),
    }
    // Direct-mode HmrSource returns "" for source_path, so the cascade
    // event also has an empty path. The important part is that the event
    // fired with the parent's current value.
    assert!(
        evt.source_path.is_empty(),
        "direct-mode source_path should be empty (no override)"
    );
}

#[test]
fn cascade_does_not_fire_for_unrelated_types() {
    use bevy_assets_hmr::DependencyGraph;

    let mut app = make_cascade_app(Duration::from_millis(0));
    app.insert_resource(CapturedParentRefresh::default());
    app.add_systems(Update, capture_parent_refresh_system);

    let parent_id = AssetId::<ParentDb>::Uuid {
        uuid: Uuid::from_u128(1300),
    };
    {
        let mut parent_assets = app.world_mut().resource_mut::<Assets<ParentDb>>();
        let _ = parent_assets.insert(
            parent_id,
            ParentDb {
                items: vec![ParentItem { id: "p1".into() }],
                child_handle: Handle::default(),
            },
        );
    }

    let unrelated_child_id = AssetId::<ChildDb>::Uuid {
        uuid: Uuid::from_u128(1301),
    };
    {
        let mut child_assets = app.world_mut().resource_mut::<Assets<ChildDb>>();
        let _ = child_assets.insert(
            unrelated_child_id,
            ChildDb {
                items: vec![ChildItem {
                    id: "uc1".into(),
                    v: 1,
                }],
            },
        );
    }

    for _ in 0..5 {
        app.update();
    }
    app.world_mut().resource_mut::<CapturedParentRefresh>().0 = None;

    {
        let mut child_assets = app.world_mut().resource_mut::<Assets<ChildDb>>();
        let _ = child_assets.insert(
            unrelated_child_id,
            ChildDb {
                items: vec![ChildItem {
                    id: "uc1".into(),
                    v: 2,
                }],
            },
        );
    }

    for _ in 0..10 {
        app.update();
    }

    let graph = app.world().resource::<DependencyGraph>();
    let parents = graph.parents_of(unrelated_child_id.untyped());
    assert!(
        parents.is_empty(),
        "unrelated child should have no parents in the graph"
    );

    let captured = app.world().resource::<CapturedParentRefresh>();
    assert!(
        captured.0.is_none(),
        "no cascade should fire for unrelated types"
    );
}

/// Verify that removing the child asset triggers a cascade
/// `ConfigRefresh<ParentDb>` (with empty `changed_ids`), so the parent
/// subscriber can clean up any derived state.
#[test]
fn cascade_triggers_on_child_removed() {
    let mut app = make_cascade_app(Duration::from_millis(0));
    app.insert_resource(CapturedParentRefresh::default());
    app.add_systems(Update, capture_parent_refresh_system);

    let child_handle = insert_child_and_handle(
        &mut app,
        ChildDb {
            items: vec![ChildItem {
                id: "c1".into(),
                v: 1,
            }],
        },
    );
    let actual_child_id = child_handle.id();
    let parent_id = AssetId::<ParentDb>::Uuid {
        uuid: Uuid::from_u128(1401),
    };
    {
        let mut parent_assets = app.world_mut().resource_mut::<Assets<ParentDb>>();
        let _ = parent_assets.insert(
            parent_id,
            ParentDb {
                items: vec![ParentItem { id: "p1".into() }],
                child_handle: child_handle.clone(),
            },
        );
    }

    // Let dependency graph build.
    for _ in 0..10 {
        app.update();
    }
    app.world_mut().resource_mut::<CapturedParentRefresh>().0 = None;

    // Remove the child asset - this should cascade a refresh on the
    // parent on the next frame.
    {
        let mut child_assets = app.world_mut().resource_mut::<Assets<ChildDb>>();
        child_assets.remove(actual_child_id);
    }

    for _ in 0..10 {
        app.update();
    }

    let captured = app.world().resource::<CapturedParentRefresh>();
    let evt = captured
        .0
        .as_ref()
        .expect("cascade should fire ConfigRefresh<ParentDb> after child removal");
    assert!(
        evt.changed_ids.is_empty(),
        "removal cascade should also have empty changed_ids"
    );
    assert_eq!(evt.asset_id, parent_id.untyped());
    match &evt.cause {
        RefreshCause::Dependency { triggered_by } => {
            assert_eq!(
                triggered_by,
                &[actual_child_id.untyped()].into_iter().collect()
            );
        }
        cause => panic!("expected dependency cause, got {cause:?}"),
    }
}

#[test]
fn cascade_triggers_on_child_load_failure() {
    let mut app = make_cascade_app(Duration::from_millis(0));
    app.insert_resource(CapturedParentRefresh::default());
    app.add_systems(Update, capture_parent_refresh_system);

    let child_handle = insert_child_and_handle(
        &mut app,
        ChildDb {
            items: vec![ChildItem {
                id: "c1".into(),
                v: 1,
            }],
        },
    );
    let actual_child_id = child_handle.id();
    let parent_id = AssetId::<ParentDb>::Uuid {
        uuid: Uuid::from_u128(1501),
    };
    {
        let mut parent_assets = app.world_mut().resource_mut::<Assets<ParentDb>>();
        let _ = parent_assets.insert(
            parent_id,
            ParentDb {
                items: vec![ParentItem { id: "p1".into() }],
                child_handle: child_handle.clone(),
            },
        );
    }

    // Build the dependency graph and initialize the child snapshot first.
    for _ in 0..10 {
        app.update();
    }
    app.world_mut().resource_mut::<CapturedParentRefresh>().0 = None;

    app.world_mut()
        .resource_mut::<Messages<AssetLoadFailedEvent<ChildDb>>>()
        .write(AssetLoadFailedEvent {
            id: actual_child_id,
            path: AssetPath::from("data/child.ron"),
            error: AssetLoadError::EmptyPath(AssetPath::from("")),
        });
    for _ in 0..10 {
        app.update();
    }

    let captured = app.world().resource::<CapturedParentRefresh>();
    let evt = captured
        .0
        .as_ref()
        .expect("child load failure should cascade to the parent");
    assert_eq!(evt.asset_id, parent_id.untyped());
    match &evt.cause {
        RefreshCause::Dependency { triggered_by } => {
            assert_eq!(
                triggered_by,
                &[actual_child_id.untyped()].into_iter().collect()
            );
        }
        cause => panic!("expected dependency cause, got {cause:?}"),
    }

    app.world_mut().resource_mut::<CapturedParentRefresh>().0 = None;
    let _ = app.world_mut().resource_mut::<Assets<ChildDb>>().insert(
        actual_child_id,
        ChildDb {
            items: vec![ChildItem {
                id: "c1".into(),
                v: 1,
            }],
        },
    );
    for _ in 0..10 {
        app.update();
    }
    assert!(
        app.world().resource::<CapturedParentRefresh>().0.is_some(),
        "child recovery should also cascade to the parent"
    );
}
