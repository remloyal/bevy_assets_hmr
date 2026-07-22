use bevy::asset::{Asset, AssetId, AssetLoadError, AssetLoadFailedEvent, AssetPath, Assets};
use bevy::ecs::message::{MessageReader, Messages};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{
    ActiveConfigView, AppliedRevision, AssetRevision, AssetRevisionStatus, ConfigAsset, ConfigBind,
    ConfigDiff, ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh, ConfigViewSync, RefreshCause,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default, ConfigDiff)]
#[config_diff(field = "items", id = "id")]
struct ViewDb {
    items: Vec<ViewItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
struct ViewItem {
    id: String,
    value: u32,
}

type ViewAsset = ConfigAsset<ViewDb>;

#[derive(Resource, Default)]
struct CapturedSyncs(Vec<ConfigViewSync<ViewAsset>>);

#[derive(Resource, Default)]
struct CapturedRefreshes(Vec<ConfigRefresh<ViewDb>>);

fn capture_syncs(
    mut reader: MessageReader<ConfigViewSync<ViewAsset>>,
    mut captured: ResMut<CapturedSyncs>,
) {
    captured.0.extend(reader.read().cloned());
}

fn capture_refreshes(
    mut reader: MessageReader<ConfigRefresh<ViewDb>>,
    mut captured: ResMut<CapturedRefreshes>,
) {
    captured.0.extend(reader.read().cloned());
}

fn make_app() -> App {
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(1).build());
    let mut app = App::new();
    app.add_plugins((
        bevy::app::TaskPoolPlugin::default(),
        bevy::asset::AssetPlugin::default(),
        bevy::time::TimePlugin,
    ));
    app.add_plugins(ConfigHmrPlugin {
        debounce_window: Duration::ZERO,
    });
    app.setup_hmr_headless();
    bevy_assets_hmr::ext::register_config_impl::<ViewDb>(&mut app, "data/view.ron", false);
    app.insert_resource(CapturedSyncs::default());
    app.insert_resource(CapturedRefreshes::default());
    app.add_systems(Update, (capture_syncs, capture_refreshes));
    app
}

fn make_id(seed: u128) -> AssetId<ViewAsset> {
    AssetId::Uuid {
        uuid: Uuid::from_u128(seed),
    }
}

fn insert_value(app: &mut App, id: AssetId<ViewAsset>, value: u32) {
    let _ = app.world_mut().resource_mut::<Assets<ViewAsset>>().insert(
        id,
        ConfigAsset {
            raw: ViewDb {
                items: vec![ViewItem {
                    id: "entry".into(),
                    value,
                }],
            },
            source_path: "data/view.ron".into(),
        },
    );
}

fn spawn_view(app: &mut App, id: AssetId<ViewAsset>, active: bool) -> Entity {
    let bind = ConfigBind::<ViewAsset>::with_id(id);
    let applied = AppliedRevision::new(bind.handle.clone());
    let mut entity = app.world_mut().spawn((bind, applied));
    if active {
        entity.insert(ActiveConfigView);
    }
    entity.id()
}

fn update_frames(app: &mut App, frames: usize) {
    for _ in 0..frames {
        app.update();
    }
}

#[test]
fn active_view_receives_immediate_sync() {
    let mut app = make_app();
    let id = make_id(1);
    let entity = spawn_view(&mut app, id, true);
    insert_value(&mut app, id, 1);
    update_frames(&mut app, 3);
    app.world_mut().resource_mut::<CapturedSyncs>().0.clear();

    insert_value(&mut app, id, 2);
    update_frames(&mut app, 3);

    let syncs = &app.world().resource::<CapturedSyncs>().0;
    assert_eq!(syncs.len(), 1);
    assert_eq!(syncs[0].entity, entity);
    assert_eq!(syncs[0].asset_id, id);
    assert_eq!(syncs[0].status, AssetRevisionStatus::Available);
    let current = app.world().resource::<AssetRevision<ViewAsset>>();
    assert_eq!(syncs[0].revision, current.revision(id).unwrap());
}

#[test]
fn hidden_view_coalesces_multiple_updates_until_activation() {
    let mut app = make_app();
    let id = make_id(2);
    let entity = spawn_view(&mut app, id, false);
    insert_value(&mut app, id, 1);
    update_frames(&mut app, 3);
    app.world_mut().resource_mut::<CapturedSyncs>().0.clear();

    for value in [2, 3, 4] {
        insert_value(&mut app, id, value);
        update_frames(&mut app, 3);
    }
    assert!(app.world().resource::<CapturedSyncs>().0.is_empty());
    assert_eq!(
        app.world().resource::<CapturedRefreshes>().0.len(),
        3,
        "global config consumers still receive updates while the view is hidden"
    );
    assert_eq!(
        app.world()
            .get::<AppliedRevision<ViewAsset>>(entity)
            .unwrap()
            .revision,
        0
    );

    app.world_mut().entity_mut(entity).insert(ActiveConfigView);
    update_frames(&mut app, 2);

    let syncs = &app.world().resource::<CapturedSyncs>().0;
    assert_eq!(syncs.len(), 1);
    let assets = app.world().resource::<Assets<ViewAsset>>();
    assert_eq!(assets.get(id).unwrap().raw.items[0].value, 4);
    assert_eq!(
        syncs[0].revision,
        app.world()
            .resource::<AssetRevision<ViewAsset>>()
            .revision(id)
            .unwrap()
    );
}

