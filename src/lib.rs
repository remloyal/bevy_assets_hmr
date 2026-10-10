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
//! #[derive(Resource, Deserialize, Default)]
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
//! - `.ron` → `ron::from_str`
//! - `.json` → `serde_json::from_str`
//! - anything else → register with [`watch_config_with`] and supply a parser:
//!
//! ```no_run
//! # use bevy::prelude::*;
//! # use bevy_assets_hmr::ConfigWatchAppExt;
//! #[derive(Resource, Default)]
//! struct Volume {
//!     level: u32,
//! }
//!
//! App::new()
//!     .add_plugins(MinimalPlugins)
//!     // no `toml` dependency here — the parser is entirely yours
//!     .watch_config_with::<Volume, _>("data/volume.txt", |text| {
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
//! - **Change detection is content-based**: the file is re-read each poll and
//!   compared to the previous text, so edits are detected regardless of
//!   filesystem mtime granularity. `T` is only re-inserted when a load
//!   succeeds, so `Res<T>::is_changed` never fires spuriously.
//!
//! # Requirements on `T`
//!
//! For [`watch_config`](ConfigWatchAppExt::watch_config), `T` must be
//! `Resource + Deserialize + Default`:
//! - `Deserialize` to parse the file.
//! - `Resource` so the value can live in the world.
//! - `Default` so that `Res<T>` resolves even before the first successful
//!   load (you then read `T::default()`; the watcher logs a warning when the
//!   file is missing). This avoids `Option<Res<T>>` in consumers and any
//!   runtime panic from a missing resource.
//!
//! [`watch_config_with`](ConfigWatchAppExt::watch_config_with) keeps only the
//! `Resource + Default` requirements — a custom parser needs no `Deserialize`.
//!
//! # One file per type
//!
//! A `T` is loaded from a single file. Registering the same `T` twice logs a
//! warning and ignores the second path.

#![forbid(unsafe_code)]

mod watch;

pub use watch::{ConfigWatchPlugin, DEFAULT_POLL_INTERVAL, DEFAULT_ROOT};

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
    fn watch_config<T: Resource + DeserializeOwned + Default + Send + Sync + 'static>(
        &mut self,
        path: &str,
    ) -> &mut Self;

    /// Watch a config file using a caller-supplied parser.
    ///
    /// The parser receives the file's full text and returns either the parsed
    /// value or an error message (logged, with the last good value kept).
    ///
    /// Unlike [`watch_config`](Self::watch_config), `T` does **not** need to
    /// implement `Deserialize` — the parser is entirely yours.
    ///
    /// ```no_run
    /// # use bevy::prelude::*;
    /// # use bevy_assets_hmr::ConfigWatchAppExt;
    /// #[derive(Resource, Default)]
    /// struct Volume {
    ///     level: u32,
    /// }
    ///
    /// # let mut app = App::new();
    /// app.watch_config_with::<Volume, _>("data/volume.txt", |text| {
    ///     text.trim()
    ///         .parse::<u32>()
    ///         .map(|level| Volume { level })
    ///         .map_err(|error| error.to_string())
    /// });
    /// ```
    fn watch_config_with<T, F>(&mut self, path: &str, parser: F) -> &mut Self
    where
        T: Resource + Default + Send + Sync + 'static,
        F: Fn(&str) -> Result<T, String> + Send + Sync + 'static;
}

impl ConfigWatchAppExt for App {
    fn watch_config<T: Resource + DeserializeOwned + Default + Send + Sync + 'static>(
        &mut self,
        path: &str,
    ) -> &mut Self {
        let parse = watch::builtin_parser::<T>(path);
        watch::install(
            self,
            watch::WatchRegistration {
                path: path.to_string(),
                parse,
            },
        );
        self
    }

    fn watch_config_with<T, F>(&mut self, path: &str, parser: F) -> &mut Self
    where
        T: Resource + Default + Send + Sync + 'static,
        F: Fn(&str) -> Result<T, String> + Send + Sync + 'static,
    {
        watch::install(
            self,
            watch::WatchRegistration {
                path: path.to_string(),
                parse: Box::new(parser),
            },
        );
        self
    }
}
