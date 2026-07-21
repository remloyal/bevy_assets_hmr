//! Path registry mapping config type names to their asset paths.
//!
//! Populated by [`App::register_config`](crate::ConfigHmrAppExt::register_config),
//! keyed by `T::type_path()`. Useful for diagnostics or for systems that
//! need to look up which file backs a given config type.

use bevy::prelude::Resource;
use std::collections::HashMap;

/// Registry mapping config type name (`T::type_path()`) to the asset path
/// (relative to `assets/`, e.g. `"data/npc.ron"`).
///
/// Populated by [`App::register_config`](crate::ConfigHmrAppExt::register_config).
#[derive(Resource, Default)]
pub struct ConfigPathRegistry {
    /// `T::type_path()` -> asset path.
    pub paths: HashMap<String, String>,
}
