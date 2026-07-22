//! Diff trait for config types.
//!
//! Consumers implement [`ConfigDiff`] for each of their `XxxDatabase` Asset
//! types. The HMR core calls `<T as ConfigDiff>::diff(old, new)` on each
//! `AssetEvent::Modified` to determine which logical ids changed, then
//! broadcasts a [`crate::ConfigRefresh`] event carrying the `changed_ids`.
//!
//! For the common `Vec<Entry>` + `Entry.id: String` pattern, use the
//! `impl_config_diff!` macro to generate the implementation in one line.

use std::collections::{HashMap, HashSet};
use std::hash::Hash;

/// Added, removed, and modified id sets returned by [`ConfigDiff::diff`].
pub type ConfigDiffResult<Id> = (HashSet<Id>, HashSet<Id>, HashSet<Id>);

/// Trait implemented by config types to support diff-based refresh.
///
/// `diff` returns three id sets: `(added, removed, modified)`.
///
/// This trait intentionally does **not** require `Asset` — it only describes
/// how to compute an id-level delta between two values. The `Asset`
/// requirement is enforced separately by [`crate::HmrAsset`] / [`crate::HmrSource`]
/// where the type is actually used as a Bevy asset. This lets you implement
/// `ConfigDiff` on plain data structs (no `#[derive(Asset)]`) and reuse the
/// diff logic outside of Bevy's asset system if needed.
///
/// For the common `Vec<Entry>` pattern, use the `impl_config_diff!` macro
/// instead of implementing this manually.
///
/// # 如何选择实现方式
///
/// | 场景 | 实现方式 | 需要写 `type Id`？ |
/// |------|---------|-------------------|
/// | 单对象 / 枚举（整体比较） | [`SimpleConfigDiff`] | **否** |
/// | `Vec<Entry>` + `id: String` | `impl_config_diff!` 宏 | 否（宏生成） |
/// | `Vec<Entry>` + `id: String` | `#[derive(ConfigDiff)]` | 否（derive 生成） |
/// | `Vec<Entry>` + 非 String id（`u32`/`Uuid`） | `#[derive(ConfigDiff)]` + `id_type` | 否（derive 生成） |
/// | `Vec<Entry>` + 非 String id（`u32`/`Uuid`） | `impl_config_diff!` 宏 + 类型参数 | 否（宏生成） |
/// | 特殊 id 逻辑 | 手写 `impl ConfigDiff` | **是** |
///
/// 只有需要**完全自定义 diff 逻辑**时才需要手写 `type Id`。
pub trait ConfigDiff: Send + Sync + 'static {
    /// The logical id used to identify entries within this config.
    ///
    /// Set this to `String` for the common `id: String` database pattern,
    /// or to `u32`, `Uuid`, or any `Eq + Hash + Clone + Send + Sync + 'static`
    /// key to use a non-`String` primary key. The derive macro,
    /// `impl_config_diff!` macro, and [`SimpleConfigDiff`] blanket impl all
    /// default this to `String` for you - you only need to set it manually
    /// when using a non-`String` id type.
    type Id: Eq + Hash + Clone + Send + Sync + std::fmt::Debug + 'static;

    /// Compare two versions of this config and return the id delta.
    ///
    /// - `added`: ids present in `new` but not in `old`
    /// - `removed`: ids present in `old` but not in `new`
    /// - `modified`: ids present in both whose entries differ
    fn diff(old: &Self, new: &Self) -> ConfigDiffResult<Self::Id>;
}

/// Diff two entry slices using an indexed lookup by logical id.
///
/// The returned sets are `(added, removed, modified)`. Each slice is indexed
/// once, so the normal path is average O(n), including modified-entry checks.
/// IDs must be unique within each slice. When a duplicate is encountered the
/// duplicate id is logged and classified as modified, making the invalid
/// input observable without silently choosing an arbitrary entry as unchanged.
pub fn diff_entries_by_id<Entry, Id, F>(
    old: &[Entry],
    new: &[Entry],
    mut id_of: F,
) -> (HashSet<Id>, HashSet<Id>, HashSet<Id>)
where
    Entry: PartialEq,
    Id: Eq + Hash + Clone + Send + Sync + std::fmt::Debug + 'static,
    F: FnMut(&Entry) -> Id,
{
    let mut old_index: HashMap<Id, &Entry> = HashMap::with_capacity(old.len());
    let mut new_index: HashMap<Id, &Entry> = HashMap::with_capacity(new.len());
    let mut duplicate_ids = HashSet::new();

    for entry in old {
        let id = id_of(entry);
        if old_index.insert(id.clone(), entry).is_some() {
            duplicate_ids.insert(id);
        }
    }
    for entry in new {
        let id = id_of(entry);
        if new_index.insert(id.clone(), entry).is_some() {
            duplicate_ids.insert(id);
        }
    }

    let old_ids: HashSet<Id> = old_index.keys().cloned().collect();
    let new_ids: HashSet<Id> = new_index.keys().cloned().collect();
    let added = new_ids.difference(&old_ids).cloned().collect();
    let removed = old_ids.difference(&new_ids).cloned().collect();
    let mut modified: HashSet<Id> = old_index
        .keys()
        .filter_map(|id| match new_index.get(id) {
            Some(new_entry) if old_index[id] != *new_entry => Some(id.clone()),
            _ => None,
        })
        .collect();

    if !duplicate_ids.is_empty() {
        bevy::log::error!(
            "[HMR] duplicate config entry ids detected; treating them as modified: {:?}",
            duplicate_ids
        );
        modified.extend(duplicate_ids);
    }

    (added, removed, modified)
}

