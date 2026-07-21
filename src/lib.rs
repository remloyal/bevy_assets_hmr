//! Generic HMR (Hot Module Replacement) framework for Bevy 0.19.
//!
//! Hot-reload ron/json config files with targeted, diff-based refresh.
//! Designed to be project-agnostic: define your `XxxDatabase` Asset type,
//! implement [`ConfigDiff`], then call [`App::register_config`](crate::ConfigHmrAppExt::register_config)
//! to wire it up. Subscribers consume [`ConfigRefresh<T>`] messages.
//!
//! # Quick start
//! ```no_run
//! use bevy::prelude::*;
//! use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin, ConfigDiff};
//! use std::collections::HashSet;
//! # use bevy::asset::Asset;
//! # use bevy::reflect::TypePath;
//! # use serde::{Deserialize, Serialize};
//! # #[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
//! # struct MyDb;
//! # impl ConfigDiff for MyDb {
//! #     fn diff(_old: &Self, _new: &Self) -> (HashSet<String>, HashSet<String>, HashSet<String>) { Default::default() }
//! # }
//!
//! let mut app = App::new();
//! app.add_plugins(MinimalPlugins);
//! app.add_plugins(bevy::asset::AssetPlugin::default());
//! app.add_plugins(ConfigHmrPlugin::default());
//! app.register_config::<MyDb>("data/my.ron");
//! ```

#![forbid(unsafe_code)]

mod asset;
mod binding;
mod core;
mod debounce;
mod diff;
mod ext;
mod loader;
mod refresh;
mod registry;

pub use asset::{ConfigAsset, ConfigHandle};
pub use binding::{config_binding_registry_system, ConfigBind, HandleEntityCache};
pub use core::{cache_validation_system, hmr_core_system, HmrAsset, HmrSource, LastSnapshot};
pub use debounce::{flush_debounced_refresh, RefreshDebouncer};
pub use diff::ConfigDiff;
pub use ext::ConfigHmrAppExt;
pub use loader::ConfigLoader;
pub use refresh::{ConfigRefresh, DiffKind};
pub use registry::ConfigPathRegistry;

// Re-export derive macro，让用户 `use bevy_assets_hmr::ConfigDiff` 同时
// 获得 trait 和 `#[derive(ConfigDiff)]` 能力（Rust 允许 derive macro 和
// trait 同名，类似 bevy 的 `Component`）。
pub use bevy_assets_hmr_derive::ConfigDiff;

use bevy::prelude::*;
use std::time::Duration;

/// HMR plugin. Add to your `App` once, then call
/// [`App::register_config`](ConfigHmrAppExt::register_config) for each
/// config type you want to hot-reload.
///
/// # Example
/// ```no_run
/// use bevy::prelude::*;
/// use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin};
///
/// let mut app = App::new();
/// app.add_plugins(MinimalPlugins);
/// app.add_plugins(bevy::asset::AssetPlugin::default());
/// app.add_plugins(ConfigHmrPlugin::default());
/// // app.register_config::<MyDb>("data/my.ron");
/// ```
pub struct ConfigHmrPlugin {
    /// Debounce window for batching rapid file changes. Default 150ms.
    pub debounce_window: Duration,
}

impl Default for ConfigHmrPlugin {
    fn default() -> Self {
        Self {
            debounce_window: Duration::from_millis(150),
        }
    }
}

impl Plugin for ConfigHmrPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ConfigPathRegistry>();
        // Store the global debounce window so `App::register_config` can
        // apply it to each per-type `RefreshDebouncer<T>`. This avoids the
        // old "must use the same plugin instance" footgun.
        app.insert_resource(HmrSettings {
            debounce_window: self.debounce_window,
        });
    }
}

/// Internal global settings, set by [`ConfigHmrPlugin::build`], read by
/// [`App::register_config`](ConfigHmrAppExt::register_config).
#[derive(Resource)]
pub(crate) struct HmrSettings {
    pub debounce_window: Duration,
}