#[test]
fn shared_asset_only_syncs_active_view_until_hidden_view_activates() {
    let mut app = make_app();
    let id = make_id(3);
    let active = spawn_view(&mut app, id, true);
    let hidden = spawn_view(&mut app, id, false);
    insert_value(&mut app, id, 1);
    update_frames(&mut app, 3);
    app.world_mut().resource_mut::<CapturedSyncs>().0.clear();

    insert_value(&mut app, id, 2);
    update_frames(&mut app, 3);
    assert_eq!(
        app.world()
            .resource::<CapturedSyncs>()
            .0
            .iter()
            .map(|sync| sync.entity)
            .collect::<Vec<_>>(),
        vec![active]
    );

    app.world_mut().resource_mut::<CapturedSyncs>().0.clear();
    app.world_mut().entity_mut(hidden).insert(ActiveConfigView);
    update_frames(&mut app, 2);
    let syncs = &app.world().resource::<CapturedSyncs>().0;
    assert_eq!(syncs.len(), 1);
    assert_eq!(syncs[0].entity, hidden);
}

#[test]
fn hidden_view_observes_removed_state_when_activated() {
    let mut app = make_app();
    let id = make_id(4);
    let entity = spawn_view(&mut app, id, false);
    insert_value(&mut app, id, 1);
    update_frames(&mut app, 3);

    app.world_mut()
        .resource_mut::<Assets<ViewAsset>>()
        .remove(id);
    update_frames(&mut app, 3);
    assert!(app.world().resource::<CapturedSyncs>().0.is_empty());

    app.world_mut().entity_mut(entity).insert(ActiveConfigView);
    update_frames(&mut app, 2);
    let syncs = &app.world().resource::<CapturedSyncs>().0;
    assert_eq!(syncs.len(), 1);
    assert_eq!(syncs[0].status, AssetRevisionStatus::Removed);
}

#[test]
fn hidden_view_observes_failure_and_last_valid_asset_when_activated() {
    let mut app = make_app();
    let id = make_id(6);
    let entity = spawn_view(&mut app, id, false);
    insert_value(&mut app, id, 7);
    update_frames(&mut app, 3);

    app.world_mut()
        .resource_mut::<Messages<AssetLoadFailedEvent<ViewAsset>>>()
        .write(AssetLoadFailedEvent {
            id,
            path: AssetPath::from("data/view.ron"),
            error: AssetLoadError::EmptyPath(AssetPath::from("")),
        });
    update_frames(&mut app, 2);
    assert!(app.world().resource::<CapturedSyncs>().0.is_empty());

    app.world_mut().entity_mut(entity).insert(ActiveConfigView);
    update_frames(&mut app, 2);
    let syncs = &app.world().resource::<CapturedSyncs>().0;
    assert_eq!(syncs.len(), 1);
    assert!(matches!(
        syncs[0].status,
        AssetRevisionStatus::LoadFailed { .. }
    ));
    assert_eq!(
        app.world()
            .resource::<Assets<ViewAsset>>()
            .get(id)
            .unwrap()
            .raw
            .items[0]
            .value,
        7
    );
}

#[test]
fn failed_reload_then_same_content_dispatches_recovery() {
    let mut app = make_app();
    let id = make_id(5);
    let entity = spawn_view(&mut app, id, true);
    insert_value(&mut app, id, 1);
    update_frames(&mut app, 3);
    app.world_mut().resource_mut::<CapturedSyncs>().0.clear();
    app.world_mut()
        .resource_mut::<CapturedRefreshes>()
        .0
        .clear();

    app.world_mut()
        .resource_mut::<Messages<AssetLoadFailedEvent<ViewAsset>>>()
        .write(AssetLoadFailedEvent {
            id,
            path: AssetPath::from("data/view.ron"),
            error: AssetLoadError::EmptyPath(AssetPath::from("")),
        });
    update_frames(&mut app, 2);
    assert!(matches!(
        app.world()
            .resource::<CapturedSyncs>()
            .0
            .last()
            .unwrap()
            .status,
        AssetRevisionStatus::LoadFailed { .. }
    ));
    assert_eq!(
        app.world()
            .resource::<Assets<ViewAsset>>()
            .get(id)
            .unwrap()
            .raw
            .items[0]
            .value,
        1
    );

    app.world_mut().resource_mut::<CapturedSyncs>().0.clear();
    insert_value(&mut app, id, 1);
    update_frames(&mut app, 3);

    let refreshes = &app.world().resource::<CapturedRefreshes>().0;
    assert_eq!(refreshes.len(), 1);
    assert_eq!(refreshes[0].cause, RefreshCause::Recovery);
    assert!(refreshes[0].delta.is_empty());
    let syncs = &app.world().resource::<CapturedSyncs>().0;
    assert_eq!(syncs.len(), 1);
    assert_eq!(syncs[0].entity, entity);
    assert_eq!(syncs[0].status, AssetRevisionStatus::Available);
}
