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
    pub changed_ids: HashSet<String>,
    /// Coarse diff categorization.
    pub diff_kind: DiffKind,
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
