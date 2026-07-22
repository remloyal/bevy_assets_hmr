//! File-level hot-reload for **any** Bevy `Asset` type (images, 3D models,
//! audio, scenes, fonts, …) — no `ConfigDiff` required.
//!
//! Unlike [`crate::register_config`] / [`crate::register_asset`], which
//! compute an id-level diff and dispatch [`crate::ConfigRefresh`],
//! [`watch_asset`](crate::ConfigHmrAppExt::watch_asset) only provides:
//!
//! 1. **Entity binding** via [`AssetBind<A>`] — attach to entities so the
//!    framework knows which entities depend on a given asset handle.
//! 2. **Change notification** via [`AssetChanged<A>`] — fired whenever the
//!    asset is modified (file edited on disk → `AssetEvent::Modified`).
//!
//! Bevy's `AssetServer` (with `bevy/file_watcher`) already handles the actual
//! file reloading and GPU upload for built-in asset types. This module just
//! adds the "which entities care about this asset?" tracking and a clean
//! notification event, so subscribers can react (e.g. rebuild a mesh
//! collider, re-derive a sprite atlas, log a reload).
//!
//! # Example
//!
//! ```ignore
//! app.add_plugins(DefaultPlugins) // 需在 Cargo.toml 显式启用 `bevy/file_watcher` feature
//!     .add_plugins(ConfigHmrPlugin::default())
//!     .watch_asset::<Image>()
//!     .watch_asset::<Scene>();
//!
//! // Subscribe:
//! fn on_image_changed(mut reader: MessageReader<AssetChanged<Image>>) {
//!     for evt in reader.read() {
//!         println!("image changed: {} entities", evt.target_entities.len());
//!     }
//! }
//! ```

use bevy::asset::{Asset, AssetEvent, AssetId, HandleTemplate};
use bevy::ecs::{
    component::Component,
    entity::Entity,
    message::{Message, MessageReader, MessageWriter},
    system::{Query, ResMut},
    template::{FromTemplate, SpecializeFromTemplate, Template, TemplateContext},
};
use bevy::prelude::{Changed, Handle, RemovedComponents, Resource};
use std::collections::{HashMap, HashSet};

/// Marker component binding an entity to an `A` handle (where `A: Asset`).
///
/// The `watch_asset` framework auto-registers it into [`AssetBindCache<A>`]
/// and uses the cache to populate `target_entities` in [`AssetChanged<A>`].
///
/// This is the `Asset`-level counterpart of [`crate::ConfigBind`]; use it for
/// non-config asset types (images, meshes, audio) where you only need
/// change-notification, not id-level diffing.
#[derive(Component, Clone, Debug, Default)]
pub struct AssetBind<A: Asset> {
    /// Handle to the asset this entity depends on.
    pub handle: Handle<A>,
}

// ["auto trait specialization" trick](https://github.com/coolcatcoder/rust_techniques/issues/1)
// — mirrors Bevy's `impl Unpin for Handle<T> where for<'a> [()]: SpecializeFromTemplate`.
// Because `SpecializeFromTemplate` has no impls, this fixes `AssetBind<A>` as
// *definitely not* `Unpin`, which is what lets the hand-written `impl
// FromTemplate for AssetBind<A>` below not conflict with Bevy's blanket
// `impl<T: Clone + Default + Unpin> FromTemplate for T` (E0119). Without this,
// the compiler assumes upstream *might* add `Unpin for Handle<T>` later and
// rejects our impl as a potential future conflict.
impl<A: Asset> Unpin for AssetBind<A> where for<'a> [()]: SpecializeFromTemplate {}

impl<A: Asset> AssetBind<A> {
    /// Construct from a `Handle<A>` (typically from `AssetServer::load`).
    pub fn new(handle: impl Into<Handle<A>>) -> Self {
        Self {
            handle: handle.into(),
        }
    }

    /// Construct from an `AssetId<A>` using a Uuid-backed weak handle.
    ///
    /// See [`crate::ConfigBind::with_id`] for the same limitation regarding
    /// `AssetId::Index` (Bevy has no public `Handle::Index` constructor).
    pub fn with_id(id: AssetId<A>) -> Self {
        let handle = match id {
            AssetId::Uuid { uuid } => Handle::<A>::Uuid(uuid, std::marker::PhantomData),
            AssetId::Index { .. } => {
                bevy::log::warn!(
                    "[HMR] AssetBind::with_id received AssetId::Index for {}, \
                     which cannot be converted to a matching Handle. \
                     Falling back to Handle::default().",
                    A::type_path()
                );
                Handle::default()
            }
        };
        Self { handle }
    }
}

