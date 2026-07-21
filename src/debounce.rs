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
use crate::refresh::{ConfigRefresh, ConfigReloadFailed, ConfigRemoved, DiffKind};
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
    /// 每个 id 上次被 flush 的时间（用于死循环保护 cooldown）。
    ///
    /// flush 成功后记录 `Instant::now()`；cooldown 时间内同一 id 的新事件
    /// 会被 `enqueue` 直接跳过，防止「重载回调写文件 → file_watcher 触发
    /// → 再次 enqueue → flush → 写文件 → …」无限循环。
    pub flushed_at: HashMap<AssetId<A>, Instant>,
    /// Cooldown 窗口：flush 后在此时间内不再处理同一 id 的新事件。
    /// 默认 500ms，区分于 debounce 窗口（debounce 是合并短时间多次写入，
    /// cooldown 是防止写回触发的递归重入）。
    pub cooldown: Duration,
}

impl<A: HmrSource> Default for RefreshDebouncer<A> {
    fn default() -> Self {
        Self {
            pending: HashMap::new(),
            pending_removed: HashMap::new(),
            window: Duration::from_millis(150),
            flushed_at: HashMap::new(),
            cooldown: Duration::from_millis(500),
        }
    }
}

/// Enqueue a modified asset for debounced refresh. Called by `hmr_core_system`.
///
/// 如果该 id 刚被 flush 且在 cooldown 窗口内，直接跳过——防止「重载回调写文件
/// → file_watcher 触发 → 再次 enqueue」的无限循环。
pub(crate) fn enqueue<A: HmrSource>(debouncer: &mut RefreshDebouncer<A>, id: AssetId<A>) {
    // Cooldown 检查：防止写回死循环
    if let Some(last_flush) = debouncer.flushed_at.get(&id) {
        if last_flush.elapsed() < debouncer.cooldown {
            bevy::log::debug!(
                "[HMR] {} skipped: within cooldown window ({}ms since last flush)",
                A::type_path(),
                last_flush.elapsed().as_millis(),
            );
            return;
        }
    }
    debouncer.pending.insert(id, Instant::now());
}

/// Enqueue a removed asset for debounced `ConfigRemoved` dispatch.
/// Called by `hmr_core_system` on `AssetEvent::Removed`.
pub(crate) fn enqueue_removed<A: HmrSource>(debouncer: &mut RefreshDebouncer<A>, id: AssetId<A>) {
    debouncer.pending_removed.insert(id, Instant::now());
}

/// System: flush any pending refreshes whose debounce window has elapsed.
///
/// 注意：`assets` 参数从 `Res<Assets<A>>` 改为 `ResMut<Assets<A>>`，
/// 因为回滚逻辑需要在 `assets.get(id)` 失败时执行 `assets.insert(id, ...)`
/// 将旧快照重建的资产插回。这会与所有读 `Assets<A>` 的系统串行化——
/// 对于注册了大量 asset 类型的应用可能有微小的帧时间影响，但回滚是
/// 低频事件（仅在文件损坏/语法错误时触发），实际影响可忽略。
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
    mut assets: ResMut<Assets<A>>,
    cache: Res<HandleEntityCache<A>>,
    mut snapshots: ResMut<LastSnapshot<A>>,
    mut refresh_evts: MessageWriter<ConfigRefresh<A::Config>>,
    mut removed_evts: MessageWriter<ConfigRemoved<A::Config>>,
    mut failed_evts: MessageWriter<ConfigReloadFailed<A::Config>>,
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
                // --- 回滚：新版本加载失败，用旧 snapshot 重建 Asset 并插回 ---
                if let Some(old_config) = snapshots.map.get(&id).cloned() {
                    let source_path = snapshots.source_paths.get(&id).cloned().unwrap_or_default();

                    // 用旧 snapshot 重建 Asset 并插回 Assets
                    let rolled_back = A::from_config(old_config.clone(), source_path.clone());
                    let _ = assets.insert(id, rolled_back);

                    // 收集关联实体
                    let target_entities: Vec<_> = cache
                        .get_entities(&id)
                        .map(|s| s.iter().copied().collect())
                        .unwrap_or_default();

                    // 派发失败事件（包含 source_path 以便定位问题文件）
                    failed_evts.write(ConfigReloadFailed {
                        source_path: source_path.clone(),
                        error: format!(
                            "Asset corrupted or failed to load for '{}'; rolled back to previous version",
                            source_path
                        ),
                        target_entities,
                        current_config: old_config,
                    });

                    // 记录 flush 时间（cooldown 保护）
                    debouncer.flushed_at.insert(id, Instant::now());
                } else {
                    // 无旧版本可回滚：清理 snapshot
                    snapshots.map.remove(&id);
                    snapshots.source_paths.remove(&id);
                }
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

            // 记录 flush 时间（cooldown 保护，防止写回死循环）
            debouncer.flushed_at.insert(id, Instant::now());

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
