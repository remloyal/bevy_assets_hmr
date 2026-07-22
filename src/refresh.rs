//! Refresh event types.
//!
//! [`ConfigRefresh<T>`] is dispatched by the HMR core after a config Asset
//! has been modified and the debounce window has elapsed. Subscribers
//! consume it via `MessageReader<ConfigRefresh<T>>` and filter by
//! `changed_ids` (or `target_entities` for `ConfigBind<A>` users).
//!
//! 注意：这里的 `T` 是 **Config 数据类型**（实现 [`crate::ConfigDiff`]），
//! 不是 Asset 包装类型。在包装模式下 `T` 是 `ConfigAsset<X>::Config` = `X`
//! 本身；在直接模式下 `T` 是用户 Asset 自己（或其 `HmrSource::Config`）。

use crate::diff::ConfigDiff;
use bevy::ecs::message::Message;
use std::collections::HashSet;

/// Event dispatched when a config of type `T` has been hot-reloaded.
///
/// `T` 是 Config 数据类型（实现 [`ConfigDiff`]），不是 Asset 包装类型。
///
/// Carries enough information for subscribers to do targeted refresh:
/// - `new_config`: the freshly loaded config value (cloned — subscribers
///   that only need `changed_ids` can ignore this field).
/// - `target_entities`: entities that had `ConfigBind<A>` attached to
///   this asset handle at the time of the refresh. Empty for the
///   "data table via Resource" pattern.
/// - `changed_ids`: ids that were added, removed, or modified relative
///   to the previous version. Subscribers intersect this with their own
///   per-entity id field to decide which entities to touch.
/// - `diff_kind`: coarse categorization of the diff, useful for choosing
///   between component-level and entity-level refresh strategies.
#[derive(Message, Clone)]
pub struct ConfigRefresh<T: ConfigDiff + Clone + Send + Sync + 'static> {
    /// The freshly loaded config value (the `A::Config`, not the Asset).
    pub new_config: T,
    /// Entities bound to this handle via `ConfigBind<A>`.
    pub target_entities: Vec<bevy::ecs::entity::Entity>,
    /// Ids that changed (added ∪ removed ∪ modified).
    pub changed_ids: HashSet<T::Id>,
    /// Coarse diff categorization.
    pub diff_kind: DiffKind,
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
    /// 在资产被删除时，之前绑定到此资产的实体集合（快照）。
    pub target_entities: Vec<bevy::ecs::entity::Entity>,
    /// 源文件路径（若删除前有记录）。可能为空字符串。
    pub source_path: String,
    /// 被删除资产的逻辑占位类型，用于让泛型 `T` 参与类型推断。
    pub _marker: std::marker::PhantomData<T>,
}

impl<T: ConfigDiff + Clone + Send + Sync + 'static> Default for ConfigRemoved<T> {
    fn default() -> Self {
        Self {
            target_entities: Vec::new(),
            source_path: String::new(),
            _marker: std::marker::PhantomData,
        }
    }
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
