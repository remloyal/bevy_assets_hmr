//! Core HMR systems: asset event listening, snapshot management, cache validation.
//!
//! The core layer sits between the binding layer (which tracks which entities
//! depend on which config handles) and the event layer (which broadcasts
//! `ConfigRefresh<T>` to subscribers). It:
//!
//! 1. Listens for `AssetEvent::Added` and `AssetEvent::Modified` on
//!    `A` (where `A: HmrSource`). (`Assets::insert` triggers `Added` on
//!    first insert and `Modified` on subsequent ones; `AssetServer::load`
//!    triggers `Added` on first load.)
//! 2. Enqueues the modified asset id in the [`crate::RefreshDebouncer`].
//! 3. The debouncer's `flush_debounced_refresh` system then performs the
//!    actual diff + dispatch (see `debounce.rs`).
//!
//! The split lets `hmr_core_system` run cheaply every frame (just enqueue),
//! while the more expensive diff/dispatch only happens after the debounce
//! window elapses.
//!
//! # 两种模式
//!
//! - **包装模式**：`ConfigAsset<T>` impl `HmrSource`（框架自动做），
//!   `type Config = T`。用户用 `register_config::<T>()` 注册。
//! - **直接模式**：用户自己的 `Asset` 直接 impl `HmrSource`，
//!   用自己的 loader 加载，用自己的 `A::type_path()` 注册路径。

use crate::asset::ConfigAsset;
use crate::binding::HandleEntityCache;
use crate::debounce::{RefreshDebouncer, enqueue, enqueue_removed};
use crate::diff::ConfigDiff;
use bevy::asset::{Asset, AssetEvent, AssetId, AssetLoadFailedEvent};
use bevy::ecs::system::Query;
use bevy::prelude::{MessageReader, MessageWriter, Res, ResMut, Resource};
use serde::de::DeserializeOwned;
use std::collections::HashMap;

/// HMR 数据源：从 Bevy Asset 提取配置数据用于 diff。
///
/// - **包装模式**：`ConfigAsset<T>` impl `HmrSource`（框架自动做），
///   `type Config = T`。
/// - **直接模式**：`T` 本身 impl `HmrSource`（用户自定义 Asset 自己 impl），
///   `type Config = T`（或用户指定的 Config 类型）。
///
/// 泛型参数 `A` 是 "Asset"，`A::Config` 是 "Config 数据类型"（实现
/// [`ConfigDiff`]）。在包装模式下 `A = ConfigAsset<T>`，`A::Config = T`；
/// 在直接模式下 `A = T`（用户的 Asset），`A::Config` 通常是 `T` 自己。
pub trait HmrSource: Asset + Send + Sync + 'static {
    /// 配置数据类型（实现 [`ConfigDiff`]）。
    type Config: ConfigDiff + Clone + Send + Sync + 'static;
    /// 从 Asset 提取 Config 引用。
    fn config(&self) -> &Self::Config;
    /// 该资产对应的磁盘源路径（相对 `assets/`，如 `data/npc.ron`）。
    ///
    /// 用于在 [`crate::ConfigRefresh`] / [`crate::ConfigRemoved`] 事件中
    /// 携带 `source_path`，方便订阅方做日志/调试。包装模式
    ///（`ConfigAsset<T>`）自动返回 `&self.source_path`；直接模式下用户
    /// 可按需 override（默认返回空字符串）。
    fn source_path(&self) -> &str {
        ""
    }
}

/// Per-type snapshot of the last-seen `Config` value for each `AssetId<A>`.
///
/// - On first load (no prior snapshot), `flush_debounced_refresh` initializes
///   the snapshot from the newly loaded asset and dispatches **no** event.
/// - On subsequent modifications, the snapshot is used as the "old" version
///   for `A::Config::diff(old, new)` and is updated after dispatch.
#[derive(Resource)]
pub struct LastSnapshot<A: HmrSource> {
    /// `AssetId<A>` -> last-seen `A::Config` value.
    pub map: HashMap<AssetId<A>, A::Config>,
    /// `AssetId<A>` -> last-known source path.
    ///
    /// Recorded whenever a snapshot is initialized or updated, so that
    /// [`crate::ConfigRemoved`] events can still carry a `source_path`
    /// after the asset itself is gone.
    pub source_paths: HashMap<AssetId<A>, String>,
}

impl<A: HmrSource> Default for LastSnapshot<A> {
    fn default() -> Self {
        Self {
            map: HashMap::new(),
            source_paths: HashMap::new(),
        }
    }
}

