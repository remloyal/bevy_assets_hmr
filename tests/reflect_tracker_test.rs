#![cfg(feature = "reflect_auto_tracking")]

use bevy::asset::{Asset, AssetId, AssetPlugin, Assets, ReflectHandle};
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::reflect::{Reflect, TypeRegistry, map::DynamicMap, set::DynamicSet};
use bevy::scene::{ScenePlugin, WorldSceneExt};
use bevy_assets_hmr::{
    AssetBind, AssetChanged, ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh, HmrSource,
    ReflectHandleCache, ReflectHandleTrackingMetrics, ReflectHandleTrackingPlugin,
    SimpleConfigDiff, extract_reflected_handles,
};
use std::marker::PhantomData;
use uuid::Uuid;

#[derive(Asset, Reflect, Clone, Debug, Default, PartialEq)]
struct TrackedAsset {
    value: u32,
}

impl SimpleConfigDiff for TrackedAsset {}

impl HmrSource for TrackedAsset {
    type Config = Self;

    fn config(&self) -> &Self::Config {
        self
    }
}

#[derive(Component, Reflect, Clone)]
#[reflect(Component)]
struct HandleComponent {
    direct: Handle<TrackedAsset>,
    optional: Option<Handle<TrackedAsset>>,
    list: Vec<Handle<TrackedAsset>>,
}

#[derive(Component, Reflect, Clone)]
#[reflect(Component)]
struct SecondaryHandleComponent(Handle<TrackedAsset>);

#[derive(Component, Reflect, Clone)]
struct UnregisteredHandleComponent(Handle<TrackedAsset>);

#[derive(Component, Reflect, FromTemplate, Clone)]
#[reflect(Component)]
struct BsnHandleComponent {
    handle: Handle<TrackedAsset>,
}

#[derive(Reflect)]
enum NestedVariant {
    Tuple(Handle<TrackedAsset>, Option<Handle<TrackedAsset>>),
    Struct { handle: Handle<TrackedAsset> },
}

#[derive(Reflect)]
struct NestedValue {
    direct: Handle<TrackedAsset>,
    optional: Option<Handle<TrackedAsset>>,
    list: Vec<Handle<TrackedAsset>>,
    array: [Handle<TrackedAsset>; 1],
    tuple: (Handle<TrackedAsset>, Option<Handle<TrackedAsset>>),
    variant: NestedVariant,
}

#[derive(Reflect)]
struct NestedTupleStruct(Handle<TrackedAsset>, Option<Handle<TrackedAsset>>);

#[derive(Resource, Default)]
struct CapturedChanges(Vec<AssetChanged<TrackedAsset>>);

#[derive(Resource, Default)]
struct CapturedRefreshes(Vec<ConfigRefresh<TrackedAsset>>);

fn capture_changes(
    mut reader: MessageReader<AssetChanged<TrackedAsset>>,
    mut captured: ResMut<CapturedChanges>,
) {
    captured.0.extend(reader.read().cloned());
}

fn capture_refreshes(
    mut reader: MessageReader<ConfigRefresh<TrackedAsset>>,
    mut captured: ResMut<CapturedRefreshes>,
) {
    captured.0.extend(reader.read().cloned());
}

fn handle(uuid: u128) -> Handle<TrackedAsset> {
    Handle::Uuid(Uuid::from_u128(uuid), PhantomData)
}

fn reflected_app() -> App {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<TrackedAsset>()
        .register_asset_reflect::<TrackedAsset>()
        .register_type::<HandleComponent>()
        .register_type::<SecondaryHandleComponent>()
        .add_plugins(ReflectHandleTrackingPlugin::default());
    app
}

fn reflected_scene_app() -> App {
    let mut app = reflected_app();
    app.add_plugins(ScenePlugin);
    app.register_type::<BsnHandleComponent>();
    app
}