/// [`Template`] backed by a [`HandleTemplate<A>`], used as
/// `<AssetBind<A> as FromTemplate>::Template` so that [`AssetBind<A>`] can be
/// written directly inside `bevy::scene::bsn!`.
///
/// # Why this exists
///
/// Bevy 0.19's blanket `impl<T: Clone + Default + Unpin> FromTemplate for T`
/// intentionally excludes `Handle<A>` (via the `SpecializeFromTemplate` auto-
/// trait trick), forcing handle-bearing types to use a custom [`Template`].
/// Because `AssetBind` contains a `Handle<A>`, it cannot pick up that
/// blanket impl. This struct is the hand-written counterpart: it stores a
/// [`HandleTemplate<A>`] (which already implements `Template<Output = Handle<A>>`),
/// and builds an `AssetBind<A>` by resolving that template at spawn time.
///
/// `HandleTemplate<A>: Default + Clone + Send + Sync + 'static` for any
/// `A: Asset`, this type also satisfies the `Template + Default + Send + Sync
/// + 'static` bound required by `ResolvedScene::get_or_insert_template` (the
/// insertion point used by `bsn!`'s `FromTemplatePatch` codegen).
///
/// # Usage in `bsn!`
///
/// You don't construct this directly. Just write `AssetBind<A>` like any other
/// component in `bsn!`; both string-path and `Handle<A>` field values work,
/// because `bsn!` inserts `.into()` and `HandleTemplate<A>: From<Handle<A>>`
/// / `From<impl Into<AssetPath<'static>>>`:
///
/// ```ignore
/// commands.spawn_scene(bsn! {
///     ImageNode { image: "textures/bg.png" }
///     AssetBind::<Image> { handle: "textures/bg.png" }
/// });
/// // or with a preloaded handle:
/// let handle: Handle<Image> = asset_server.load("textures/bg.png");
/// commands.spawn_scene(bsn! {
///     ImageNode { image: handle.clone() }
///     AssetBind::<Image> { handle }
/// });
/// ```
#[derive(Default)]
pub struct AssetBindTemplate<A: Asset> {
    /// Handle template — accepts `Handle<A>`, `&'static str` (asset path), or
    /// anything `Into<AssetPath<'static>>` at `bsn!` call sites.
    pub handle: HandleTemplate<A>,
}

// Same `SpecializeFromTemplate` trick as `AssetBind<A>` above: makes
// `AssetBindTemplate<A>` definitively `!Unpin` so its hand-written `Template`
// impl doesn't conflict with Bevy's blanket `impl<T: Clone + Default + Unpin>
// Template for T`.
impl<A: Asset> Unpin for AssetBindTemplate<A> where for<'a> [()]: SpecializeFromTemplate {}

impl<A: Asset> Template for AssetBindTemplate<A> {
    type Output = AssetBind<A>;

    fn build_template(
        &self,
        context: &mut TemplateContext,
    ) -> bevy::ecs::error::Result<AssetBind<A>> {
        Ok(AssetBind {
            handle: self.handle.build_template(context)?,
        })
    }

    fn clone_template(&self) -> Self {
        Self {
            handle: self.handle.clone_template(),
        }
    }
}

impl<A: Asset> FromTemplate for AssetBind<A> {
    type Template = AssetBindTemplate<A>;
}

/// Cache mapping `AssetId<A> -> {Entity}` for all entities with
/// `AssetBind<A>`. Used to populate `target_entities` in [`AssetChanged<A>`].
///
/// Also maintains an `AssetId<A> -> source_path` registry so that
/// [`AssetChanged<A>`] events can carry a meaningful `source_path`.
///
/// Incrementally maintained by [`asset_bind_registry_system`] (Added/Changed)
/// and [`asset_bind_cleanup_system`] (Removed/Despawn).
#[derive(Resource)]
pub struct AssetBindCache<A: Asset> {
    /// `AssetId<A>` -> set of entities bound to that handle.
    pub handle_to_entities: HashMap<AssetId<A>, HashSet<Entity>>,
    /// `Entity` -> `AssetId<A>` (for cleanup).
    pub entity_to_handle: HashMap<Entity, AssetId<A>>,
    /// `AssetId<A>` -> source path (relative to `assets/`).
    ///
    /// Populated from `Handle::path()` whenever an `AssetBind<A>` with a
    /// path-bearing handle is registered. Used by [`asset_watcher_system`]
    /// to fill `AssetChanged<A>.source_path`.
    pub path_registry: HashMap<AssetId<A>, String>,
}

