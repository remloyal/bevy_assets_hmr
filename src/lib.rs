//! Minimal config-file hot-reload for Bevy 0.20.
//!
//! Point at a config file once. Its parsed contents live in the world as a
//! [`Resource`] of **your own type**, and are replaced in place whenever the
//! file changes on disk. There is no wrapper type, no event, and no hook.
//!
//! Supported out of the box: `.ron` and `.json` (chosen by file extension).
//! Any other format works through a custom parser.
//!
//! # Quick start
//!
//! ```no_run
//! use bevy::prelude::*;
//! use bevy_assets_hmr::ConfigWatchAppExt;
//! use serde::Deserialize;
//!
//! #[derive(Resource, Deserialize, PartialEq, Default)]
//! struct Settings {
//!     title: String,
//! }
//!
//! fn main() {
//!     App::new()
//!         .add_plugins(MinimalPlugins)
//!         // format is picked from the extension; resolved against `assets/`
//!         .watch_config::<Settings>("data/settings.ron")
//!         .add_systems(Update, use_settings)
//!         .run();
//! }
//!
//! // Read it like any other resource.
//! fn use_settings(settings: Res<Settings>) {
//!     if !settings.is_changed() {
//!         return;
//!     }
//!     println!("title = {}", settings.title);
//! }
//! ```
//!
//! # Formats
//!
//! - `.ron` → `ron::de::from_bytes`
//! - `.json` → `serde_json::from_slice`
//! - anything else → register with [`watch_config_with`] and supply a parser
//!   over raw bytes (text parsers decode with `std::str::from_utf8`):
//!
//! ```no_run
//! # use bevy::prelude::*;
//! # use bevy_assets_hmr::ConfigWatchAppExt;
//! #[derive(Resource, PartialEq, Default)]
//! struct Volume {
//!     level: u32,
//! }
//!
//! App::new()
//!     .add_plugins(MinimalPlugins)
//!     // parsers receive raw bytes, so binary formats work too;
//!     // text formats decode with `std::str::from_utf8`
//!     .watch_config_with::<Volume, _>("data/volume.txt", |bytes| {
//!         let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
//!         text.trim()
//!             .parse::<u32>()
//!             .map(|level| Volume { level })
//!             .map_err(|error| error.to_string())
//!     });
//! ```
//!
//! # Behavior
//!
//! - **Setup**: one line per type. The file is loaded on the first frame the
//!   app updates; no separate startup step is required.
//! - **Consumption**: a plain `Res<T>` of your own type. Bevy's
//!   [`Res::is_changed`] tells you when the value was reloaded.
//! - **Debounce**: files are polled on an interval (default 150ms), which
//!   coalesces editor atomic saves.
//! - **Errors do not lose data**: a parse error or a temporarily missing file
//!   keeps the last valid value and logs a warning/error. Fixing the file
//!   recovers automatically.
//! - **Value-precise change detection**: the file is re-read each poll and
//!   compared with the previous bytes (so edits are seen regardless of
//!   filesystem mtime granularity), and the parsed value is then compared with
//!   the one in the world. `T` is re-inserted only when it actually differs —
//!   editing whitespace, comments, or fields `T` ignores does not fire
//!   `Res<T>::is_changed`. This is whole-value precision, not per-field:
//!   changing any field of `T` still fires it.
//! - **Logging**: the first load and each successful hot reload are logged at
//!   `info` (through `bevy::log`), the reload line including the changed field
//!   paths for built-in formats; a content change that parses to an equal
//!   value is logged at `debug`; parse/read failures are logged at
//!   `error`/`warn` and keep the last value.
//!
//! # Changed fields
//!
//! For the built-in `.ron` / `.json` formats, each reload also reports *which*
//! fields changed, both in the log line and through the [`ConfigChanges<T>`]
//! resource:
//!
//! ```no_run
//! # use bevy::prelude::*;
//! # use bevy_assets_hmr::{ConfigChanges, ConfigWatchAppExt};
//! # use serde::Deserialize;
//! # #[derive(Resource, Deserialize, PartialEq, Default)]
//! # struct Settings { title: String }
//! fn on_settings(settings: Res<Settings>, changes: Res<ConfigChanges<Settings>>) {
//!     if !settings.is_changed() {
//!         return;
//!     }
//!     if changes.contains("title") {
//!         println!("title 变了: {}", settings.title);
//!     }
//! }
//! ```
//!
//! Paths are dotted for nesting (`graphics.resolution`) and use the field's own
//! name for an array (`npcs`). Custom parsers
//! ([`watch_config_with`](ConfigWatchAppExt::watch_config_with)) have no generic
//! structural diff, so [`ConfigChanges`] stays empty there.
//!
//! # Requirements on `T`
//!
//! For [`watch_config`](ConfigWatchAppExt::watch_config), `T` must be
//! `Resource + Deserialize + PartialEq + Default`:
//! - `Deserialize` to parse the file.
//! - `Resource` so the value can live in the world.
//! - `PartialEq` so a reload can tell whether the parsed value actually
//!   changed.
//! - `Default` so that `Res<T>` resolves even before the first successful
//!   load (you then read `T::default()`; the watcher logs a warning when the
//!   file is missing). This avoids `Option<Res<T>>` in consumers and any
//!   runtime panic from a missing resource.
//!
//! [`watch_config_with`](ConfigWatchAppExt::watch_config_with) drops only the
//! `Deserialize` requirement — a custom parser still needs
//! `Resource + PartialEq + Default`.
//!
//! # One file per type
//!
//! A `T` is loaded from a single file. Registering the same `T` twice logs a
//! warning and ignores the second path.