#[test]
fn extractor_walks_nested_containers_and_deduplicates() {
    let first = handle(1);
    let second = handle(2);
    let mut registry = TypeRegistry::default();
    registry.register::<Handle<TrackedAsset>>();
    registry.register_type_data::<Handle<TrackedAsset>, ReflectHandle>();

    let value = NestedValue {
        direct: first.clone(),
        optional: Some(second.clone()),
        list: vec![first.clone(), second.clone()],
        array: [first.clone()],
        tuple: (second.clone(), Some(first.clone())),
        variant: NestedVariant::Struct {
            handle: second.clone(),
        },
    };
    let extraction = extract_reflected_handles(&value, &registry, 32);
    assert_eq!(extraction.handles.len(), 2);
    assert!(extraction.handles.contains(&first.id().untyped()));
    assert!(extraction.handles.contains(&second.id().untyped()));
    assert!(!extraction.max_depth_reached);

    let mut map = DynamicMap::default();
    map.insert("first".to_string(), first.clone());
    map.insert("second".to_string(), second.clone());
    let map_result = extract_reflected_handles(&map, &registry, 32);
    assert_eq!(map_result.handles.len(), 2);

    let mut set = DynamicSet::default();
    set.insert(first.clone());
    set.insert(second.clone());
    let set_result = extract_reflected_handles(&set, &registry, 32);
    assert_eq!(set_result.handles.len(), 2);

    let tuple_struct = NestedTupleStruct(first.clone(), Some(second.clone()));
    let tuple_struct_result = extract_reflected_handles(&tuple_struct, &registry, 32);
    assert_eq!(tuple_struct_result.handles.len(), 2);

    let tuple_variant = NestedVariant::Tuple(first.clone(), Some(second.clone()));
    let tuple_variant_result = extract_reflected_handles(&tuple_variant, &registry, 32);
    assert_eq!(tuple_variant_result.handles.len(), 2);

    let mut handle_map = DynamicMap::default();
    handle_map.insert(first.clone(), second.clone());
    let handle_map_result = extract_reflected_handles(&handle_map, &registry, 32);
    assert_eq!(handle_map_result.handles.len(), 2);
}

#[test]
fn extractor_obeys_depth_limit() {
    let first = handle(3);
    let mut registry = TypeRegistry::default();
    registry.register::<Handle<TrackedAsset>>();
    registry.register_type_data::<Handle<TrackedAsset>, ReflectHandle>();

    let value = NestedValue {
        direct: first,
        optional: None,
        list: Vec::new(),
        array: [handle(4)],
        tuple: (handle(5), None),
        variant: NestedVariant::Tuple(handle(6), None),
    };
    let extraction = extract_reflected_handles(&value, &registry, 0);
    assert!(extraction.max_depth_reached);
    assert!(extraction.handles.is_empty());
}

#[test]
fn reflected_component_index_updates_and_cleans_up() {
    let first = handle(10);
    let second = handle(11);
    let mut app = reflected_app();
    let entity = app
        .world_mut()
        .spawn(HandleComponent {
            direct: first.clone(),
            optional: Some(second.clone()),
            list: vec![first.clone()],
        })
        .id();

    app.update();
    let cache = app.world().resource::<ReflectHandleCache>();
    assert!(
        cache
            .entities_for(first.id().untyped())
            .is_some_and(|entities| entities.contains(&entity))
    );
    assert!(
        cache
            .entities_for(second.id().untyped())
            .is_some_and(|entities| entities.contains(&entity))
    );

    app.world_mut().entity_mut(entity).insert(HandleComponent {
        direct: second.clone(),
        optional: None,
        list: Vec::new(),
    });
    app.update();

    let cache = app.world().resource::<ReflectHandleCache>();
    assert!(
        !cache
            .entities_for(first.id().untyped())
            .is_some_and(|entities| entities.contains(&entity))
    );
    assert!(
        cache
            .entities_for(second.id().untyped())
            .is_some_and(|entities| entities.contains(&entity))
    );

    app.world_mut()
        .entity_mut(entity)
        .remove::<HandleComponent>();
    app.update();
    let cache = app.world().resource::<ReflectHandleCache>();
    assert!(cache.assets_for(entity).is_none());
    assert!(cache.entities_for(second.id().untyped()).is_none());

    app.world_mut().despawn(entity);
    app.update();
    let cache = app.world().resource::<ReflectHandleCache>();
    assert!(cache.entities_for(second.id().untyped()).is_none());
    assert!(cache.assets_for(entity).is_none());

    let replacement = app
        .world_mut()
        .spawn(HandleComponent {
            direct: second.clone(),
            optional: None,
            list: Vec::new(),
        })
        .id();
    app.update();
    let cache = app.world().resource::<ReflectHandleCache>();
    assert!(
        cache
            .entities_for(second.id().untyped())
            .is_some_and(|entities| entities.contains(&replacement))
    );

    let metrics = app.world().resource::<ReflectHandleTrackingMetrics>();
    assert!(metrics.components_reflected >= 3);
    assert!(metrics.entities_removed >= 1);
}