impl<A: Asset> Default for AssetBindCache<A> {
    fn default() -> Self {
        Self {
            handle_to_entities: HashMap::new(),
            entity_to_handle: HashMap::new(),
            path_registry: HashMap::new(),
        }
    }
}

impl<A: Asset> AssetBindCache<A> {
    /// Insert / update an entity's binding.
    pub fn insert(&mut self, entity: Entity, id: AssetId<A>) {
        // Remove old binding if any.
        if let Some(old_id) = self.entity_to_handle.get(&entity).copied() {
            if old_id == id {
                return; // unchanged
            }
            let old_empty = if let Some(set) = self.handle_to_entities.get_mut(&old_id) {
                set.remove(&entity);
                set.is_empty()
            } else {
                false
            };
            if old_empty {
                self.handle_to_entities.remove(&old_id);
                self.path_registry.remove(&old_id);
            }
        }
        self.entity_to_handle.insert(entity, id);
        self.handle_to_entities
            .entry(id)
            .or_default()
            .insert(entity);
    }

    /// Remove an entity from the cache (on despawn / component removal).
    pub fn remove(&mut self, entity: Entity) {
        if let Some(id) = self.entity_to_handle.remove(&entity) {
            let last_binding = if let Some(set) = self.handle_to_entities.get_mut(&id) {
                set.remove(&entity);
                set.is_empty()
            } else {
                false
            };
            if last_binding {
                self.handle_to_entities.remove(&id);
                self.path_registry.remove(&id);
            }
        }
    }

    /// Get the set of entities bound to the given asset id.
    pub fn get_entities(&self, id: &AssetId<A>) -> Option<&HashSet<Entity>> {
        self.handle_to_entities.get(id)
    }

    /// Record the source path for an asset id (if not already known).
    ///
    /// Called by [`asset_bind_registry_system`] when a `Handle<A>` with a
    /// path is encountered. Idempotent: the first path recorded wins.
    pub fn record_path(&mut self, id: AssetId<A>, path: String) {
        self.path_registry.entry(id).or_insert(path);
    }

    /// Look up the source path for an asset id.
    pub fn get_path(&self, id: &AssetId<A>) -> Option<&str> {
        self.path_registry.get(id).map(|s| s.as_str())
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entity_to_handle.is_empty()
    }
}

/// Event dispatched when an asset of type `A` has been added or modified
/// (including hot reloads from disk by `AssetServer`).
///
/// The asset value is deliberately not embedded in this message. Large assets
/// such as Bevy's `Image` own decoded byte buffers, so cloning the
/// value on every hot reload can stall the main thread and temporarily double
/// its memory use. Consumers that need the value can query [`bevy::asset::Assets<A>`]
/// using [`asset_id`](Self::asset_id), or call [`asset`](Self::asset).
#[derive(Message)]
pub struct AssetChanged<A: Asset> {
    pub asset_id: AssetId<A>,
    pub target_entities: Vec<Entity>,
    /// Populated from `AssetBindCache::path_registry` when available.
    pub source_path: String,
}

impl<A: Asset> Clone for AssetChanged<A> {
    fn clone(&self) -> Self {
        Self {
            asset_id: self.asset_id,
            target_entities: self.target_entities.clone(),
            source_path: self.source_path.clone(),
        }
    }
}

impl<A: Asset> AssetChanged<A> {
    /// Look up the current asset value without cloning it into the message.
    pub fn asset<'a>(&self, assets: &'a bevy::asset::Assets<A>) -> Option<&'a A> {
        assets.get(self.asset_id)
    }
}

/// Event dispatched when an asset of type `A` has been **removed**.
#[derive(Message)]
pub struct AssetRemoved<A: Asset> {
    pub asset_id: AssetId<A>,
    pub target_entities: Vec<Entity>,
    pub source_path: String,
    pub _marker: std::marker::PhantomData<A>,
}

impl<A: Asset> Clone for AssetRemoved<A> {
    fn clone(&self) -> Self {
        Self {
            asset_id: self.asset_id,
            target_entities: self.target_entities.clone(),
            source_path: self.source_path.clone(),
            _marker: std::marker::PhantomData,
        }
    }
}

impl<A: Asset> Default for AssetRemoved<A> {
    fn default() -> Self {
        Self {
            asset_id: AssetId::default(),
            target_entities: Vec::new(),
            source_path: String::new(),
            _marker: std::marker::PhantomData,
        }
    }
}

