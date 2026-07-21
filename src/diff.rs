//! Diff trait for config types.
//!
//! Consumers implement [`ConfigDiff`] for each of their `XxxDatabase` Asset
//! types. The HMR core calls `<T as ConfigDiff>::diff(old, new)` on each
//! `AssetEvent::Modified` to determine which logical ids changed, then
//! broadcasts a [`crate::ConfigRefresh`] event carrying the `changed_ids`.
//!
//! For the common `Vec<Entry>` + `Entry.id: String` pattern, use the
//! [`impl_config_diff!`] macro to generate the implementation in one line.

use bevy::asset::Asset;
use std::collections::HashSet;

/// Trait implemented by config Asset types to support diff-based refresh.
///
/// `diff` returns three id sets: `(added, removed, modified)`.
///
/// For the common `Vec<Entry>` pattern, use the [`impl_config_diff!`] macro
/// instead of implementing this manually.
pub trait ConfigDiff: Asset + Send + Sync + 'static {
    /// Compare two versions of this config and return the id delta.
    ///
    /// - `added`: ids present in `new` but not in `old`
    /// - `removed`: ids present in `old` but not in `new`
    /// - `modified`: ids present in both whose entries differ
    fn diff(old: &Self, new: &Self) -> (HashSet<String>, HashSet<String>, HashSet<String>);
}

/// Implements [`ConfigDiff`] for a "Vec<Entry> wrapper" database type by
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
/// For non-Vec patterns (e.g. a single config object like `UiTheme`),
/// implement [`ConfigDiff`] manually instead.
#[macro_export]
macro_rules! impl_config_diff {
    ($db:ty, $field:ident, $id_field:ident) => {
        impl $crate::ConfigDiff for $db {
            fn diff(
                old: &Self,
                new: &Self,
            ) -> (
                std::collections::HashSet<String>,
                std::collections::HashSet<String>,
                std::collections::HashSet<String>,
            ) {
                use std::collections::HashSet;
                let old_ids: HashSet<String> =
                    old.$field.iter().map(|e| e.$id_field.clone()).collect();
                let new_ids: HashSet<String> =
                    new.$field.iter().map(|e| e.$id_field.clone()).collect();
                let added: HashSet<String> =
                    new_ids.difference(&old_ids).cloned().collect();
                let removed: HashSet<String> =
                    old_ids.difference(&new_ids).cloned().collect();
                let modified: HashSet<String> = old_ids
                    .intersection(&new_ids)
                    .filter(|id| {
                        old.$field.iter().find(|e| &e.$id_field == *id)
                            != new.$field.iter().find(|e| &e.$id_field == *id)
                    })
                    .cloned()
                    .collect();
                (added, removed, modified)
            }
        }
    };
}
