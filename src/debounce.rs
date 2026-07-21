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
use crate::refresh::{ConfigRefresh, DiffKind};
use bevy::asset::{Assets, AssetId};
use bevy::ecs::message::MessageWriter;
use bevy::prelude::{Res, ResMut, Resource, Time};
use std::collections::HashMap;
use std::time::{Duration, Instant};

/// Per-type debouncer. Holds a map of `AssetId<A> -> last event time` for
/// pending refreshes. [`flush_debounced_refresh`] runs every frame and
/// dispatches any whose `window` has elapsed.
#[derive(Resource)]
pub struct RefreshDebouncer<A: HmrSource> {
    /// Pending asset ids -> time of most recent event.
    pub pending: HashMap<AssetId<A>, Instant>,
    /// Debounce window. Set by `App::register_config` from
    /// `ConfigHmrPlugin::debounce_window`.
    pub window: Duration,
}

impl<A: HmrSource> Default for RefreshDebouncer<A> {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
            window: Duration::from_millis(150),
        }
    }
}

/// Enqueue a modified asset for debounced refresh. Called by `hmr_core_system`.
pub(crate) fn enqueue<A: HmrSource>(
    debouncer: &mut RefreshDebouncer<A>,
    id: AssetId<A>,
) {
    debouncer.pending.insert(id, Instant::now());
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
pub fn flush_debounced_refresh<A: HmrSource>(
    time: Res<Time>,
    mut debouncer: ResMut<RefreshDebouncer<A>>,
    assets: Res<Assets<A>>,
    cache: Res<HandleEntityCache<A>>,
    mut snapshots: ResMut<LastSnapshot<A>>,
    mut refresh_evts: MessageWriter<ConfigRefresh<A::Config>>,
) {
    if debouncer.pending.is_empty() {
        return;
    }
    let now = time.elapsed();
    let window = debouncer.window;

    // Collect ids whose debounce window has elapsed.
    let ready: Vec<_> = debouncer
        .pending
        .iter()
        .filter(|(_, t)| {
            let elapsed = Instant::now().duration_since(**t);
            elapsed >= window || now.as_secs_f64() == 0.0 // First-frame safety
        })
        .map(|(id, _)| *id)
        .collect();

    for id in ready {
        debouncer.pending.remove(&id);

        let Some(new_asset) = assets.get(id) else {
            // Asset was unloaded; drop the snapshot too.
            snapshots.map.remove(&id);
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
                continue;
            }
        };

        // Update snapshot to the new version (regardless of whether we dispatch).
        snapshots.map.insert(id, new_raw.clone());

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

        refresh_evts.write(ConfigRefresh {
            new_config: new_raw.clone(),
            target_entities,
            changed_ids,
            diff_kind,
        });
    }
}
