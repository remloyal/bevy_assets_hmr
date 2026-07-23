//! Incremental discovery of asset handles inside reflected ECS components.
//!
//! The tracker is intentionally additive: explicit [`crate::ConfigBind`] and
//! [`crate::AssetBind`] components remain supported, while this module adds an
//! untyped asset-to-entity index for reflected components that contain
//! `Handle<A>` values.

use bevy::asset::{ReflectHandle, UntypedAssetId};
use bevy::ecs::{component::ComponentId, reflect::AppTypeRegistry};
use bevy::prelude::{App, Entity, Plugin, Resource, SystemSet};
use bevy::reflect::{PartialReflect, ReflectRef, TypeRegistry};
#[cfg(feature = "reflect_auto_tracking")]
use bevy::{
    ecs::{
        archetype::ArchetypeId, component::Components, reflect::ReflectComponent,
        resource::IsResource, system::SystemChangeTick, world::EntityRef,
    },
    prelude::{IntoScheduleConfigs, Query, Res, ResMut, Update, Without},
};
#[cfg(feature = "reflect_auto_tracking")]
use std::time::Instant;
use std::{
    any::TypeId,
    collections::{HashMap, HashSet},
    time::Duration,
};

/// Default maximum depth for recursive reflected-value traversal.
pub const DEFAULT_MAX_REFLECT_DEPTH: usize = 32;

/// Result of extracting handles from one reflected value.
#[derive(Debug, Default)]
pub struct ReflectHandleExtraction {
    /// Unique asset ids discovered in the reflected value.
    pub handles: HashSet<UntypedAssetId>,
    /// Number of reflected nodes visited, including containers and leaves.
    pub visited_values: u64,
    /// Whether traversal stopped because `max_depth` was exceeded.
    pub max_depth_reached: bool,
    /// Handle-looking types that were not registered with `ReflectHandle`.
    pub missing_handle_type_data: HashSet<TypeId>,
}

/// Recursively extract every registered `Handle<A>` from a reflected value.
///
/// `Handle<A>` receives `ReflectHandle` type data through Bevy's
/// `AssetApp::register_asset_reflect::<A>()`. Values without that type data
/// are traversed normally and reported through
/// [`ReflectHandleExtraction::missing_handle_type_data`].
pub fn extract_reflected_handles(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    max_depth: usize,
) -> ReflectHandleExtraction {
    let mut extraction = ReflectHandleExtraction::default();
    visit_reflected_value(value, registry, max_depth, 0, &mut extraction);
    extraction
}

fn visit_reflected_value(
    value: &dyn PartialReflect,
    registry: &TypeRegistry,
    max_depth: usize,
    depth: usize,
    extraction: &mut ReflectHandleExtraction,
) {
    if depth > max_depth {
        extraction.max_depth_reached = true;
        return;
    }
    extraction.visited_values += 1;

    if let Some(reflected) = value.try_as_reflect() {
        let type_id = reflected.as_any().type_id();
        if let Some(reflect_handle) = registry
            .get(type_id)
            .and_then(|registration| registration.data::<ReflectHandle>())
        {
            if let Some(handle) = reflect_handle.downcast_handle_untyped(reflected.as_any()) {
                extraction.handles.insert(handle.id());
                return;
            }
        } else if value
            .get_represented_type_info()
            .is_some_and(|info| info.type_path().starts_with("bevy_asset::Handle<"))
        {
            extraction.missing_handle_type_data.insert(type_id);
            return;
        }
    }

    let next_depth = depth + 1;
    match value.reflect_ref() {
        ReflectRef::Struct(value) => {
            for (_, field) in value.iter_fields() {
                visit_reflected_value(field, registry, max_depth, next_depth, extraction);
            }
        }
        ReflectRef::TupleStruct(value) => {
            for field in value.iter_fields() {
                visit_reflected_value(field, registry, max_depth, next_depth, extraction);
            }
        }
        ReflectRef::Tuple(value) => {
            for field in value.iter_fields() {
                visit_reflected_value(field, registry, max_depth, next_depth, extraction);
            }
        }
        ReflectRef::List(value) => {
            for item in value.iter() {
                visit_reflected_value(item, registry, max_depth, next_depth, extraction);
            }
        }
        ReflectRef::Array(value) => {
            for item in value.iter() {
                visit_reflected_value(item, registry, max_depth, next_depth, extraction);
            }
        }
        ReflectRef::Map(value) => {
            for (key, item) in value.iter() {
                visit_reflected_value(key, registry, max_depth, next_depth, extraction);
                visit_reflected_value(item, registry, max_depth, next_depth, extraction);
            }
        }
        ReflectRef::Set(value) => {
            for item in value.iter() {
                visit_reflected_value(item, registry, max_depth, next_depth, extraction);
            }
        }
        ReflectRef::Enum(value) => {
            for field in value.iter_fields() {
                visit_reflected_value(field.value(), registry, max_depth, next_depth, extraction);
            }
        }
        ReflectRef::Opaque(_) => {}
    }
}

