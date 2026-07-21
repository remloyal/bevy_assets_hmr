//! Tests for the user-facing `ConfigDiff` trait.
//!
//! Validates that a consuming crate can implement `ConfigDiff` for a
//! `Vec<Entry>`-style database Asset, and that the diff produces correct
//! (added, removed, modified) id sets.

use bevy::asset::Asset;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{ConfigDiff, SimpleConfigDiff};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Simplified test database that mimics zz_plot's `Vec<Entry>` pattern.
#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
struct TestDatabase {
    entries: Vec<TestEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
struct TestEntry {
    id: String,
    value: u32,
}

impl TestDatabase {
    fn new(entries: Vec<TestEntry>) -> Self {
        Self { entries }
    }

    fn ids(&self) -> HashSet<String> {
        self.entries.iter().map(|e| e.id.clone()).collect()
    }

    fn lookup(&self, id: &str) -> Option<&TestEntry> {
        self.entries.iter().find(|e| e.id == id)
    }
}

/// Reference diff algorithm: matches the macro the consuming crate would use.
fn diff_database(
    old: &TestDatabase,
    new: &TestDatabase,
) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
    let old_ids = old.ids();
    let new_ids = new.ids();

    let added: HashSet<String> = new_ids.difference(&old_ids).cloned().collect();
    let removed: HashSet<String> = old_ids.difference(&new_ids).cloned().collect();

    let modified: HashSet<String> = old_ids
        .intersection(&new_ids)
        .filter(|id| old.lookup(id) != new.lookup(id))
        .cloned()
        .collect();

    (added, removed, modified)
}

impl ConfigDiff for TestDatabase {
    type Id = String;
    fn diff(old: &Self, new: &Self) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
        diff_database(old, new)
    }
}

#[test]
fn diff_empty_to_empty_yields_all_empty() {
    let old = TestDatabase::new(vec![]);
    let new = TestDatabase::new(vec![]);
    let (a, r, m) = TestDatabase::diff(&old, &new);
    assert!(a.is_empty());
    assert!(r.is_empty());
    assert!(m.is_empty());
}

#[test]
fn diff_added_ids_are_returned_in_added_set() {
    let old = TestDatabase::new(vec![TestEntry {
        id: "a".into(),
        value: 1,
    }]);
    let new = TestDatabase::new(vec![
        TestEntry {
            id: "a".into(),
            value: 1,
        },
        TestEntry {
            id: "b".into(),
            value: 2,
        },
    ]);
    let (added, removed, modified) = TestDatabase::diff(&old, &new);
    assert_eq!(added, ["b".to_string()].into_iter().collect());
    assert!(removed.is_empty());
    assert!(modified.is_empty());
}

#[test]
fn diff_removed_ids_are_returned_in_removed_set() {
    let old = TestDatabase::new(vec![
        TestEntry {
            id: "a".into(),
            value: 1,
        },
        TestEntry {
            id: "b".into(),
            value: 2,
        },
    ]);
    let new = TestDatabase::new(vec![TestEntry {
        id: "a".into(),
        value: 1,
    }]);
    let (added, removed, modified) = TestDatabase::diff(&old, &new);
    assert!(added.is_empty());
    assert_eq!(removed, ["b".to_string()].into_iter().collect());
    assert!(modified.is_empty());
}

#[test]
fn diff_value_change_is_classified_as_modified() {
    let old = TestDatabase::new(vec![TestEntry {
        id: "a".into(),
        value: 1,
    }]);
    let new = TestDatabase::new(vec![TestEntry {
        id: "a".into(),
        value: 99,
    }]);
    let (added, removed, modified) = TestDatabase::diff(&old, &new);
    assert!(added.is_empty());
    assert!(removed.is_empty());
    assert_eq!(modified, ["a".to_string()].into_iter().collect());
}

#[test]
fn diff_mixed_changes_are_partitioned_correctly() {
    let old = TestDatabase::new(vec![
        TestEntry {
            id: "keep".into(),
            value: 1,
        },
        TestEntry {
            id: "remove".into(),
            value: 2,
        },
        TestEntry {
            id: "modify".into(),
            value: 3,
        },
    ]);
    let new = TestDatabase::new(vec![
        TestEntry {
            id: "keep".into(),
            value: 1,
        },
        TestEntry {
            id: "modify".into(),
            value: 30,
        },
        TestEntry {
            id: "add".into(),
            value: 4,
        },
    ]);
    let (added, removed, modified) = TestDatabase::diff(&old, &new);
    assert_eq!(added, ["add".to_string()].into_iter().collect());
    assert_eq!(removed, ["remove".to_string()].into_iter().collect());
    assert_eq!(modified, ["modify".to_string()].into_iter().collect());
}

#[test]
fn diff_unchanged_entries_are_not_reported() {
    let entry = TestEntry {
        id: "x".into(),
        value: 42,
    };
    let old = TestDatabase::new(vec![entry.clone()]);
    let new = TestDatabase::new(vec![entry]);
    let (a, r, m) = TestDatabase::diff(&old, &new);
    assert!(a.is_empty());
    assert!(r.is_empty());
    assert!(m.is_empty());
}

// ---- Problem 5: non-String Id type (u32) ----

