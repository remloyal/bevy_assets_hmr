//! File polling: watch a registered config file, reload it when it changes,
//! and write the parsed value straight into the world as a resource of the
//! user's own type `T`. Built-in `.ron` / `.json` files additionally report
//! which fields changed.

use bevy::app::App;
use bevy::prelude::{Commands, First, Local, Plugin, Res, Resource};
use serde::de::DeserializeOwned;
use std::fs;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

/// Default poll interval between filesystem checks.
pub const DEFAULT_POLL_INTERVAL: Duration = Duration::from_millis(150);

/// Default directory that registered paths are resolved against.
pub const DEFAULT_ROOT: &str = "assets";

/// A format the crate can both parse and structurally diff.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum BuiltinFormat {
    Ron,
    Json,
}

/// Changed field paths from the most recent reload of a watched type.
///
/// Populated only for the built-in `.ron` / `.json` formats (a custom parser
/// has no generic structural diff, so the list stays empty there). Paths use
/// dots for nesting and the field's own name for an array, e.g. `"title"`,
/// `"graphics.resolution"`, `"npcs"`.
///
/// ```no_run
/// # use bevy::prelude::*;
/// # use bevy_assets_hmr::{ConfigChanges, ConfigWatchAppExt};
/// # use serde::Deserialize;
/// # #[derive(Resource, Deserialize, PartialEq, Default)]
/// # struct Settings { title: String }
/// fn on_settings(settings: Res<Settings>, changes: Res<ConfigChanges<Settings>>) {
///     if !settings.is_changed() {
///         return;
///     }
///     if changes.contains("title") {
///         println!("title 变了 -> {}", settings.title);
///     }
/// }
/// ```
#[derive(Resource)]
pub struct ConfigChanges<T> {
    /// Dotted paths of the fields that changed on the last reload, sorted.
    pub changed: Vec<String>,
    /// Bumped once per reload that actually changed the value.
    pub revision: u64,
    _marker: PhantomData<fn() -> T>,
}

impl<T> Default for ConfigChanges<T> {
    fn default() -> Self {
        Self {
            changed: Vec::new(),
            revision: 0,
            _marker: PhantomData,
        }
    }
}

impl<T> ConfigChanges<T> {
    /// Whether a specific field path changed on the last reload.
    pub fn contains(&self, path: &str) -> bool {
        self.changed.iter().any(|changed| changed == path)
    }

    /// Whether no tracked field path changed (always true for custom parsers).
    pub fn is_empty(&self) -> bool {
        self.changed.is_empty()
    }
}

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

/// Per-type registration: which file `T` is loaded from, how to parse it, and
/// (for built-in formats) whether it can be structurally diffed.
#[derive(Resource)]
pub(crate) struct WatchRegistration<T> {
    pub(crate) path: String,
    pub(crate) parse: Box<dyn Fn(&[u8]) -> Result<T, String> + Send + Sync>,
    pub(crate) format: Option<BuiltinFormat>,
}

/// Poll state kept per registered type (via `Local`).
#[derive(Default)]
pub(crate) struct PollState {
    /// Raw bytes of the last read, used to detect content changes without
    /// relying on filesystem mtime granularity, and as the "old" side of the
    /// changed-field diff.
    pub(crate) last_bytes: Option<Vec<u8>>,
    /// Whether the file existed on the last poll.
    pub(crate) present: bool,
    /// Whether a "file missing" warning has already been logged, so a missing
    /// file does not spam the log every poll.
    pub(crate) missing_warned: bool,
    /// Whether the file has been parsed successfully at least once, so the
    /// first success logs "loaded" and later ones log "reloaded".
    pub(crate) loaded_once: bool,
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
) -> (Box<dyn Fn(&[u8]) -> Result<T, String> + Send + Sync>, BuiltinFormat) {
    match Path::new(path)
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("ron") => (
            Box::new(|bytes| ron::de::from_bytes::<T>(bytes).map_err(|error| error.to_string())),
            BuiltinFormat::Ron,
        ),
        Some("json") => (
            Box::new(|bytes| serde_json::from_slice::<T>(bytes).map_err(|error| error.to_string())),
            BuiltinFormat::Json,
        ),
        other => panic!(
            "[HMR] 不支持的配置扩展名 {other:?}（路径 `{path}`）。\
             内置支持 `.ron` 和 `.json`；其它格式请用 `watch_config_with` 提供解析函数。"
        ),
    }
}