/// Simplified diff trait for **single-object** configs (one file = one value).
///
/// Implement this instead of [`ConfigDiff`] when you don't need id-level
/// diffing - e.g. a `UiTheme`, `GameSettings`, or an enum config. The whole
/// value is compared via `PartialEq`; if it changed, a single id (the type
/// name, or a custom string) is reported as "modified".
///
/// Any type implementing `SimpleConfigDiff` automatically gets a
/// [`ConfigDiff`] impl (with `type Id = String`) via blanket impl - **no
/// need to write `type Id = String;` yourself**.
///
/// # Example
///
/// ```
/// use bevy_assets_hmr::SimpleConfigDiff;
///
/// #[derive(PartialEq)]
/// struct UiTheme { bg_color: String }
/// impl SimpleConfigDiff for UiTheme {}
///
/// // Optionally override the id string:
/// #[derive(PartialEq)]
/// struct LevelConfig { difficulty: u32 }
/// impl SimpleConfigDiff for LevelConfig {
///     fn diff_id() -> &'static str { "level" }
/// }
/// ```
///
/// For `Vec<Entry>` databases with id-level diffing, use
/// `impl_config_diff!` or
/// `#[derive(ConfigDiff)]` instead.
pub trait SimpleConfigDiff: PartialEq + Send + Sync + 'static {
    /// The id string reported as "modified" when the value changes.
    ///
    /// Defaults to `std::any::type_name::<Self>()` (the fully-qualified Rust
    /// type path). Override to use a shorter/custom id.
    fn diff_id() -> &'static str {
        std::any::type_name::<Self>()
    }
}

/// Blanket impl: any `SimpleConfigDiff` type gets `ConfigDiff` for free,
/// with `type Id = String` and whole-value comparison via `PartialEq`.
impl<T: SimpleConfigDiff> ConfigDiff for T {
    type Id = String;
    fn diff(old: &Self, new: &Self) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
        if old != new {
            let mut modified = HashSet::new();
            modified.insert(T::diff_id().to_string());
            (HashSet::new(), HashSet::new(), modified)
        } else {
            (HashSet::new(), HashSet::new(), HashSet::new())
        }
    }
}

/// Implements [`ConfigDiff`] for a "`Vec<Entry>` wrapper" database type by
/// diffing on the `id` field of each entry.
///
/// Eliminates ~20 lines of boilerplate per database type. The entry type
/// must implement `PartialEq` (for modified detection); the `id` field
/// must be a `String`.
///
/// # Example
/// ```no_run
/// # use bevy::asset::Asset;
/// # use bevy::reflect::TypePath;
/// # use serde::{Deserialize, Serialize};
/// # #[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
/// # struct NpcDatabase {
/// #     npcs: Vec<NpcEntry>,
/// # }
/// # #[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
/// # struct NpcEntry {
/// #     id: String,
/// #     hp: u32,
/// # }
/// bevy_assets_hmr::impl_config_diff!(NpcDatabase, npcs, id);
/// ```
///
/// This is equivalent to:
/// ```ignore
/// impl ConfigDiff for NpcDatabase {
///     type Id = String;
///     fn diff(old: &Self, new: &Self) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
///         // 1. Collect id sets from old.npcs and new.npcs
///         // 2. added = new_ids - old_ids
///         // 3. removed = old_ids - new_ids
///         // 4. modified = ids in both where entry differs (by PartialEq)
///         // 5. return (added, removed, modified)
///     }
/// }
/// ```
///
/// # Non-String id type
///
/// For databases whose entry id is not a `String` (e.g. `u32`, `Uuid`),
/// pass the id type as a fourth argument:
///
/// ```no_run
/// # use bevy::asset::Asset;
/// # use bevy::reflect::TypePath;
/// # use serde::{Deserialize, Serialize};
/// # #[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
/// # struct MonsterDatabase {
/// #     entries: Vec<MonsterEntry>,
/// # }
/// # #[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
/// # struct MonsterEntry {
/// #     id: u32,
/// #     name: String,
/// # }
/// bevy_assets_hmr::impl_config_diff!(MonsterDatabase, entries, id, u32);
/// ```
///
/// For non-Vec patterns (e.g. a single config object like `UiTheme`),
/// implement [`ConfigDiff`] manually instead.
#[macro_export]
macro_rules! impl_config_diff {
    // With explicit id type (non-String ids like u32, Uuid, etc.)
    ($db:ty, $field:ident, $id_field:ident, $id_type:ty) => {
        impl $crate::ConfigDiff for $db {
            type Id = $id_type;
            fn diff(
                old: &Self,
                new: &Self,
            ) -> (
                std::collections::HashSet<$id_type>,
                std::collections::HashSet<$id_type>,
                std::collections::HashSet<$id_type>,
            ) {
                $crate::diff_entries_by_id(&old.$field, &new.$field, |entry| {
                    entry.$id_field.clone()
                })
            }
        }
    };
    // Default: id type is String (backward compatible)
    ($db:ty, $field:ident, $id_field:ident) => {
        $crate::impl_config_diff!($db, $field, $id_field, String);
    };
}
