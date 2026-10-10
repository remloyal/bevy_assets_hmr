//! File polling: watch a registered config file, reload it when it changes,
//! and write the parsed value straight into the world as a resource of the
//! user's own type `T`.
//!
//! Parsing is captured at registration as a boxed closure: `.ron` / `.json`
//! get a built-in parser chosen from the extension, and
//! [`watch_config_with`](crate::ConfigWatchAppExt::watch_config_with) supplies
//! any custom one. The polling machinery is therefore format-agnostic and does
//! not require `T: Deserialize`.

use bevy::app::App;
use bevy::prelude::{Commands, First, Local, Plugin, Res, Resource};
use serde::de::DeserializeOwned;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Default poll interval between filesystem checks.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Default directory that registered paths are resolved against.
pub const DEFAULT_ROOT: &str = "assets";

/// Runtime settings for the watcher.
#[derive(Resource, Clone, Debug)]
pub(crate) struct WatchSettings {
    pub(crate) root: PathBuf,
    pub(crate) poll_interval: Duration,
}

impl Default for WatchSettings {
    fn default() -> Self {
        Self {
            root: PathBuf::from(DEFAULT_ROOT),
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

/// Per-type registration: which file `T` is loaded from, and how to parse it.
#[derive(Resource)]
pub(crate) struct WatchRegistration<T> {
    pub(crate) path: String,
    pub(crate) parse: Box<dyn Fn(&str) -> Result<T, String> + Send + Sync>,
}

/// Poll state kept per registered type (via `Local`).
#[derive(Default)]
pub(crate) struct PollState {
    /// Raw text of the last read, used to detect content changes without
    /// relying on filesystem mtime granularity.
    pub(crate) last_raw: Option<String>,
    /// Whether the file existed on the last poll.
    pub(crate) present: bool,
    /// Whether a "file missing" warning has already been logged, so a missing
    /// file does not spam the log every poll.
    pub(crate) missing_warned: bool,
}

/// Optional plugin form of the watcher, for custom `root` / `poll_interval`.
///
/// [`watch_config`](crate::ConfigWatchAppExt::watch_config) installs defaults
/// on its own, so this plugin is only needed when you want to configure the
/// watcher. Add it **before** watching.
///
/// ```no_run
/// # use bevy::prelude::*;
/// # use bevy_assets_hmr::ConfigWatchPlugin;
/// # use std::time::Duration;
/// App::new()
///     .add_plugins(MinimalPlugins)
///     .add_plugins(ConfigWatchPlugin {
///         root: "config".into(),
///         poll_interval: Duration::from_millis(200),
///     });
/// ```
#[derive(Clone, Debug)]
pub struct ConfigWatchPlugin {
    /// Directory that registered paths are resolved against.
    pub root: PathBuf,
    /// Delay between filesystem checks.
    pub poll_interval: Duration,
}

impl Default for ConfigWatchPlugin {
    fn default() -> Self {
        Self {
            root: PathBuf::from(DEFAULT_ROOT),
            poll_interval: DEFAULT_POLL_INTERVAL,
        }
    }
}

impl Plugin for ConfigWatchPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(WatchSettings {
            root: self.root.clone(),
            poll_interval: self.poll_interval,
        });
    }
}

/// Built-in parser for `T`, chosen from the file extension.
///
/// Panics for extensions other than `.ron` / `.json`, pointing at
/// `watch_config_with`.
pub(crate) fn builtin_parser<T: DeserializeOwned + 'static>(
    path: &str,
) -> Box<dyn Fn(&str) -> Result<T, String> + Send + Sync> {
    match Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("ron") => Box::new(|text| ron::from_str::<T>(text).map_err(|error| error.to_string())),
        Some("json") => {
            Box::new(|text| serde_json::from_str::<T>(text).map_err(|error| error.to_string()))
        }
        other => panic!(
            "[HMR] 不支持的配置扩展名 {other:?}（路径 `{path}`）。\
             内置支持 `.ron` 和 `.json`；其它格式请用 `watch_config_with` 提供解析函数。"
        ),
    }
}

/// Install the watcher resources and system for `T`.
pub(crate) fn install<T: Resource + Default + Send + Sync + 'static>(
    app: &mut App,
    registration: WatchRegistration<T>,
) {
    if app.world().get_resource::<WatchSettings>().is_none() {
        app.insert_resource(WatchSettings::default());
    }

    if app.world().get_resource::<WatchRegistration<T>>().is_none() {
        // Seed a default so `Res<T>` always resolves.
        app.init_resource::<T>();
        app.insert_resource(registration);
        // Runs before `Update` so consumers see the current frame's load.
        app.add_systems(First, poll_config::<T>);
    } else {
        bevy::log::warn!(
            "[HMR] {} 已注册，忽略额外路径 `{}`（一个类型只监听一个文件）",
            std::any::type_name::<T>(),
            registration.path,
        );
    }
}

/// System: poll `T`'s file and insert the parsed value as a resource when the
/// file content changes.
///
/// Runs in [`First`](bevy::prelude::First) so systems in later schedules see
/// the current frame's value. The resource is only (re)inserted when a load
/// succeeds, so [`Res::is_changed`] fires exactly on reloads.
pub(crate) fn poll_config<T: Resource + Default + Send + Sync + 'static>(
    settings: Res<WatchSettings>,
    registration: Res<WatchRegistration<T>>,
    mut last_run: Local<Option<Instant>>,
    mut state: Local<PollState>,
    mut commands: Commands,
) {
    let now = Instant::now();
    if let Some(previous) = *last_run
        && now.saturating_duration_since(previous) < settings.poll_interval
    {
        return;
    }
    *last_run = Some(now);

    let full_path = settings.root.join(&registration.path);
    match fs::read_to_string(&full_path) {
        Ok(raw) => {
            if state.present && state.last_raw.as_deref() == Some(raw.as_str()) {
                return;
            }
            state.missing_warned = false;
            match (registration.parse)(&raw) {
                Ok(value) => {
                    commands.insert_resource(value);
                    state.last_raw = Some(raw);
                    state.present = true;
                }
                Err(error) => {
                    // Keep the previously loaded value; record the raw text so
                    // an unchanged broken file is not re-parsed every poll.
                    bevy::log::error!(
                        "[HMR] {} 解析失败（保留上一份值）: {error}",
                        full_path.display()
                    );
                    state.last_raw = Some(raw);
                    state.present = true;
                }
            }
        }
        Err(error) => {
            if !state.missing_warned {
                bevy::log::warn!(
                    "[HMR] {} 读取失败（保留上一份值）: {error}",
                    full_path.display()
                );
                state.missing_warned = true;
            }
            state.present = false;
        }
    }
}
