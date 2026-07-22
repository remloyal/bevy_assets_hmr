//! Debounce layer: batches rapid file-change events into a single refresh.
//!
//! When a file is saved, the OS / editor may emit multiple write events in
//! quick succession (e.g. atomic-write: delete + rename). Without debouncing,
//! each event would trigger a diff + refresh cycle, wasting work and
//! potentially causing visual flicker.
//!
//! The debouncer records the `Instant` of each `AssetEvent::Added` /
//! `AssetEvent::Modified` and only flushes a pending refresh once `window`
//! has elapsed with no new events for that asset id.
//!
//! # First-load behavior
//!
//! When an asset id has no prior snapshot (first ever load), the flush
//! initializes the snapshot from the newly loaded asset and dispatches
//! **no** `ConfigRefresh<A::Config>` event. This means callers using
//! `AssetServer::load` or `App::insert_config` for the initial load won't
//! see a spurious "first load" refresh - they only see refreshes when the
//! asset is subsequently modified.

use crate::binding::HandleEntityCache;
use crate::core::{HmrSource, LastSnapshot};
use crate::diff::ConfigDiff;
use crate::refresh::{ConfigRefresh, ConfigRemoved, DiffKind};
use bevy::asset::{AssetId, Assets};
use bevy::ecs::message::MessageWriter;
use bevy::prelude::{Res, ResMut, Resource};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Per-type debouncer. Holds a map of `AssetId<A> -> last event time` for
/// pending refreshes. [`flush_debounced_refresh`] runs every frame and
/// dispatches any whose `window` has elapsed.
#[derive(Resource)]
pub struct RefreshDebouncer<A: HmrSource> {
    /// Pending asset ids -> time of most recent event.
    pub pending: HashMap<AssetId<A>, Instant>,
    /// Pending removed asset ids (debounced like `pending`).
    ///
    /// Populated by `hmr_core_system` on `AssetEvent::Removed` and flushed
    /// by `flush_debounced_refresh` after the debounce window elapses,
    /// dispatching [`crate::ConfigRemoved`] events.
    pub pending_removed: HashMap<AssetId<A>, Instant>,
    /// Debounce window. Set by `App::register_config` from
    /// `ConfigHmrPlugin::debounce_window`.
    pub window: Duration,
}

impl<A: HmrSource> Default for RefreshDebouncer<A> {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
            pending_removed: HashMap::new(),
            window: Duration::from_millis(150),
        }
    }
}

/// Enqueue a modified asset for debounced refresh. Called by `hmr_core_system`.
pub(crate) fn enqueue<A: HmrSource>(debouncer: &mut RefreshDebouncer<A>, id: AssetId<A>) {
    // Last event wins within the debounce window. This handles editor atomic
    // saves, which commonly appear as Removed followed by Added.
    debouncer.pending_removed.remove(&id);
    debouncer.pending.insert(id, Instant::now());
}

/// Enqueue a removed asset for debounced `ConfigRemoved` dispatch.
/// Called by `hmr_core_system` on `AssetEvent::Removed`.
pub(crate) fn enqueue_removed<A: HmrSource>(debouncer: &mut RefreshDebouncer<A>, id: AssetId<A>) {
    // A later removal supersedes an earlier Added/Modified for the same id.
    debouncer.pending.remove(&id);
    debouncer.pending_removed.insert(id, Instant::now());
}