/// System: auto-register new/changed `AssetBind<A>` into the cache.
///
/// Also records the handle's source path (if available via `Handle::path()`)
/// into `AssetBindCache::path_registry`, so that `AssetChanged<A>` events
/// can carry a meaningful `source_path`.
pub fn asset_bind_registry_system<A: Asset>(
    mut cache: ResMut<AssetBindCache<A>>,
    bindings: Query<(Entity, &AssetBind<A>), Changed<AssetBind<A>>>,
) {
    for (entity, bind) in bindings.iter() {
        let id = bind.handle.id();
        cache.insert(entity, id);
        // Record the source path from the handle (if it has one).
        // Strong handles created via `AssetServer::load` carry a path;
        // weak/default handles don't, in which case we leave the registry
        // untouched (source_path will be empty for that asset).
        if let Some(path) = bind.handle.path() {
            cache.record_path(id, path.to_string());
        }
    }
}

/// System: remove despawned entities from [`AssetBindCache<A>`].
pub fn asset_bind_cleanup_system<A: Asset>(
    mut cache: ResMut<AssetBindCache<A>>,
    mut removed: RemovedComponents<AssetBind<A>>,
) {
    for entity in removed.read() {
        cache.remove(entity);
    }
}

/// System: watch `AssetEvent<A>` and dispatch:
/// - [`AssetChanged<A>`] on `Added` / `Modified` (with `source_path` filled
///   from [`AssetBindCache::path_registry`] when available).
/// - [`AssetRemoved<A>`] on `Removed` (so subscribers can do cleanup).
pub fn asset_watcher_system<A: Asset>(
    mut asset_events: MessageReader<AssetEvent<A>>,
    bindings: Query<&AssetBind<A>>,
    mut cache: ResMut<AssetBindCache<A>>,
    mut changed_evts: MessageWriter<AssetChanged<A>>,
    mut removed_evts: MessageWriter<AssetRemoved<A>>,
) {
    for evt in asset_events.read() {
        match evt {
            AssetEvent::Added { id } => {
                let id = *id;
                // A previous Removed event clears the path registry. If the
                // same handle becomes live again, recover its path from the
                // still-present binding before dispatching AssetChanged.
                if cache.get_path(&id).is_none() {
                    let path = bindings
                        .iter()
                        .find(|bind| bind.handle.id() == id)
                        .and_then(|bind| bind.handle.path())
                        .map(ToString::to_string);
                    if let Some(path) = path {
                        cache.record_path(id, path);
                    }
                }
                let target_entities: Vec<Entity> = cache
                    .get_entities(&id)
                    .map(|s| s.iter().copied().collect())
                    .unwrap_or_default();
                let target_count = target_entities.len();
                let source_path = cache.get_path(&id).unwrap_or_default().to_string();
                changed_evts.write(AssetChanged {
                    asset_id: id,
                    target_entities,
                    source_path,
                });
                bevy::log::debug!(
                    "[HMR] AssetChanged<{}> asset_id={:?}，关联实体数={}",
                    std::any::type_name::<A>(),
                    id,
                    target_count,
                );
            }
            AssetEvent::Modified { id } => {
                let id = *id;
                let target_entities: Vec<Entity> = cache
                    .get_entities(&id)
                    .map(|s| s.iter().copied().collect())
                    .unwrap_or_default();
                let target_count = target_entities.len();
                let source_path = cache.get_path(&id).unwrap_or_default().to_string();
                changed_evts.write(AssetChanged {
                    asset_id: id,
                    target_entities,
                    source_path,
                });
                bevy::log::debug!(
                    "[HMR] AssetChanged<{}> asset_id={:?}，关联实体数={}",
                    std::any::type_name::<A>(),
                    id,
                    target_count,
                );
            }
            AssetEvent::Removed { id } => {
                let id = *id;
                let target_entities: Vec<Entity> = cache
                    .get_entities(&id)
                    .map(|s| s.iter().copied().collect())
                    .unwrap_or_default();
                let target_count = target_entities.len();
                // Preserve the path in AssetRemoved, then release the cache
                // entry. A later Added event can recover it from live binds.
                let source_path = cache.path_registry.remove(&id).unwrap_or_default();
                removed_evts.write(AssetRemoved {
                    asset_id: id,
                    target_entities,
                    source_path,
                    _marker: std::marker::PhantomData,
                });
                bevy::log::debug!(
                    "[HMR] AssetRemoved<{}> asset_id={:?}，关联实体数={}",
                    std::any::type_name::<A>(),
                    id,
                    target_count,
                );
            }
            _ => {}
        }
    }
}