/// Bidirectional index maintained by the reflected Handle tracking system.
#[derive(Resource, Default)]
pub struct ReflectHandleCache {
    component_to_assets: HashMap<Entity, HashMap<ComponentId, HashSet<UntypedAssetId>>>,
    /// All reflected asset ids referenced by each entity.
    pub entity_to_assets: HashMap<Entity, HashSet<UntypedAssetId>>,
    /// All entities whose reflected components reference an asset id.
    pub asset_to_entities: HashMap<UntypedAssetId, HashSet<Entity>>,
    #[cfg(feature = "reflect_auto_tracking")]
    entity_archetypes: HashMap<Entity, ArchetypeId>,
    #[cfg(feature = "reflect_auto_tracking")]
    archetype_candidates: HashMap<ArchetypeId, Vec<ComponentId>>,
    #[cfg(feature = "reflect_auto_tracking")]
    missing_component_reflection: HashSet<TypeId>,
    #[cfg(feature = "reflect_auto_tracking")]
    missing_handle_reflection: HashSet<TypeId>,
    #[cfg(feature = "reflect_auto_tracking")]
    last_validation: Option<Instant>,
    #[cfg(feature = "reflect_auto_tracking")]
    registry_registration_count: usize,
}

impl ReflectHandleCache {
    /// Entities currently referencing `asset_id` through reflected components.
    pub fn entities_for(&self, asset_id: UntypedAssetId) -> Option<&HashSet<Entity>> {
        self.asset_to_entities.get(&asset_id)
    }

    /// Asset ids currently referenced by `entity` through reflected components.
    pub fn assets_for(&self, entity: Entity) -> Option<&HashSet<UntypedAssetId>> {
        self.entity_to_assets.get(&entity)
    }

    /// Asset ids extracted from one component on one entity.
    pub fn component_assets(
        &self,
        entity: Entity,
        component_id: ComponentId,
    ) -> Option<&HashSet<UntypedAssetId>> {
        self.component_to_assets
            .get(&entity)
            .and_then(|components| components.get(&component_id))
    }

    #[cfg(feature = "reflect_auto_tracking")]
    fn set_component_assets(
        &mut self,
        entity: Entity,
        component_id: ComponentId,
        assets: HashSet<UntypedAssetId>,
    ) -> bool {
        let components = self.component_to_assets.entry(entity).or_default();
        if components.get(&component_id) == Some(&assets) {
            return false;
        }
        if assets.is_empty() {
            components.remove(&component_id);
        } else {
            components.insert(component_id, assets);
        }
        if components.is_empty() {
            self.component_to_assets.remove(&entity);
        }
        true
    }

    #[cfg(feature = "reflect_auto_tracking")]
    fn retain_entity_components(
        &mut self,
        entity: Entity,
        current_components: &HashSet<ComponentId>,
    ) -> bool {
        let Some(components) = self.component_to_assets.get_mut(&entity) else {
            return false;
        };
        let old_len = components.len();
        components.retain(|component_id, _| current_components.contains(component_id));
        let changed = old_len != components.len();
        if components.is_empty() {
            self.component_to_assets.remove(&entity);
        }
        changed
    }