/// A database keyed by `u32` ids, to verify the `ConfigDiff::Id` associated
/// type supports non-`String` keys.
#[derive(Clone, Debug, PartialEq, Default)]
struct U32Database {
    entries: Vec<U32Entry>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct U32Entry {
    id: u32,
    value: u32,
}

impl U32Database {
    fn new(entries: Vec<U32Entry>) -> Self {
        Self { entries }
    }
}

impl ConfigDiff for U32Database {
    type Id = u32;
    fn diff(old: &Self, new: &Self) -> (HashSet<u32>, HashSet<u32>, HashSet<u32>) {
        let old_ids: HashSet<u32> = old.entries.iter().map(|e| e.id).collect();
        let new_ids: HashSet<u32> = new.entries.iter().map(|e| e.id).collect();
        let added: HashSet<u32> = new_ids.difference(&old_ids).copied().collect();
        let removed: HashSet<u32> = old_ids.difference(&new_ids).copied().collect();
        let modified: HashSet<u32> = old_ids
            .intersection(&new_ids)
            .filter(|id| {
                old.entries.iter().find(|e| &e.id == *id)
                    != new.entries.iter().find(|e| &e.id == *id)
            })
            .copied()
            .collect();
        (added, removed, modified)
    }
}

#[test]
fn diff_with_u32_id_type_works() {
    let old = U32Database::new(vec![
        U32Entry { id: 1, value: 10 },
        U32Entry { id: 2, value: 20 },
        U32Entry { id: 3, value: 30 },
    ]);
    let new = U32Database::new(vec![
        U32Entry { id: 1, value: 10 }, // unchanged
        U32Entry { id: 2, value: 99 }, // modified
        U32Entry { id: 4, value: 40 }, // added
                                       // id 3 removed
    ]);
    let (added, removed, modified) = U32Database::diff(&old, &new);
    assert_eq!(added, [4u32].into_iter().collect());
    assert_eq!(removed, [3u32].into_iter().collect());
    assert_eq!(modified, [2u32].into_iter().collect());
}

// ---- Problem 9: derive(ConfigDiff) on enums ----

#[derive(Clone, Debug, PartialEq, ConfigDiff)]
enum GameModeConfig {
    Easy {
        damage_multiplier: f32,
    },
    Hard {
        damage_multiplier: f32,
        enemy_count: u32,
    },
}

#[test]
fn derive_config_diff_on_enum_reports_modified_when_changed() {
    let old = GameModeConfig::Easy {
        damage_multiplier: 1.0,
    };
    let new = GameModeConfig::Hard {
        damage_multiplier: 2.0,
        enemy_count: 10,
    };
    let (added, removed, modified) = GameModeConfig::diff(&old, &new);
    assert!(added.is_empty());
    assert!(removed.is_empty());
    assert_eq!(
        modified,
        ["GameModeConfig".to_string()].into_iter().collect()
    );
}

#[test]
fn derive_config_diff_on_enum_reports_empty_when_unchanged() {
    let old = GameModeConfig::Easy {
        damage_multiplier: 1.0,
    };
    let new = GameModeConfig::Easy {
        damage_multiplier: 1.0,
    };
    let (added, removed, modified) = GameModeConfig::diff(&old, &new);
    assert!(added.is_empty());
    assert!(removed.is_empty());
    assert!(modified.is_empty());
}

// ---- SimpleConfigDiff: blanket impl, no `type Id` needed ----

#[derive(Clone, Debug, PartialEq)]
struct SimpleTheme {
    bg_color: String,
}

impl SimpleConfigDiff for SimpleTheme {
    fn diff_id() -> &'static str {
        "theme"
    }
}

#[derive(Clone, Debug, PartialEq)]
struct DefaultIdConfig {
    value: i32,
}

impl SimpleConfigDiff for DefaultIdConfig {}

#[test]
fn simple_config_diff_reports_modified_when_changed() {
    let old = SimpleTheme {
        bg_color: "red".into(),
    };
    let new = SimpleTheme {
        bg_color: "blue".into(),
    };
    let (added, removed, modified) = SimpleTheme::diff(&old, &new);
    assert!(added.is_empty());
    assert!(removed.is_empty());
    assert_eq!(modified, ["theme".to_string()].into_iter().collect());
}

#[test]
fn simple_config_diff_reports_empty_when_unchanged() {
    let old = SimpleTheme {
        bg_color: "red".into(),
    };
    let new = SimpleTheme {
        bg_color: "red".into(),
    };
    let (added, removed, modified) = SimpleTheme::diff(&old, &new);
    assert!(added.is_empty());
    assert!(removed.is_empty());
    assert!(modified.is_empty());
}

#[test]
fn simple_config_diff_defaults_diff_id_to_type_name() {
    // When `diff_id()` is not overridden, the id should be the type name.
    let old = DefaultIdConfig { value: 1 };
    let new = DefaultIdConfig { value: 2 };
    let (_, _, modified) = DefaultIdConfig::diff(&old, &new);
    assert_eq!(modified.len(), 1);
    let id = modified.iter().next().unwrap();
    assert!(
        id.ends_with("DefaultIdConfig"),
        "default diff_id should be the type name, got: {id}"
    );
}