#[test]
fn tracker_keeps_multiple_component_asset_edges_independent() {
    let first = handle(12);
    let second = handle(13);
    let mut app = reflected_app();
    let entity = app
        .world_mut()
        .spawn((
            HandleComponent {
                direct: first.clone(),
                optional: None,
                list: Vec::new(),
            },
            SecondaryHandleComponent(second.clone()),
        ))
        .id();

    app.update();
    let cache = app.world().resource::<ReflectHandleCache>();
    assert_eq!(cache.assets_for(entity).map(|assets| assets.len()), Some(2));

    app.world_mut()
        .entity_mut(entity)
        .remove::<HandleComponent>();
    app.update();
    let cache = app.world().resource::<ReflectHandleCache>();
    assert!(cache.entities_for(first.id().untyped()).is_none());
    assert!(
        cache
            .entities_for(second.id().untyped())
            .is_some_and(|entities| entities.contains(&entity))
    );
}

#[test]
fn tracker_skips_component_without_reflect_component_registration() {
    let asset = handle(14);
    let mut app = reflected_app();
    let entity = app
        .world_mut()
        .spawn(UnregisteredHandleComponent(asset.clone()))
        .id();

    app.update();

    let cache = app.world().resource::<ReflectHandleCache>();
    assert!(cache.assets_for(entity).is_none());
    assert!(cache.entities_for(asset.id().untyped()).is_none());
    assert!(
        app.world()
            .resource::<ReflectHandleTrackingMetrics>()
            .missing_component_types
            >= 1
    );
}

#[test]
fn asset_changed_merges_reflected_and_explicit_targets_once() {
    let mut app = reflected_app();
    app.watch_asset::<TrackedAsset>()
        .setup_hmr_headless()
        .init_resource::<CapturedChanges>()
        .add_systems(Update, capture_changes);

    let handle = app
        .world_mut()
        .resource_mut::<Assets<TrackedAsset>>()
        .add(TrackedAsset { value: 1 });
    let entity = app
        .world_mut()
        .spawn((
            HandleComponent {
                direct: handle.clone(),
                optional: None,
                list: Vec::new(),
            },
            AssetBind::new(handle.clone()),
        ))
        .id();

    app.update();
    app.update();
    app.world_mut().resource_mut::<CapturedChanges>().0.clear();
    let _ = app
        .world_mut()
        .resource_mut::<Assets<TrackedAsset>>()
        .insert(handle.id(), TrackedAsset { value: 2 });
    app.update();
    app.update();

    let captured = &app.world().resource::<CapturedChanges>().0;
    let event = captured
        .iter()
        .find(|event| event.asset_id == handle.id().untyped())
        .expect("modified asset should dispatch AssetChanged");
    assert_eq!(event.target_entities, vec![entity]);
}

#[test]
fn config_refresh_targets_reflected_component_without_config_bind() {
    let mut app = reflected_app();
    app.add_plugins(ConfigHmrPlugin {
        debounce_window: std::time::Duration::ZERO,
    });
    bevy_assets_hmr::ext::register_asset_impl::<TrackedAsset>(&mut app, "", false);
    app.setup_hmr_headless()
        .init_resource::<CapturedRefreshes>()
        .add_systems(Update, capture_refreshes);

    let asset_id = AssetId::Uuid {
        uuid: Uuid::from_u128(30),
    };
    app.insert_asset(asset_id, TrackedAsset { value: 1 }, "");
    let entity = app
        .world_mut()
        .spawn(HandleComponent {
            direct: Handle::Uuid(Uuid::from_u128(30), PhantomData),
            optional: None,
            list: Vec::new(),
        })
        .id();
    for _ in 0..3 {
        app.update();
    }
    app.world_mut()
        .resource_mut::<CapturedRefreshes>()
        .0
        .clear();

    let _ = app
        .world_mut()
        .resource_mut::<Assets<TrackedAsset>>()
        .insert(asset_id, TrackedAsset { value: 2 });
    for _ in 0..3 {
        app.update();
    }

    let captured = &app.world().resource::<CapturedRefreshes>().0;
    let refresh = captured
        .iter()
        .find(|refresh| refresh.asset_id == asset_id.untyped())
        .expect("modified config should dispatch ConfigRefresh");
    assert_eq!(refresh.target_entities, vec![entity]);
}