    #[cfg(feature = "reflect_auto_tracking")]
    fn rebuild_entity_index(&mut self, entity: Entity) {
        let new_assets: HashSet<_> = self
            .component_to_assets
            .get(&entity)
            .into_iter()
            .flat_map(|components| components.values())
            .flat_map(|assets| assets.iter().copied())
            .collect();
        let old_assets = self.entity_to_assets.remove(&entity).unwrap_or_default();

        for removed in old_assets.difference(&new_assets) {
            let remove_entry = self
                .asset_to_entities
                .get_mut(removed)
                .is_some_and(|entities| {
                    entities.remove(&entity);
                    entities.is_empty()
                });
            if remove_entry {
                self.asset_to_entities.remove(removed);
            }
        }
        for added in new_assets.difference(&old_assets) {
            self.asset_to_entities
                .entry(*added)
                .or_default()
                .insert(entity);
        }
        if !new_assets.is_empty() {
            self.entity_to_assets.insert(entity, new_assets);
        }
    }

    #[cfg(feature = "reflect_auto_tracking")]
    fn remove_entity(&mut self, entity: Entity) {
        self.component_to_assets.remove(&entity);
        self.entity_archetypes.remove(&entity);
        let Some(assets) = self.entity_to_assets.remove(&entity) else {
            return;
        };
        for asset_id in assets {
            let remove_entry = self
                .asset_to_entities
                .get_mut(&asset_id)
                .is_some_and(|entities| {
                    entities.remove(&entity);
                    entities.is_empty()
                });
            if remove_entry {
                self.asset_to_entities.remove(&asset_id);
            }
        }
    }
}

/// Cumulative diagnostics for the incremental scanner.
#[derive(Resource, Debug, Default, Clone)]
pub struct ReflectHandleTrackingMetrics {
    pub frames_scanned: u64,
    pub entities_scanned: u64,
    pub component_ticks_checked: u64,
    pub components_reflected: u64,
    pub reflected_values_visited: u64,
    pub handles_found: u64,
    pub component_cache_updates: u64,
    pub entities_removed: u64,
    pub full_validations: u64,
    pub max_depth_hits: u64,
    pub missing_component_types: u64,
    pub missing_handle_types: u64,
    pub scan_time: Duration,
}

#[cfg(feature = "reflect_auto_tracking")]
#[derive(Resource)]
struct ReflectHandleTrackingSettings {
    validation_interval: Duration,
    max_depth: usize,
}

/// Runs the reflected scanner before typed HMR systems consume asset events.
#[derive(SystemSet, Debug, Clone, Copy, Hash, PartialEq, Eq)]
pub struct ReflectHandleTrackingSet;

/// Optional development-time plugin for automatic reflected Handle tracking.
pub struct ReflectHandleTrackingPlugin {
    /// Low-frequency full validation interval used to repair cache drift.
    pub validation_interval: Duration,
    /// Maximum recursive depth for nested reflected values.
    pub max_depth: usize,
}

impl Default for ReflectHandleTrackingPlugin {
    fn default() -> Self {
        Self {
            validation_interval: Duration::from_secs(30),
            max_depth: DEFAULT_MAX_REFLECT_DEPTH,
        }
    }
}

impl Plugin for ReflectHandleTrackingPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AppTypeRegistry>()
            .init_resource::<ReflectHandleCache>()
            .init_resource::<ReflectHandleTrackingMetrics>();

        #[cfg(feature = "reflect_auto_tracking")]
        app.insert_resource(ReflectHandleTrackingSettings {
            validation_interval: self.validation_interval,
            max_depth: self.max_depth,
        })
        .add_systems(
            Update,
            reflect_handle_tracking_system.in_set(ReflectHandleTrackingSet),
        );
    }
}

