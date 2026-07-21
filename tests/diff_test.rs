//! Tests for the user-facing `ConfigDiff` trait.
//!
//! Validates that a consuming crate can implement `ConfigDiff` for a
//! `Vec<Entry>`-style database Asset, and that the diff produces correct
//! (added, removed, modified) id sets.

use bevy::asset::Asset;
use bevy::reflect::TypePath;
use bevy_assets_hmr::ConfigDiff;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

/// Simplified test database that mimics zz_plot's `Vec<Entry>` pattern.
#[derive(
    Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default,
)]
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
    fn diff(
        old: &Self,
        new: &Self,
    ) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
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
