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
//! app.add_plugins(DefaultPlugins) // enables bevy/file_watcher
//!     .add_plugins(ConfigHmrPlugin::default())
//!     .watch_asset::<Image>("textures/player.png")
//!     .watch_asset::<Scene>("models/character.gltf#Scene0");
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
    system::{Query, Res, ResMut},
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
/// Incrementally maintained by [`asset_bind_registry_system`] (Added/Changed)
/// and [`asset_bind_cleanup_system`] (Removed/Despawn).
#[derive(Resource)]
pub struct AssetBindCache<A: Asset> {
    /// `AssetId<A>` -> set of entities bound to that handle.
    pub handle_to_entities: HashMap<AssetId<A>, HashSet<Entity>>,
    /// `Entity` -> `AssetId<A>` (for cleanup).
    pub entity_to_handle: HashMap<Entity, AssetId<A>>,
}

impl<A: Asset> Default for AssetBindCache<A> {
    fn default() -> Self {
        Self {
            handle_to_entities: HashMap::new(),
            entity_to_handle: HashMap::new(),
        }
    }
}

impl<A: Asset> AssetBindCache<A> {
    /// Insert / update an entity's binding.
    pub fn insert(&mut self, entity: Entity, id: AssetId<A>) {
        // Remove old binding if any.
        if let Some(old_id) = self.entity_to_handle.get(&entity) {
            if *old_id == id {
                return; // unchanged
            }
            if let Some(set) = self.handle_to_entities.get_mut(old_id) {
                set.remove(&entity);
                if set.is_empty() {
                    self.handle_to_entities.remove(old_id);
                }
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
            if let Some(set) = self.handle_to_entities.get_mut(&id) {
                set.remove(&entity);
                if set.is_empty() {
                    self.handle_to_entities.remove(&id);
                }
            }
        }
    }

    /// Get the set of entities bound to the given asset id.
    pub fn get_entities(&self, id: &AssetId<A>) -> Option<&HashSet<Entity>> {
        self.handle_to_entities.get(id)
    }

    /// Whether the cache is empty.
    pub fn is_empty(&self) -> bool {
        self.entity_to_handle.is_empty()
    }
}

/// Event dispatched when an asset of type `A` has been modified (hot-reloaded
/// from disk by `AssetServer`).
///
/// Unlike [`crate::ConfigRefresh`], this carries no `new_config` / diff —
/// Bevy's asset pipeline has already updated the `Assets<A>` resource and (for
/// render assets) uploaded to GPU. Subscribers use this for side-effects:
/// rebuilding colliders, re-deriving atlases, logging, etc.
#[derive(Message, Clone)]
pub struct AssetChanged<A: Asset + Clone + Send + Sync + 'static> {
    /// The asset id that changed.
    pub asset_id: AssetId<A>,
    /// Entities bound to this asset via [`AssetBind<A>`] (snapshot at
    /// dispatch time).
    pub target_entities: Vec<Entity>,
    /// Asset source path (relative to `assets/`), if known. Empty if the
    /// asset was loaded without a path (e.g. inserted directly).
    pub source_path: String,
    /// The freshly reloaded asset value (a clone). Subscribers that only need
    /// the `target_entities` can ignore this.
    pub new_asset: A,
}

/// System: auto-register new/changed `AssetBind<A>` into the cache.
pub fn asset_bind_registry_system<A: Asset>(
    mut cache: ResMut<AssetBindCache<A>>,
    bindings: Query<(Entity, &AssetBind<A>), Changed<AssetBind<A>>>,
) {
    for (entity, bind) in bindings.iter() {
        cache.insert(entity, bind.handle.id());
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

/// System: watch `AssetEvent<A>` and dispatch [`AssetChanged<A>`] on
/// `Modified` (and `Added`, to notify subscribers of first load if they
/// want to initialize from the asset).
///
/// `Removed` is **not** handled here — for arbitrary asset types, removal
/// semantics are application-specific. Subscribers can additionally read
/// `AssetEvent<A>` directly if they need removal notifications.
pub fn asset_watcher_system<A: Asset + Clone + Send + Sync + 'static>(
    mut asset_events: MessageReader<AssetEvent<A>>,
    assets: Res<bevy::asset::Assets<A>>,
    cache: Res<AssetBindCache<A>>,
    mut changed_evts: MessageWriter<AssetChanged<A>>,
) {
    for evt in asset_events.read() {
        let id = match evt {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => *id,
            _ => continue,
        };

        // Only dispatch if there are bound entities or the asset exists.
        let Some(asset) = assets.get(id) else {
            continue;
        };

        let target_entities: Vec<Entity> = cache
            .get_entities(&id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();

        let target_count = target_entities.len();

        // Even with zero target_entities, dispatch so subscribers using the
        // event (not the binding) can react. But skip if there are no
        // subscribers at all — the MessageWriter will just be dropped if
        // nobody reads it, so always dispatch.
        changed_evts.write(AssetChanged {
            asset_id: id,
            target_entities,
            source_path: String::new(), // AssetServer doesn't expose path lookup easily
            new_asset: asset.clone(),
        });

        bevy::log::info!(
            "[HMR] AssetChanged<{}> asset_id={:?}，关联实体数={}",
            std::any::type_name::<A>(),
            id,
            target_count,
        );
    }
}
