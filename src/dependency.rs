//! Dependency-chain hot reload: cross-asset cascading.
//!
//! When an asset is modified, any parent asset that holds a `Handle<Child>`
//! referencing it should also receive a `ConfigRefresh<T>` so its subscribers
//! can re-derive state from the new child value.
//!
//! # How it works
//!
//! 1. **`DependencyGraph`** Resource maintains a bidirectional map:
//!    - `parents_of`: `child UntypedAssetId -> { (parent UntypedAssetId, parent TypeId) }`
//!    - Used to find which parent assets need a cascade when a child changes.
//!
//! 2. **`dependency_registry_system<A>`** (per-type, chained after
//!    `flush_debounced_refresh`) inspects `AssetEvent<A>::Added/Modified`
//!    and rebuilds the `parents_of` edges for that asset by calling
//!    `A::visit_dependencies(|child_id| ...)`.
//!
//! 3. **`flush_debounced_refresh<A>`** (existing, modified) at the end of its
//!    successful dispatch path, queries the graph for the asset's *parents*
//!    and pushes parent plus triggering-child entries into a per-frame
//!    `CascadeQueue`.
//!
//! 4. **`cascade_dispatch_system<A>`** (per-type, chained last) reads the
//!    `CascadeQueue`, filters entries where `parent_type == TypeId::of::<A>()`,
//!    and dispatches a **`ConfigRefresh<T>`** with
//!    `RefreshCause::Dependency { triggered_by }` to parent subscribers.
//!
//! # Subscriber semantics
//!
//! Cascade-fired `ConfigRefresh<T>` events have an empty delta and carry the
//! parent's current state. Subscribers can detect a cascade via:
//! ```ignore
//! if let RefreshCause::Dependency { triggered_by } = &refresh.cause {
//!     // Re-derive state using the complete trigger set.
//! }
//! ```
//!
//! # Scope
//!
//! Only assets registered via `register_config` / `register_asset` participate
//! in the graph (they implement `HmrSource: Asset + VisitAssetDependencies`).
//! `watch_asset`-registered assets only build the graph for the parent side
//! (so their cascade receives `AssetChanged<A>`, not `ConfigRefresh<T>`).

use crate::core::HmrSource;
use bevy::asset::{AssetEvent, AssetId, Assets, UntypedAssetId};
use bevy::ecs::message::MessageReader;
use bevy::ecs::system::{Res, ResMut};
use bevy::prelude::{MessageWriter, Resource};
use std::any::TypeId;
use std::collections::{HashMap, HashSet};

/// Bidirectional dependency map: which parent assets reference which children.
///
/// Keyed by child `UntypedAssetId`; value is the set of (parent id, parent
/// type) pairs that depend on the child.
#[derive(Resource, Default)]
pub struct DependencyGraph {
    /// `child UntypedAssetId -> { (parent UntypedAssetId, parent TypeId) }`
    ///
    /// Maintained incrementally by [`dependency_registry_system`] (per-type
    /// rebuild on every Modified/Added) and pruned by
    /// [`dependency_cleanup_system`] (on Removed).
    pub parents_of: HashMap<UntypedAssetId, Vec<(UntypedAssetId, TypeId)>>,
}

/// Per-frame queue of cascade entries produced by `flush_debounced_refresh`.
///
/// Each entry: a child that was just refreshed, plus a parent that should
/// be re-dispatched. The queue is drained every frame by
/// [`cascade_dispatch_system`] (one such system per registered type).
#[derive(Resource, Default)]
pub struct CascadeQueue {
    /// Requests pending cascade dispatch.
    pub pending: Vec<CascadeRequest>,
}

/// One dependency-triggered parent refresh request.
#[derive(Clone, Copy, Debug)]
pub struct CascadeRequest {
    pub parent_id: UntypedAssetId,
    pub parent_type: TypeId,
    pub triggered_by: UntypedAssetId,
}

impl DependencyGraph {
    /// Record the dependency edges for a parent asset by walking its
    /// `visit_dependencies` callback. Clears any previous edges for this
    /// parent before re-adding, so callers don't need to pre-clean.
    pub fn rebuild_for<A: HmrSource>(&mut self, parent_id: AssetId<A>, parent_asset: &A) {
        let parent_untyped = parent_id.untyped();
        // Remove any previous edges where this parent was listed as a
        // dependent of some child.
        self.parents_of.retain(|_, parents| {
            parents.retain(|(p, _)| *p != parent_untyped);
            !parents.is_empty()
        });
        // Walk the parent's `Handle<*>` fields. Collect into a set first so
        // that a parent holding the same `Handle` in multiple fields (or a
        // `Vec<Handle>` with duplicates) only registers a single edge per
        // child, preventing duplicate cascade dispatches later.
        let mut seen: HashSet<UntypedAssetId> = HashSet::new();
        parent_asset.visit_dependencies(&mut |child_id: UntypedAssetId| {
            if seen.insert(child_id) {
                self.parents_of
                    .entry(child_id)
                    .or_default()
                    .push((parent_untyped, TypeId::of::<A>()));
            }
        });
    }

    /// Remove all edges involving the given asset (called on
    /// `AssetEvent::Removed`).
    pub fn remove<A: HmrSource>(&mut self, id: AssetId<A>) {
        let untyped = id.untyped();
        // Remove from the "parents_of" maps: this asset may have been a
        // child (entries keyed by other child ids), or it may have been a
        // parent (its own outgoing edges).
        self.parents_of.remove(&untyped);
        self.parents_of.retain(|_, parents| {
            parents.retain(|(p, _)| *p != untyped);
            !parents.is_empty()
        });
    }