/// System: flush any pending refreshes whose debounce window has elapsed.
///
/// For each ready id:
/// 1. Look up the new `A` from `Assets`.
/// 2. Look up the previous `A::Config` from `LastSnapshot<A>`.
/// 3. If no prior snapshot exists (first load): initialize the snapshot
///    from the new asset and skip dispatch (no spurious "first load" event).
/// 4. Otherwise call `A::Config::diff(old, new)` to compute
///    `(added, removed, modified)`.
/// 5. If `changed_ids` is empty (content unchanged), skip dispatch.
/// 6. Update `LastSnapshot<A>` to the new version.
/// 7. Compute `target_entities` from `HandleEntityCache<A>`.
/// 8. Dispatch `ConfigRefresh<A::Config>`.
///
/// Also flushes `pending_removed`: for each removed id whose window has
/// elapsed, dispatches a [`crate::ConfigRemoved`] event (if a prior snapshot
/// existed) and cleans up the snapshot.
pub fn flush_debounced_refresh<A: HmrSource>(
    mut debouncer: ResMut<RefreshDebouncer<A>>,
    assets: Res<Assets<A>>,
    cache: Res<HandleEntityCache<A>>,
    mut snapshots: ResMut<LastSnapshot<A>>,
    mut refresh_evts: MessageWriter<ConfigRefresh<A::Config>>,
    mut removed_evts: MessageWriter<ConfigRemoved<A::Config>>,
    graph: Res<crate::dependency::DependencyGraph>,
    mut cascade_queue: ResMut<crate::dependency::CascadeQueue>,
) {
    let any_pending = !debouncer.pending.is_empty();
    let any_removed = !debouncer.pending_removed.is_empty();
    if !any_pending && !any_removed {
        return;
    }
    let window = debouncer.window;

    // ---- 1. Flush pending removals first (they take precedence: a
    // removed asset should not also produce a stale refresh). ----
    if any_removed {
        let ready_removed: Vec<_> = debouncer
            .pending_removed
            .iter()
            .filter(|(_, t)| {
                let elapsed = Instant::now().duration_since(**t);
                elapsed >= window
            })
            .map(|(id, _)| *id)
            .collect();

        for id in ready_removed {
            debouncer.pending_removed.remove(&id);
            // A removed asset should not also be pending a refresh.
            debouncer.pending.remove(&id);

            // Only dispatch ConfigRemoved if we actually had a snapshot
            // (i.e. the asset was known to subscribers). A remove event
            // for an asset we never saw is a no-op.
            let had_snapshot = snapshots.map.remove(&id).is_some();
            let source_path = snapshots.source_paths.remove(&id).unwrap_or_default();

            if !had_snapshot {
                continue;
            }

            let target_entities: Vec<_> = cache
                .get_entities(&id)
                .map(|s| s.iter().copied().collect())
                .unwrap_or_default();

            removed_evts.write(ConfigRemoved {
                target_entities,
                source_path,
                _marker: std::marker::PhantomData,
            });

            // ---- Dependency cascade on removal: enqueue this asset's
            // parents so they get re-dispatched on the next frame, so
            // subscribers can clean up any state derived from the now-
            // missing child. ----
            let parents = graph.parents_of(id.untyped());
            for (parent_untyped, parent_type) in parents {
                cascade_queue.pending.push((parent_untyped, parent_type));
            }
        }
    }

    // ---- 2. Flush pending modifications / additions. ----
    if any_pending {
        // Collect ids whose debounce window has elapsed.
        let ready: Vec<_> = debouncer
            .pending
            .iter()
            .filter(|(_, t)| {
                let elapsed = Instant::now().duration_since(**t);
                elapsed >= window
            })
            .map(|(id, _)| *id)
            .collect();

        for id in ready {
            debouncer.pending.remove(&id);

            let Some(new_asset) = assets.get(id) else {
                // Failures are reported by `asset_load_failed_system`; an
                // absent value here is a removal or stale asset event.
                continue;
            };

            let new_raw = new_asset.config();

            let (changed_ids, diff_kind) = match snapshots.map.get(&id) {
                Some(old_raw) => {
                    let (added, removed, modified) = A::Config::diff(old_raw, new_raw);
                    let kind = DiffKind::from_counts(added.len(), removed.len(), modified.len());
                    let mut ids = added;
                    ids.extend(removed);
                    ids.extend(modified);
                    (ids, kind)
                }
                None => {
                    // First load: initialize the snapshot and skip dispatch.
                    // Subscribers should not see a spurious "first load" refresh.
                    snapshots.map.insert(id, new_raw.clone());
                    snapshots
                        .source_paths
                        .insert(id, new_asset.source_path().to_string());
                    continue;
                }
            };

            // Update snapshot to the new version (regardless of whether we dispatch).
            snapshots.map.insert(id, new_raw.clone());
            snapshots
                .source_paths
                .insert(id, new_asset.source_path().to_string());

            // Skip dispatch if nothing actually changed (e.g. same content re-inserted).
            if changed_ids.is_empty() {
                continue;
            }

            // Collect target_entities from the cache (may be empty for the
            // data-table-via-Resource pattern).
            let target_entities: Vec<_> = cache
                .get_entities(&id)
                .map(|s| s.iter().copied().collect())
                .unwrap_or_default();

            let changed_count = changed_ids.len();
            let entity_count = target_entities.len();

            refresh_evts.write(ConfigRefresh {
                new_config: new_raw.clone(),
                target_entities,
                changed_ids,
                diff_kind,
                source_path: new_asset.source_path().to_string(),
            });

            // ---- Dependency cascade: enqueue this asset's parents so they
            // get re-dispatched on the next frame. ----
            let parents = graph.parents_of(id.untyped());
            for (parent_untyped, parent_type) in parents {
                cascade_queue.pending.push((parent_untyped, parent_type));
            }

            // info 日志：路径 + changed_ids 数 + 关联实体数
            bevy::log::info!(
                "[HMR] {} 已更新，changed_ids={}，关联实体数={}",
                new_asset.source_path(),
                changed_count,
                entity_count,
            );
        }
    }
}