/// Install the watcher resources and system for `T`.
pub(crate) fn install<T: Resource + PartialEq + Default + Send + Sync + 'static>(
    app: &mut App,
    registration: WatchRegistration<T>,
) {
    if app.world().get_resource::<WatchSettings>().is_none() {
        app.insert_resource(WatchSettings::default());
    }

    if app.world().get_resource::<WatchRegistration<T>>().is_none() {
        // Seed a default so `Res<T>` always resolves.
        app.init_resource::<T>();
        app.init_resource::<ConfigChanges<T>>();
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
/// the current frame's value. The parsed value is compared with the one
/// already in the world, and the resource is only re-inserted when it actually
/// differs, so [`Res::is_changed`] fires only on a real change.
pub(crate) fn poll_config<T: Resource + PartialEq + Default + Send + Sync + 'static>(
    settings: Res<WatchSettings>,
    registration: Res<WatchRegistration<T>>,
    current: Res<T>,
    changes: Res<ConfigChanges<T>>,
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
    match fs::read(&full_path) {
        Ok(bytes) => {
            if state.present && state.last_bytes.as_deref() == Some(bytes.as_slice()) {
                return;
            }
            state.missing_warned = false;
            match (registration.parse)(&bytes) {
                Ok(value) => {
                    // Only re-insert when the parsed value actually changed:
                    // whitespace/comment edits, or edits to fields `T` ignores,
                    // parse to an equal value and must not fire `is_changed`.
                    let changed = &*current != &value;
                    let was_loaded = state.loaded_once;
                    // Diff old vs new structure for built-in formats, before
                    // `state.last_bytes` is overwritten. Only real reloads
                    // (not the first load) have a previous version to diff.
                    let changed_paths = if changed && was_loaded {
                        registration
                            .format
                            .zip(state.last_bytes.as_deref())
                            .and_then(|(format, old)| diff_field_paths(format, old, &bytes))
                    } else {
                        None
                    };

                    if !was_loaded {
                        state.loaded_once = true;
                        bevy::log::info!("[HMR] {} 首次加载", full_path.display());
                    } else if changed {
                        match changed_paths.as_deref() {
                            Some(paths) if !paths.is_empty() => bevy::log::info!(
                                "[HMR] {} 已热更新，变更字段: {}",
                                full_path.display(),
                                paths.join(", ")
                            ),
                            _ => bevy::log::info!("[HMR] {} 已热更新", full_path.display()),
                        }
                    } else {
                        bevy::log::debug!(
                            "[HMR] {} 内容变化但解析值未变，跳过",
                            full_path.display()
                        );
                    }

                    if changed {
                        // Report changed fields only for actual reloads; the
                        // first load is not a change and leaves it empty.
                        if was_loaded {
                            commands.insert_resource(ConfigChanges::<T> {
                                changed: changed_paths.unwrap_or_default(),
                                revision: changes.revision.wrapping_add(1),
                                _marker: PhantomData,
                            });
                        }
                        commands.insert_resource(value);
                    }

                    state.last_bytes = Some(bytes);
                    state.present = true;
                }
                Err(error) => {
                    // Keep the previously loaded value; record the raw bytes so
                    // an unchanged broken file is not re-parsed every poll.
                    bevy::log::error!(
                        "[HMR] {} 解析失败（保留上一份值）: {error}",
                        full_path.display()
                    );
                    state.last_bytes = Some(bytes);
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

/// Dotted paths of the fields that differ between two versions of a built-in
/// config file. Returns `None` when the format cannot be structurally parsed
/// (e.g. the old bytes were a broken file).
fn diff_field_paths(format: BuiltinFormat, old: &[u8], new: &[u8]) -> Option<Vec<String>> {
    let old = bytes_to_json(format, old)?;
    let new = bytes_to_json(format, new)?;
    let mut out = Vec::new();
    diff_json("", &old, &new, &mut out);
    out.sort();
    Some(out)
}

/// Parse a built-in config into a generic JSON value (RON goes through
/// `ron::Value`, which serializes to a JSON-compatible map with string keys).
fn bytes_to_json(format: BuiltinFormat, bytes: &[u8]) -> Option<serde_json::Value> {
    match format {
        BuiltinFormat::Json => serde_json::from_slice(bytes).ok(),
        BuiltinFormat::Ron => {
            let text = std::str::from_utf8(bytes).ok()?;
            let value: ron::Value = ron::from_str(text).ok()?;
            serde_json::to_value(value).ok()
        }
    }
}

/// Recursively collect changed field paths. Objects recurse by key; anything
/// else (scalars, arrays) is reported as a single path.
fn diff_json(
    prefix: &str,
    old: &serde_json::Value,
    new: &serde_json::Value,
    out: &mut Vec<String>,
) {
    match (old, new) {
        (serde_json::Value::Object(old_map), serde_json::Value::Object(new_map)) => {
            for (key, old_value) in old_map {
                let path = join_path(prefix, key);
                match new_map.get(key) {
                    Some(new_value) if new_value != old_value => {
                        diff_json(&path, old_value, new_value, out)
                    }
                    None => out.push(path),
                    _ => {}
                }
            }
            for key in new_map.keys() {
                if !old_map.contains_key(key) {
                    out.push(join_path(prefix, key));
                }
            }
        }
        _ => {
            if old != new {
                out.push(if prefix.is_empty() {
                    "<root>".to_string()
                } else {
                    prefix.to_string()
                });
            }
        }
    }
}

fn join_path(prefix: &str, key: &str) -> String {
    if prefix.is_empty() {
        key.to_string()
    } else {
        format!("{prefix}.{key}")
    }
}
