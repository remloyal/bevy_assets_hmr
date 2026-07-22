//! Path registry mapping config type names to their asset paths.
//!
//! Populated by [`App::register_config`](crate::ConfigHmrAppExt::register_config),
//! keyed by `T::type_path()`. A type may have multiple registered files.

use bevy::prelude::Resource;
use std::collections::HashMap;

/// Registry mapping config type name (`T::type_path()`) to asset paths
/// (relative to `assets/`, e.g. `"data/npc.ron"`).
///
/// Populated by [`App::register_config`](crate::ConfigHmrAppExt::register_config).
#[derive(Resource, Default)]
pub struct ConfigPathRegistry {
    /// `T::type_path()` -> deduplicated asset paths.
    pub paths: HashMap<String, Vec<String>>,
}

impl ConfigPathRegistry {
    /// Register a path once for an asset type.
    pub fn register(&mut self, type_path: impl Into<String>, path: impl Into<String>) {
        let paths = self.paths.entry(type_path.into()).or_default();
        let path = path.into();
        if !paths.iter().any(|existing| existing == &path) {
            paths.push(path);
        }
    }
}