/// System: listen for `AssetEvent::Added` / `AssetEvent::Modified` on
/// `A` and enqueue them in the per-type debouncer.
///
/// - `Added` fires when `Assets::insert` is called for a new id, or when
///   `AssetServer::load` finishes loading a file for the first time. The
///   debouncer will treat this as "first load" and initialize the snapshot
///   without dispatching a refresh.
/// - `Modified` fires when `Assets::insert` is called for an existing id,
///   or when `AssetServer` hot-reloads a changed file. The debouncer will
///   diff against the snapshot and dispatch `ConfigRefresh<A::Config>`.
///
/// Cheap (just hashmap inserts). Runs every frame, chained after
/// `config_binding_registry_system` and before `flush_debounced_refresh`.
pub fn hmr_core_system<A: HmrSource>(
    mut asset_events: MessageReader<AssetEvent<A>>,
    mut debouncer: ResMut<RefreshDebouncer<A>>,
) {
    for evt in asset_events.read() {
        match evt {
            AssetEvent::Added { id } | AssetEvent::Modified { id } => {
                enqueue::<A>(&mut debouncer, *id);
            }
            AssetEvent::Removed { id } => {
                // Asset was explicitly removed (e.g. file deleted, or
                // `Assets::remove` called). Enqueue a `ConfigRemoved`
                // dispatch so subscribers can do cleanup.
                enqueue_removed::<A>(&mut debouncer, *id);
            }
            _ => {}
        }
    }
}

/// Forward Bevy asset loading failures into the typed HMR failure message.
///
/// Bevy keeps the previously loaded asset when a hot reload fails. The HMR
/// layer reports the failure and last valid snapshot without writing an
/// artificial rollback asset back into `Assets<A>`.
pub fn asset_load_failed_system<A: HmrSource>(
    mut failures: MessageReader<AssetLoadFailedEvent<A>>,
    snapshots: Res<LastSnapshot<A>>,
    cache: Res<HandleEntityCache<A>>,
    mut revisions: ResMut<crate::view::AssetRevision<A>>,
    mut failed_messages: MessageWriter<crate::refresh::ConfigReloadFailed<A::Config>>,
) {
    for failure in failures.read() {
        let current_config = snapshots.map.get(&failure.id).cloned();
        let source_path = snapshots
            .source_paths
            .get(&failure.id)
            .cloned()
            .unwrap_or_else(|| failure.path.to_string());
        let target_entities = cache
            .get_entities(&failure.id)
            .map(|entities| entities.iter().copied().collect())
            .unwrap_or_default();
        let error = failure.error.to_string();

        revisions.record_failed(failure.id, source_path.clone(), error.clone());

        failed_messages.write(crate::refresh::ConfigReloadFailed {
            asset_id: failure.id.untyped(),
            source_path,
            error,
            target_entities,
            current_config,
        });
    }
}

/// System: periodic cache validation (drift correction).
///
/// Runs every 30 seconds (via `on_timer`). Walks all entities that currently
/// have a `ConfigBind<A>` component and ensures the cache reflects truth.
///
/// This is a **safety net**, not the primary update path. The hot path is:
/// - `config_binding_registry_system` — incremental `Changed<ConfigBind<A>>`
/// - `config_binding_cleanup_system` — incremental `RemovedComponents`
///
/// This system only catches drift caused by direct mutation of
/// `ConfigBind<A>` outside those paths (e.g. `world.entity_mut(e).insert(..)`
/// bypassing `Changed` detection in rare edge cases). The 30 s interval keeps
/// the O(n) full scan cost low while bounding worst-case drift duration.
pub fn cache_validation_system<A: HmrSource>(
    mut cache: ResMut<HandleEntityCache<A>>,
    bindings: Query<(bevy::prelude::Entity, &crate::binding::ConfigBind<A>)>,
) {
    let mut new_cache = HandleEntityCache::<A>::default();
    for (entity, bind) in bindings.iter() {
        new_cache.insert(entity, bind.handle.id());
    }
    if cache.entity_to_handle != new_cache.entity_to_handle {
        *cache = new_cache;
    }
}

/// Convenience trait alias: any `Asset` that also implements [`ConfigDiff`]
/// and `DeserializeOwned` can be hot-reloaded by the HMR core via the
/// wrapping `ConfigAsset<T>` pattern (using `ConfigLoader<T>`).
///
/// For the "direct mode" pattern (user's own `Asset` + loader), implement
/// [`HmrSource`] directly on your asset type instead.
pub trait HmrAsset: Asset + ConfigDiff + Clone + DeserializeOwned + Send + Sync + 'static {}

impl<T> HmrAsset for T where T: Asset + ConfigDiff + Clone + DeserializeOwned + Send + Sync + 'static
{}

/// 包装模式：`ConfigAsset<T>` 自动实现 `HmrSource`，配置类型就是 `T` 本身。
impl<T: HmrAsset> HmrSource for ConfigAsset<T> {
    type Config = T;
    fn config(&self) -> &T {
        &self.raw
    }
    fn source_path(&self) -> &str {
        &self.source_path
    }
}