#![forbid(unsafe_code)]

mod watch;

pub use watch::{ConfigChanges, ConfigWatchPlugin, DEFAULT_POLL_INTERVAL, DEFAULT_ROOT};

use bevy::prelude::*;
use serde::de::DeserializeOwned;

/// App extension trait enabling config-file watching.
///
/// Import it as `use bevy_assets_hmr::ConfigWatchAppExt;`.
pub trait ConfigWatchAppExt {
    /// Watch a config file, choosing the parser from the file extension
    /// (`.ron` or `.json`), and keep its parsed value up to date in the world
    /// as a resource of type `T`.
    ///
    /// `T` must be `Resource + Deserialize + Default` (see the crate docs).
    /// `path` is resolved against [`ConfigWatchPlugin::root`] (default
    /// `assets/`); an absolute path is used as-is.
    ///
    /// # Panics
    ///
    /// Panics if the extension is neither `.ron` nor `.json`. Use
    /// [`watch_config_with`](Self::watch_config_with) for other formats.
    fn watch_config<T: Resource + DeserializeOwned + PartialEq + Default + Send + Sync + 'static>(
        &mut self,
        path: &str,
    ) -> &mut Self;

    /// Watch a config file using a caller-supplied parser.
    ///
    /// The parser receives the file's raw bytes and returns either the parsed
    /// value or an error message (logged, with the last good value kept).
    /// Receiving bytes (not `&str`) means binary formats work too; text
    /// parsers decode with `std::str::from_utf8`.
    ///
    /// Unlike [`watch_config`](Self::watch_config), `T` does **not** need to
    /// implement `Deserialize` — the parser is entirely yours.
    ///
    /// ```no_run
    /// # use bevy::prelude::*;
    /// # use bevy_assets_hmr::ConfigWatchAppExt;
    /// #[derive(Resource, PartialEq, Default)]
    /// struct Volume {
    ///     level: u32,
    /// }
    ///
    /// # let mut app = App::new();
    /// app.watch_config_with::<Volume, _>("data/volume.txt", |bytes| {
    ///     let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    ///     text.trim()
    ///         .parse::<u32>()
    ///         .map(|level| Volume { level })
    ///         .map_err(|error| error.to_string())
    /// });
    /// ```
    fn watch_config_with<T, F>(&mut self, path: &str, parser: F) -> &mut Self
    where
        T: Resource + PartialEq + Default + Send + Sync + 'static,
        F: Fn(&[u8]) -> Result<T, String> + Send + Sync + 'static;
}

impl ConfigWatchAppExt for App {
    fn watch_config<T: Resource + DeserializeOwned + PartialEq + Default + Send + Sync + 'static>(
        &mut self,
        path: &str,
    ) -> &mut Self {
        let (parse, format) = watch::builtin_parser::<T>(path);
        watch::install(
            self,
            watch::WatchRegistration {
                path: path.to_string(),
                parse,
                format: Some(format),
            },
        );
        self
    }

    fn watch_config_with<T, F>(&mut self, path: &str, parser: F) -> &mut Self
    where
        T: Resource + PartialEq + Default + Send + Sync + 'static,
        F: Fn(&[u8]) -> Result<T, String> + Send + Sync + 'static,
    {
        watch::install(
            self,
            watch::WatchRegistration {
                path: path.to_string(),
                parse: Box::new(parser),
                format: None,
            },
        );
        self
    }
}