    /// Look up all parents of the given child id, deduplicated by parent id.
    pub fn parents_of(&self, child: UntypedAssetId) -> Vec<(UntypedAssetId, TypeId)> {
        self.parents_of.get(&child).cloned().unwrap_or_default()
    }
}

/// System: walk `AssetEvent<A>::Added/Modified` events and rebuild the
/// parent's dependency edges in [`DependencyGraph`].
///
/// Chained after `flush_debounced_refresh` so the snapshot is updated
/// before we re-walk dependencies (avoids one-frame staleness on circular
/// reads).
///
/// # Note
///
/// This only re-records edges for assets that receive Added/Modified; it
/// does **not** dispatch anything. Cascade dispatch happens separately in
/// `flush_debounced_refresh` (pushes to `CascadeQueue`) and
/// `cascade_dispatch_system` (drains the queue).
pub fn dependency_registry_system<A: HmrSource>(
    mut asset_events: MessageReader<AssetEvent<A>>,
    assets: Res<Assets<A>>,
    mut graph: ResMut<DependencyGraph>,
) {
    for evt in asset_events.read() {
        let id = match evt {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => *id,
            _ => continue,
        };
        let Some(asset) = assets.get(id) else {
            continue;
        };
        graph.rebuild_for(id, asset);
    }
}

/// System: walk `AssetEvent<A>::Removed` events and prune the asset's edges
/// from [`DependencyGraph`].
pub fn dependency_cleanup_system<A: HmrSource>(
    mut asset_events: MessageReader<AssetEvent<A>>,
    mut graph: ResMut<DependencyGraph>,
) {
    for evt in asset_events.read() {
        if let AssetEvent::Removed { id } = evt {
            graph.remove::<A>(*id);
        }
    }
}

/// System: drain [`CascadeQueue`] entries whose parent type matches `A`,
/// and dispatch one dependency-caused `ConfigRefresh<A::Config>` for each
/// parent asset that still exists.
///
/// Chained as the **last** system in the per-type pipeline (after
/// `flush_debounced_refresh` and `dependency_registry_system`), so the
/// cascade fires on the frame *after* the child was flushed.
///
/// # Subscriber semantics
///
/// The dispatched `ConfigRefresh<T>` has:
/// - `delta` / `changed_ids`: empty (the parent asset itself didn't change)
/// - `cause`: dependency with all triggering child asset ids
/// - `new_config`: the parent's *current* `Assets<A>` value (re-derived)
/// - `source_path`: the parent's source path (looked up via
///   `Assets<A>::get(id)` → `HmrSource::source_path`)
///
/// Subscribers detect a cascade via `RefreshCause::Dependency`. The parent
/// asset is guaranteed to still exist in `Assets<A>` when we read it (we
/// skip otherwise), so a despawned parent won't receive a stale event.
///
/// We batch-process all entries for type `A` per frame, then clear them
/// from the queue. Entries for other types remain for their own dispatch
/// system.
pub fn cascade_dispatch_system<A: HmrSource>(
    mut queue: ResMut<CascadeQueue>,
    assets: Res<Assets<A>>,
    cache: Res<crate::binding::HandleEntityCache<A>>,
    mut refresh_evts: MessageWriter<crate::refresh::ConfigRefresh<A::Config>>,
) {
    if queue.pending.is_empty() {
        return;
    }
    let target_type = TypeId::of::<A>();
    // Drain entries for our type, preserving entries for other types.
    // Deduplicate parent ids so a parent enqueued multiple times in one
    // frame (e.g. via both the removed- and modified-cascade paths) only
    // receives a single `ConfigRefresh`.
    let mut kept = Vec::new();
    let mut to_dispatch: HashMap<UntypedAssetId, HashSet<UntypedAssetId>> = HashMap::new();
    for entry in queue.pending.drain(..) {
        if entry.parent_type == target_type {
            to_dispatch
                .entry(entry.parent_id)
                .or_default()
                .insert(entry.triggered_by);
        } else {
            kept.push(entry);
        }
    }
    queue.pending = kept;

    for (parent_untyped, triggered_by) in to_dispatch {
        // Convert untyped -> typed (debug-checked in debug builds; silent
        // type mismatch in release means we just skip, which is safe).
        let parent_id: AssetId<A> = parent_untyped.typed_debug_checked::<A>();
        let Some(asset) = assets.get(parent_id) else {
            // Parent was removed; nothing to dispatch.
            continue;
        };
        let new_config = asset.config().clone();
        let source_path = asset.source_path().to_string();
        let target_entities: Vec<bevy::ecs::entity::Entity> = cache
            .get_entities(&parent_id)
            .map(|s| s.iter().copied().collect())
            .unwrap_or_default();

        refresh_evts.write(crate::refresh::ConfigRefresh {
            asset_id: parent_untyped,
            new_config,
            target_entities,
            changed_ids: Default::default(), // empty = cascade marker
            delta: Default::default(),
            diff_kind: crate::refresh::DiffKind::Modified,
            cause: crate::refresh::RefreshCause::Dependency {
                triggered_by: triggered_by.clone(),
            },
            source_path,
        });

        bevy::log::info!(
            "[HMR] cascade: parent={} triggered by child={:?}",
            std::any::type_name::<A>(),
            triggered_by,
        );
    }
}