/// Incrementally scan reflected components and maintain the three-level index.
#[cfg(feature = "reflect_auto_tracking")]
fn reflect_handle_tracking_system(
    entities: Query<EntityRef, Without<IsResource>>,
    components: &Components,
    type_registry: Res<AppTypeRegistry>,
    settings: Res<ReflectHandleTrackingSettings>,
    mut cache: ResMut<ReflectHandleCache>,
    mut metrics: ResMut<ReflectHandleTrackingMetrics>,
    change_tick: SystemChangeTick,
) {
    let started = Instant::now();
    metrics.frames_scanned += 1;

    let registry = type_registry.read();
    let registration_count = registry.iter().count();
    let registry_changed = registration_count != cache.registry_registration_count;
    cache.registry_registration_count = registration_count;
    let now = Instant::now();
    let full_validation = registry_changed
        || cache
            .last_validation
            .is_none_or(|last| now.duration_since(last) >= settings.validation_interval);
    if full_validation {
        cache.last_validation = Some(now);
        cache.archetype_candidates.clear();
        metrics.full_validations += 1;
    }

    let mut live_entities = HashSet::new();
    for entity_ref in entities.iter() {
        let entity = entity_ref.id();
        let archetype = entity_ref.archetype();
        let archetype_id = archetype.id();
        live_entities.insert(entity);
        metrics.entities_scanned += 1;

        let candidates = if let Some(candidates) = cache.archetype_candidates.get(&archetype_id) {
            candidates.clone()
        } else {
            let mut candidates = Vec::new();
            for component_id in archetype.iter_components() {
                let Some(info) = components.get_info(component_id) else {
                    continue;
                };
                let Some(type_id) = info.type_id() else {
                    continue;
                };
                let Some(registration) = registry.get(type_id) else {
                    if cache.missing_component_reflection.insert(type_id) {
                        metrics.missing_component_types += 1;
                    }
                    continue;
                };
                if registration.data::<ReflectComponent>().is_some() {
                    candidates.push(component_id);
                } else if cache.missing_component_reflection.insert(type_id) {
                    metrics.missing_component_types += 1;
                }
            }
            cache
                .archetype_candidates
                .insert(archetype_id, candidates.clone());
            candidates
        };

        let archetype_changed =
            cache.entity_archetypes.insert(entity, archetype_id) != Some(archetype_id);
        let current_components: HashSet<_> = candidates.iter().copied().collect();
        let mut entity_dirty = if full_validation || archetype_changed {
            cache.retain_entity_components(entity, &current_components)
        } else {
            false
        };

        for component_id in candidates {
            metrics.component_ticks_checked += 1;
            let changed = full_validation
                || archetype_changed
                || entity_ref
                    .get_change_ticks_by_id(component_id)
                    .is_some_and(|ticks| {
                        ticks.is_added(change_tick.last_run(), change_tick.this_run())
                            || ticks.is_changed(change_tick.last_run(), change_tick.this_run())
                    });
            if !changed {
                continue;
            }

            let Some(type_id) = components
                .get_info(component_id)
                .and_then(|info| info.type_id())
            else {
                continue;
            };
            let Some(reflect_component) = registry
                .get(type_id)
                .and_then(|registration| registration.data::<ReflectComponent>())
            else {
                continue;
            };
            let Some(value) = reflect_component.reflect(entity_ref.into_filtered()) else {
                continue;
            };

            metrics.components_reflected += 1;
            let extraction = extract_reflected_handles(value, &registry, settings.max_depth);
            metrics.reflected_values_visited += extraction.visited_values;
            metrics.handles_found += extraction.handles.len() as u64;
            if extraction.max_depth_reached {
                metrics.max_depth_hits += 1;
            }
            for type_id in extraction.missing_handle_type_data {
                if cache.missing_handle_reflection.insert(type_id) {
                    metrics.missing_handle_types += 1;
                    bevy::log::warn!(
                        "[HMR] reflected Handle type is missing ReflectHandle data; call register_asset_reflect for its asset type"
                    );
                }
            }
            if cache.set_component_assets(entity, component_id, extraction.handles) {
                metrics.component_cache_updates += 1;
                entity_dirty = true;
            }
        }

        if entity_dirty {
            cache.rebuild_entity_index(entity);
        }
    }

    let stale_entities: Vec<_> = cache
        .entity_archetypes
        .keys()
        .copied()
        .filter(|entity| !live_entities.contains(entity))
        .collect();
    for entity in stale_entities {
        cache.remove_entity(entity);
        metrics.entities_removed += 1;
    }

    metrics.scan_time += started.elapsed();
}

/// Merge reflected targets into an existing typed binding snapshot.
pub(crate) fn merge_target_entities(
    cache: Option<&ReflectHandleCache>,
    asset_id: UntypedAssetId,
    target_entities: &mut Vec<Entity>,
) {
    let Some(reflected_entities) = cache.and_then(|cache| cache.entities_for(asset_id)) else {
        return;
    };
    let mut seen: HashSet<_> = target_entities.iter().copied().collect();
    for entity in reflected_entities {
        if seen.insert(*entity) {
            target_entities.push(*entity);
        }
    }
    target_entities.sort_unstable_by_key(|entity| entity.to_bits());
}
