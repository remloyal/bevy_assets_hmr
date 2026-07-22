//! Refresh event types.
//!
//! [`ConfigRefresh<T>`] is dispatched by the HMR core after a config Asset
//! has been modified and the debounce window has elapsed. Subscribers
//! consume it via `MessageReader<ConfigRefresh<T>>` and filter by `asset_id`,
//! structured `delta`, or `target_entities` for `ConfigBind<A>` users.
//!
//! 注意：这里的 `T` 是 **Config 数据类型**（实现 [`crate::ConfigDiff`]），
//! 不是 Asset 包装类型。在包装模式下 `T` 是 `ConfigAsset<X>::Config` = `X`
//! 本身；在直接模式下 `T` 是用户 Asset 自己（或其 `HmrSource::Config`）。

use crate::diff::ConfigDiff;
use bevy::asset::UntypedAssetId;
use bevy::ecs::message::Message;
use std::collections::HashSet;

/// Structured id-level changes between two config versions.
///
/// ```
/// use bevy_assets_hmr::ConfigDelta;
///
/// let delta = ConfigDelta {
///     added: ["new".to_string()].into_iter().collect(),
///     removed: ["old".to_string()].into_iter().collect(),
///     modified: ["changed".to_string()].into_iter().collect(),
/// };
/// assert_eq!(delta.changed_ids().len(), 3);
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConfigDelta<Id: Eq + std::hash::Hash> {
    pub added: HashSet<Id>,
    pub removed: HashSet<Id>,
    pub modified: HashSet<Id>,
}

impl<Id: Eq + std::hash::Hash> Default for ConfigDelta<Id> {
    fn default() -> Self {
        Self {
            added: HashSet::new(),
            removed: HashSet::new(),
            modified: HashSet::new(),
        }
    }
}

impl<Id: Eq + std::hash::Hash + Clone> ConfigDelta<Id> {
    /// Return the union of all changed ids for compatibility with consumers
    /// that do not need to distinguish added, removed, and modified entries.
    pub fn changed_ids(&self) -> HashSet<Id> {
        self.added
            .iter()
            .chain(&self.removed)
            .chain(&self.modified)
            .cloned()
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && self.modified.is_empty()
    }
}

/// Why a [`ConfigRefresh`] was dispatched.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RefreshCause {
    /// The asset's own content changed.
    Direct,
    /// One or more child assets changed or were removed.
    Dependency {
        triggered_by: HashSet<UntypedAssetId>,
    },
    /// A subscriber explicitly requested a refresh.
    Manual,
    /// A previously failed source became valid again.
    Recovery,
}

/// Event dispatched when a config of type `T` has been hot-reloaded.
///
/// `T` 是 Config 数据类型（实现 [`ConfigDiff`]），不是 Asset 包装类型。
///
/// Carries enough information for subscribers to do targeted refresh:
/// - `asset_id`: identifies the exact asset when one config type has several
///   loaded files.
/// - `new_config`: the freshly loaded config value (cloned — subscribers
///   that only need `changed_ids` can ignore this field).
/// - `target_entities`: entities that had `ConfigBind<A>` attached to
///   this asset handle at the time of the refresh. Empty for the
///   "data table via Resource" pattern.
/// - `changed_ids`: ids that were added, removed, or modified relative
///   to the previous version. Subscribers intersect this with their own
///   per-entity id field to decide which entities to touch.
/// - `delta`: the same ids partitioned into added, removed, and modified.
/// - `cause`: distinguishes direct, dependency, manual, and recovery refresh.
/// - `diff_kind`: coarse categorization of the diff, useful for choosing
///   between component-level and entity-level refresh strategies.
#[derive(Message, Clone)]
pub struct ConfigRefresh<T: ConfigDiff + Clone + Send + Sync + 'static> {
    /// Stable id of the config asset instance that changed.
    pub asset_id: UntypedAssetId,
    /// The freshly loaded config value (the `A::Config`, not the Asset).
    pub new_config: T,
    /// Entities bound to this handle via `ConfigBind<A>`.
    pub target_entities: Vec<bevy::ecs::entity::Entity>,
    /// Ids that changed (added ∪ removed ∪ modified).
    ///
    /// Kept for source compatibility; new consumers should prefer `delta`.
    pub changed_ids: HashSet<T::Id>,
    /// Added, removed, and modified ids kept as separate sets.
    pub delta: ConfigDelta<T::Id>,
    /// Coarse diff categorization.
    pub diff_kind: DiffKind,
    /// Direct, dependency, manual, or recovery refresh origin.
    pub cause: RefreshCause,
    /// 源文件路径（相对 `assets/`，如 `data/npc.ron`）。
    ///
    /// 直接模式下若 `HmrSource::source_path` 未 override，则为空字符串。
    pub source_path: String,
}

