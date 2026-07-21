//! Binding layer: marker component + bidirectional handle↔entity cache.
//!
//! **When to use `ConfigBind<A>`**: attach it to entities that directly
//! consume a single config asset (e.g. a UI panel driven by one layout file).
//! The HMR core will then dispatch `ConfigRefresh<A::Config>` with
//! `target_entities` populated for that handle.
//!
//! **When NOT to use it**: for "one file → many objects" data tables
//! (e.g. `npc.ron` containing all NPCs), the consuming crate typically
//! reads from a central `GameData` Resource instead. In that case, skip
//! `ConfigBind<A>` and let subscribers filter `ConfigRefresh<A::Config>`
//! by `changed_ids` against their own per-entity id field.

use crate::core::HmrSource;
use bevy::asset::AssetId;
use bevy::ecs::{component::Component, entity::Entity, system::Query};
use bevy::prelude::{Changed, Resource};
use std::collections::{HashMap, HashSet};

/// Marker component binding an entity to an `A` handle (where `A: HmrSource`).
///
/// Attach via `commands.entity(e).insert(ConfigBind::new(handle))` or
/// `ConfigBind::with_id(asset_id)` when you only have an `AssetId` (e.g.
/// headless tests). The HMR core auto-registers it into
/// [`HandleEntityCache<A>`].
#[derive(Component, Clone, Debug, Default)]
pub struct ConfigBind<A: HmrSource> {
    /// Handle to the asset this entity depends on.
    pub handle: bevy::prelude::Handle<A>,
}

impl<A: HmrSource> ConfigBind<A> {
    /// Construct from a `Handle<A>` (typically obtained from
    /// `AssetServer::load`).
    pub fn new(handle: impl Into<bevy::prelude::Handle<A>>) -> Self {
        Self {
            handle: handle.into(),
        }
    }

    /// Construct from an `AssetId<A>` using a Uuid-backed weak handle.
    /// Useful when you only have an id (e.g. headless tests that bypass
    /// `AssetServer::load`) and need `ConfigBind.handle.id()` to match
    /// a known id.
    ///
    /// **Limitation**: Bevy's `Handle` only has `Strong` and `Uuid`
    /// variants — there is no public way to construct a `Handle` from an
    /// `AssetId::Index` without going through `Assets` (which owns the
    /// `Arc<StrongHandle>`). For `AssetId::Index`, this falls back to
    /// `Handle::default()` (a Uuid handle with the default UUID) and logs a
    /// warning to stderr, because the resulting bind will never match the
    /// intended asset. Prefer [`ConfigBind::new`] with a real strong handle
    /// (e.g. obtained from `Assets::get` or `AssetServer::load`) when you
    /// have an `AssetId::Index`.
    pub fn with_id(id: AssetId<A>) -> Self {
        let handle = match id {
            AssetId::Uuid { uuid } => {
                bevy::prelude::Handle::<A>::Uuid(uuid, std::marker::PhantomData)
            }
            AssetId::Index { .. } => {
                // Bevy has no public constructor for a Handle from an
                // AssetId::Index (it requires the internal Arc<StrongHandle>
                // owned by Assets). Emit a warning so callers aren't left
                // guessing why their bind silently fails to match.
                bevy::log::warn!(
                    "[HMR] ConfigBind::with_id received AssetId::Index for {}, \
                     which cannot be converted to a matching Handle. \
                     Falling back to Handle::default() (the bind will NOT match \
                     this asset). Use ConfigBind::new with a real handle instead.",
                    A::type_path()
                );
                bevy::prelude::Handle::default()
            }
        };
        Self { handle }
    }
}

/// Bidirectional cache mapping `AssetId<A>` ↔ `Entity` set.
///
/// Populated automatically by [`config_binding_registry_system`] watching
/// `Added<ConfigBind<A>>`. Consulted by `flush_debounced_refresh` to populate
/// `target_entities` in [`crate::ConfigRefresh`] events.
#[derive(Resource)]
pub struct HandleEntityCache<A: HmrSource> {
    /// Asset id → set of entities bound to it.
    pub handle_to_entities: HashMap<AssetId<A>, HashSet<Entity>>,
    /// Reverse: entity → asset id (for O(1) removal).
    pub entity_to_handle: HashMap<Entity, AssetId<A>>,
}

impl<A: HmrSource> Default for HandleEntityCache<A> {
    fn default() -> Self {
        Self {
            handle_to_entities: HashMap::new(),
            entity_to_handle: HashMap::new(),
        }
    }
}

impl<A: HmrSource> HandleEntityCache<A> {
    /// Register a (entity, handle) binding. If the entity was previously
    /// bound to a different handle, that prior binding is removed first.
    pub fn insert(&mut self, entity: Entity, handle: AssetId<A>) {
        // Remove old binding if present.
        if let Some(old) = self.entity_to_handle.insert(entity, handle) {
            if let Some(set) = self.handle_to_entities.get_mut(&old) {
                set.remove(&entity);
                if set.is_empty() {
                    self.handle_to_entities.remove(&old);
                }
            }
        }
        self.handle_to_entities
            .entry(handle)
            .or_default()
            .insert(entity);
    }

    /// Look up all entities bound to the given handle.
    pub fn get_entities(&self, handle: &AssetId<A>) -> Option<&HashSet<Entity>> {
        self.handle_to_entities.get(handle)
    }

    /// Remove an entity from the cache (e.g. when it's despawned).
    pub fn remove(&mut self, entity: Entity) {
        if let Some(old) = self.entity_to_handle.remove(&entity) {
            if let Some(set) = self.handle_to_entities.get_mut(&old) {
                set.remove(&entity);
                if set.is_empty() {
                    self.handle_to_entities.remove(&old);
                }
            }
        }
    }

    /// Returns `true` if the cache currently has no bindings.
    pub fn is_empty(&self) -> bool {
        self.entity_to_handle.is_empty()
    }
}

/// System: auto-register new `ConfigBind<A>` components into the cache.
///
/// Detects entities whose `ConfigBind<A>` handle has changed (including
/// newly added components) and updates [`HandleEntityCache<A>`] accordingly.
/// Runs every frame as part of the HMR pipeline (chained before
/// `hmr_core_system`).
pub fn config_binding_registry_system<A: HmrSource>(
    mut cache: bevy::prelude::ResMut<HandleEntityCache<A>>,
    bindings: Query<(Entity, &ConfigBind<A>), Changed<ConfigBind<A>>>,
) {
    for (entity, bind) in bindings.iter() {
        cache.insert(entity, bind.handle.id());
    }
}

/// System: remove despawned entities from [`HandleEntityCache<A>`].
///
/// `RemovedComponents<ConfigBind<A>>` yields entities whose `ConfigBind<A>`
/// component was removed since the last time this system ran. This covers
/// both explicit component removal and full entity despawn. Without this
/// system, the cache would retain dangling `entity → handle` mappings for
/// up to 5 seconds (until `cache_validation_system` eventually rebuilds),
/// causing `target_entities` in [`crate::ConfigRefresh`] to reference
/// non-existent entities in the interim.
///
/// Runs every frame as part of the HMR pipeline, chained after
/// `config_binding_registry_system`.
pub fn config_binding_cleanup_system<A: HmrSource>(
    mut cache: bevy::prelude::ResMut<HandleEntityCache<A>>,
    mut removed: bevy::prelude::RemovedComponents<ConfigBind<A>>,
) {
    for entity in removed.read() {
        cache.remove(entity);
    }
}