#[test]
fn tracker_resource_is_optional_for_existing_apps() {
    let mut app = App::new();
    app.add_plugins((MinimalPlugins, AssetPlugin::default()))
        .init_asset::<TrackedAsset>()
        .register_asset_reflect::<TrackedAsset>()
        .register_type::<HandleComponent>();
    app.world_mut().spawn(HandleComponent {
        direct: handle(20),
        optional: None,
        list: Vec::new(),
    });
    app.update();
    assert!(!app.world().contains_resource::<ReflectHandleCache>());
}

#[test]
fn bsn_spawn_scene_tracks_root_and_nested_entities_without_asset_bind() {
    let mut app = reflected_scene_app();
    app.watch_asset::<TrackedAsset>()
        .setup_hmr_headless()
        .init_resource::<CapturedChanges>()
        .add_systems(Update, capture_changes);
    let handle = app
        .world_mut()
        .resource_mut::<Assets<TrackedAsset>>()
        .add(TrackedAsset { value: 1 });

    let root = app
        .world_mut()
        .spawn_scene(bsn! {
            BsnHandleComponent { handle: {handle.clone()} }
            Children [
                (
                    BsnHandleComponent { handle: {handle.clone()} }
                ),
            ]
        })
        .expect("BSN scene should resolve immediately")
        .id();

    app.update();

    let entities: Vec<_> = app
        .world_mut()
        .query_filtered::<Entity, With<BsnHandleComponent>>()
        .iter(app.world())
        .collect();
    assert_eq!(entities.len(), 2);
    assert!(entities.contains(&root));

    let cache = app.world().resource::<ReflectHandleCache>();
    let tracked = cache
        .entities_for(handle.id().untyped())
        .expect("reflected BSN handles should be indexed");
    assert_eq!(tracked.len(), 2);
    assert!(entities.iter().all(|entity| tracked.contains(entity)));

    app.update();
    app.world_mut().resource_mut::<CapturedChanges>().0.clear();
    let _ = app
        .world_mut()
        .resource_mut::<Assets<TrackedAsset>>()
        .insert(handle.id(), TrackedAsset { value: 2 });
    app.update();
    app.update();

    let captured = &app.world().resource::<CapturedChanges>().0;
    let event = captured
        .iter()
        .find(|event| event.asset_id == handle.id().untyped())
        .expect("modified BSN asset should dispatch AssetChanged");
    assert_eq!(event.target_entities.len(), 2);
    assert!(
        entities
            .iter()
            .all(|entity| event.target_entities.contains(entity))
    );
}

#[test]
fn bsn_queue_spawn_scene_is_tracked_after_deferred_spawn() {
    let handle = handle(41);
    let mut app = reflected_scene_app();

    app.world_mut().queue_spawn_scene(bsn! {
        BsnHandleComponent { handle: {handle.clone()} }
        Children [
            (
                BsnHandleComponent { handle: {handle.clone()} }
            ),
        ]
    });

    // The scene queue is applied in the scene plugin's schedule after the
    // first scanner pass; the following frame discovers the generated ECS.
    app.update();
    app.update();

    let entities: Vec<_> = app
        .world_mut()
        .query_filtered::<Entity, With<BsnHandleComponent>>()
        .iter(app.world())
        .collect();
    assert_eq!(entities.len(), 2);
    let cache = app.world().resource::<ReflectHandleCache>();
    let tracked = cache
        .entities_for(handle.id().untyped())
        .expect("deferred BSN handles should be indexed");
    assert_eq!(tracked.len(), 2);
    assert!(entities.iter().all(|entity| tracked.contains(entity)));
}