/// Event dispatched when a config asset of type `T` has been **removed**
/// (e.g. the backing file was deleted, or `Assets::remove` / asset unload
/// dropped it).
///
/// `T` 是 Config 数据类型（与 [`ConfigRefresh<T>`] 相同）。
///
/// Subscribers can use this to do cleanup work (despawn dependent entities,
/// free resources, show a "missing data" placeholder, etc.).
///
/// 与 `ConfigRefresh` 的区别：
/// - 资产已被卸载，无法提供 `new_config` 或 `changed_ids`；
/// - `target_entities` 从 [`HandleEntityCache<A>`] 在派发瞬间快照取得
///   （派发后这些绑定仍存在，由订阅方决定是否 despawn 对应实体）；
/// - `source_path` 来自资产被移除前记录的值（若资产从未加载则为空）。
#[derive(Message, Clone)]
pub struct ConfigRemoved<T: ConfigDiff + Clone + Send + Sync + 'static> {
    /// Stable id of the removed config asset instance.
    pub asset_id: UntypedAssetId,
    /// 在资产被删除时，之前绑定到此资产的实体集合（快照）。
    pub target_entities: Vec<bevy::ecs::entity::Entity>,
    /// 源文件路径（若删除前有记录）。可能为空字符串。
    pub source_path: String,
    /// 被删除资产的逻辑占位类型，用于让泛型 `T` 参与类型推断。
    pub _marker: std::marker::PhantomData<T>,
}

/// Event emitted when Bevy fails to load or hot-reload a config asset.
///
/// Bevy keeps the previous asset value when a hot reload fails. If the asset
/// had a valid snapshot, `current_config` contains that last valid value. It
/// is `None` when the initial load failed before any snapshot existed.
///
/// `T` 是 Config 数据类型（与 [`ConfigRefresh<T>`] 相同）。
#[derive(Message, Clone)]
pub struct ConfigReloadFailed<T: ConfigDiff + Clone + Send + Sync + 'static> {
    /// Stable id of the asset whose load failed.
    pub asset_id: bevy::asset::UntypedAssetId,
    /// 源文件路径（相对 `assets/`，如 `data/npc.ron`）。
    pub source_path: String,
    /// 错误描述（来自 AssetLoader 的错误信息或框架内部判断）。
    pub error: String,
    /// 加载失败时仍绑定到此资产的实体（从 HandleEntityCache 快照取得）。
    pub target_entities: Vec<bevy::ecs::entity::Entity>,
    /// Last valid config, if this was a failed reload rather than first load.
    pub current_config: Option<T>,
}

/// Coarse categorization of the diff result.
///
/// Subscribers can use this to pick a refresh strategy:
/// - `Modified` → component-level refresh (e.g. update `Text2d` only)
/// - `Added` / `Removed` / `Mixed` → entity-level refresh (despawn + respawn)
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiffKind {
    /// All changes are additions of new ids.
    Added,
    /// All changes are removals of existing ids.
    Removed,
    /// All changes are in-place modifications of existing ids.
    Modified,
    /// Mix of the above.
    Mixed,
}

impl DiffKind {
    /// Compute the `DiffKind` from the three id-set sizes.
    pub fn from_counts(added: usize, removed: usize, modified: usize) -> Self {
        match (added > 0, removed > 0, modified > 0) {
            (true, false, false) => Self::Added,
            (false, true, false) => Self::Removed,
            (false, false, true) => Self::Modified,
            _ => Self::Mixed,
        }
    }
}
