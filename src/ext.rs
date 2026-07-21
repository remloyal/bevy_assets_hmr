//! Ergonomic `App` extension methods for HMR setup.
//!
//! These one-liners replace the old `add_plugins + register_hmr_type`
//! two-step dance and the manual `LastSnapshot` population. The user
//! flow becomes:
//!
//! 1. `app.add_plugins(ConfigHmrPlugin::default())`
//! 2. `app.register_config::<MyDb>("data/my.ron")` (per type，包装模式)
//! 3. (optional, headless only) `app.setup_hmr_headless()`
//! 4. (optional, tests only) `app.insert_config::<MyDb>(id, raw, path)`
//!    to seed initial data without a real file on disk.
//!
//! # 直接模式
//!
//! 直接模式下用户自己 impl `HmrSource`、注册自己的 loader、自己初始化
//! `HandleEntityCache<A>` / `RefreshDebouncer<A>` / `LastSnapshot<A>` /
//! `ConfigRefresh<A::Config>` 消息，并用 `load_config_at_startup::<A>`
//! Startup 系统加载文件。

use crate::HmrSettings;
use crate::asset::{ConfigAsset, ConfigHandle};
use crate::binding::HandleEntityCache;
use crate::core::{HmrAsset, HmrSource, LastSnapshot};
use crate::debounce::RefreshDebouncer;
use crate::loader::ConfigLoader;
use crate::refresh::{ConfigRefresh, ConfigRemoved};
use crate::registry::ConfigPathRegistry;
use bevy::app::App;
use bevy::asset::{AssetId, Assets};
use bevy::ecs::message::{MessageRegistry, ShouldUpdateMessages};
use bevy::prelude::*;
use std::time::Duration;

/// App extension trait for ergonomic HMR setup.
///
/// Imported as `use bevy_assets_hmr::ConfigHmrAppExt` to enable
/// [`App::register_config`], [`App::insert_config`], and
/// [`App::setup_hmr_headless`].
pub trait ConfigHmrAppExt {
    /// Register a config type `T` for HMR (包装模式). One-liner that replaces
    /// the old `add_plugins(ConfigHmrPlugin) + ConfigHmrPlugin::register_hmr_type`
    /// two-step dance, **and** the manual `asset_server.load` + handle
    /// retention boilerplate.
    ///
    /// - Registers `ConfigLoader<T>` (ron + json).
    /// - Initializes `ConfigAsset<T>` and `T` asset stores.
    /// - Initializes per-type resources: `HandleEntityCache<ConfigAsset<T>>`,
    ///   `RefreshDebouncer<ConfigAsset<T>>`, `LastSnapshot<ConfigAsset<T>>`.
    /// - Registers `ConfigRefresh<T>` message（`ConfigAsset<T>::Config = T`）。
    /// - Adds HMR core systems to `Update`，系统泛型用 `ConfigAsset<T>`。
    /// - Records the path in [`ConfigPathRegistry`] keyed by
    ///   `ConfigAsset::<T>::type_path()`。
    /// - **Registers a Startup system that loads `path` via `AssetServer` and
    ///   stores the strong handle in `ConfigHandle<ConfigAsset<T>>`** (prevents
    ///   the asset from being unloaded; `bevy/file_watcher` reloads update
    ///   this handle in place).
    ///
    /// The debounce window is taken from [`crate::ConfigHmrPlugin`]'s
    /// `debounce_window` field, so there's no need to pass it again here.
    ///
    /// # Example
    /// ```no_run
    /// # use bevy::prelude::*;
    /// # use bevy::asset::Asset;
    /// # use bevy::reflect::TypePath;
    /// # use serde::{Deserialize, Serialize};
    /// # use std::collections::HashSet;
    /// # use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin, SimpleConfigDiff};
    /// # #[derive(bevy::asset::Asset, bevy::reflect::TypePath, Clone, PartialEq, Default)]
    /// # #[derive(serde::Deserialize, serde::Serialize)]
    /// # struct MyDb;
    /// # impl SimpleConfigDiff for MyDb {}
    /// # let mut app = App::new();
    /// app.add_plugins(ConfigHmrPlugin::default());
    /// app.register_config::<MyDb>("data/my.ron");
    /// // ↑ 这一行就完成了：注册 loader、系统、自动加载文件、持有 handle
    /// ```
    fn register_config<T: HmrAsset>(&mut self, path: &str) -> &mut Self;

    /// Register a raw asset type `A` for HMR (直接模式). 适用于用户已用
    /// 自定义 `AssetLoader` 加载的 Asset 类型，不经过 `ConfigAsset<T>` 包装。
    ///
    /// 与 `register_config` 的区别：
    /// - **不注册 ConfigLoader**：用户用自己的 loader（如 bevy 内置的
    ///   `ImageLoader`、`AudioLoader`，或自定义二进制 loader）
    /// - **不调用 init_asset**：用户应自行 `app.init_asset::<A>()` 和
    ///   `app.init_asset_loader::<MyLoader>()`
    /// - **Asset 本身就是 Config**：`A: HmrSource<Config = A>`，用户需
    ///   impl `HmrSource`（通常 `fn config(&self) -> &Self { self }`）
    ///   并 impl `ConfigDiff`
    /// - `ConfigRefresh<A>` 的泛型是 `A` 本身（因为 `A::Config = A`）
    ///
    /// 自动完成：
    /// - 初始化 per-type resources: `HandleEntityCache<A>`,
    ///   `RefreshDebouncer<A>`, `LastSnapshot<A>`
    /// - 注册 `ConfigRefresh<A>` message
    /// - 添加 HMR 核心系统（泛型用 `A`）
    /// - 记录 path 到 `ConfigPathRegistry`（key 为 `A::type_path()`）
    /// - 注册 Startup 系统加载文件 + 持有 handle
    ///
    /// # Example
    /// ```no_run
    /// # use bevy::prelude::*;
    /// # use bevy::asset::{Asset, AssetLoader, io::Reader, LoadContext};
    /// # use bevy::reflect::TypePath;
    /// # use serde::{Deserialize, Serialize};
    /// # use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin, HmrSource, SimpleConfigDiff};
    /// # #[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
    /// # struct MyAsset { value: i32 }
    /// # impl SimpleConfigDiff for MyAsset {}
    /// # impl HmrSource for MyAsset {
    /// #     type Config = MyAsset;
    /// #     fn config(&self) -> &Self::Config { self }
    /// # }
    /// # let mut app = App::new();
    /// # app.add_plugins(bevy::asset::AssetPlugin::default());
    /// app.add_plugins(ConfigHmrPlugin::default());
    /// app.init_asset::<MyAsset>();
    /// // app.init_asset_loader::<MyAssetLoader>();  // 用户自己的 loader
    /// app.register_asset::<MyAsset>("data/my.custom");
    /// ```
    fn register_asset<A: HmrSource>(&mut self, path: &str) -> &mut Self;

    /// Insert a config asset directly (bypasses `AssetServer`).
    ///
    /// Used by tests/examples that want to seed initial data without a real
    /// file on disk. The snapshot is **not** pre-populated here - the HMR
    /// core auto-initializes it on the first `AssetEvent::Added` (which
    /// `Assets::insert` triggers), so subscribers won't receive a spurious
    /// "first load" refresh.
    ///
    /// In production, prefer `register_config` (which auto-loads the file).
    fn insert_config<T: HmrAsset>(
        &mut self,
        id: AssetId<ConfigAsset<T>>,
        raw: T,
        source_path: &str,
    ) -> &mut Self;

    /// Configure the app for headless HMR (no render world).
    ///
    /// Sets `MessageRegistry::should_update = Always` so `Messages` buffers
    /// swap every frame. Required for tests/examples that don't run the
    /// full `DefaultPlugins` (whose render world would otherwise signal the
    /// update).
    ///
    /// In a real app with `DefaultPlugins`, this is a no-op equivalent.
    fn setup_hmr_headless(&mut self) -> &mut Self;

    /// Watch an **arbitrary** `Asset` type for file-level hot-reload — no
    /// `ConfigDiff` / `HmrSource` required.
    ///
    /// This is the "lighter" alternative to [`register_asset`](Self::register_asset)
    /// for asset types where you only need change-notification + entity
    /// binding tracking, not id-level diffing. Typical use cases: `Image`,
    /// `Scene` (gltf), `AudioSource`, `Mesh`, fonts.
    ///
    /// Bevy's `AssetServer` (with `bevy/file_watcher`) handles the actual
    /// file reloading and GPU upload for built-in types. This method adds:
    ///
    /// 1. **Entity binding** via `AssetBind<A>` — attach to entities so the
    ///    framework knows which entities depend on a given handle.
    /// 2. **Change notification** via `AssetChanged<A>` — dispatched on
    ///    `AssetEvent::Added` / `Modified`.
    ///
    /// Unlike `register_asset`, this does **not**:
    /// - require `ConfigDiff` or `HmrSource`
    /// - register a `ConfigLoader` (use your own loader / Bevy's built-ins)
    /// - create a `LastSnapshot` or `RefreshDebouncer` (no diff/debounce)
    /// - dispatch `ConfigRefresh` / `ConfigRemoved`
    ///
    /// You **must** call `app.init_asset::<A>()` yourself (or use
    /// `DefaultPlugins` which handles it for built-in types).
    ///
    /// # Example
    /// ```ignore
    /// // Requires `bevy/file_watcher` feature + a real asset type like Image.
    /// use bevy::prelude::*;
    /// use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin};
    /// let mut app = App::new();
    /// app.add_plugins(DefaultPlugins); // enables bevy/file_watcher
    /// app.add_plugins(ConfigHmrPlugin::default());
    /// app.watch_asset::<Image>("textures/player.png");
    /// // Subscribe via MessageReader<AssetChanged<Image>> in your system.
    /// ```
    fn watch_asset<A: Asset + Clone + Send + Sync + 'static>(&mut self, path: &str) -> &mut Self;
}

impl ConfigHmrAppExt for App {
    fn register_config<T: HmrAsset>(&mut self, path: &str) -> &mut Self {
        // Pull the global debounce window from HmrSettings (set by
        // ConfigHmrPlugin::build). Fall back to 150ms if the plugin wasn't
        // added - this shouldn't happen in normal usage but keeps the
        // method robust.
        let debounce_window = self
            .world()
            .get_resource::<HmrSettings>()
            .map(|s| s.debounce_window)
            .unwrap_or_else(|| Duration::from_millis(150));

        // 包装模式：系统泛型用 `ConfigAsset<T>`（它自动 impl `HmrSource`），
        // `ConfigAsset<T>::Config = T`，所以 `ConfigRefresh<T>` 的泛型仍是 `T`。
        self.register_asset_loader(ConfigLoader::<T>::default())
            .init_asset::<ConfigAsset<T>>()
            .init_asset::<T>()
            .init_resource::<HandleEntityCache<ConfigAsset<T>>>()
            .init_resource::<RefreshDebouncer<ConfigAsset<T>>>()
            .init_resource::<LastSnapshot<ConfigAsset<T>>>()
            .add_message::<ConfigRefresh<T>>()
            .add_message::<ConfigRemoved<T>>();

        #[cfg(feature = "dev")]
        {
            self.add_systems(
                Update,
                (
                    crate::binding::config_binding_registry_system::<ConfigAsset<T>>,
                    crate::binding::config_binding_cleanup_system::<ConfigAsset<T>>,
                    crate::core::hmr_core_system::<ConfigAsset<T>>,
                    crate::debounce::flush_debounced_refresh::<ConfigAsset<T>>,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                crate::core::cache_validation_system::<ConfigAsset<T>>.run_if(
                    bevy::time::common_conditions::on_timer(Duration::from_secs(30)),
                ),
            );
        }

        // Apply the global debounce window to this type's debouncer.
        self.world_mut()
            .resource_mut::<RefreshDebouncer<ConfigAsset<T>>>()
            .window = debounce_window;

        // Record path in the registry, keyed by `ConfigAsset<T>::type_path()`
        // so `load_config_at_startup::<ConfigAsset<T>>` can look it up.
        self.world_mut()
            .resource_mut::<ConfigPathRegistry>()
            .paths
            .insert(ConfigAsset::<T>::type_path().to_string(), path.to_string());

        // 记录 path 在 ConfigPathRegistry 里，Startup 系统会读取它来 load。
        // 用一个泛型 Startup 系统避免闭包 + 泛型 + move 的类型推断问题。
        // 包装模式下 `A = ConfigAsset<T>`。
        self.add_systems(
            Startup,
            crate::ext::load_config_at_startup::<ConfigAsset<T>>,
        );

        self
    }

    fn insert_config<T: HmrAsset>(
        &mut self,
        id: AssetId<ConfigAsset<T>>,
        raw: T,
        source_path: &str,
    ) -> &mut Self {
        {
            let mut assets = self.world_mut().resource_mut::<Assets<ConfigAsset<T>>>();
            let _ = assets.insert(
                id,
                ConfigAsset {
                    raw: raw.clone(),
                    source_path: source_path.to_string(),
                },
            );
        }
        // Eagerly initialize the snapshot so subscribers don't need to wait
        // for the `AssetEvent::Added` to flush through the event pipeline
        // (which only happens in `PostUpdate`). The `Added` event will still
        // fire and eventually reach `flush_debounced_refresh`, but since the
        // snapshot already exists and matches the new asset, the diff will
        // be empty and no refresh will be dispatched.
        self.world_mut()
            .resource_mut::<LastSnapshot<ConfigAsset<T>>>()
            .map
            .insert(id, raw);
        self
    }

    fn register_asset<A: HmrSource>(&mut self, path: &str) -> &mut Self {
        let debounce_window = self
            .world()
            .get_resource::<HmrSettings>()
            .map(|s| s.debounce_window)
            .unwrap_or_else(|| Duration::from_millis(150));

        // 直接模式：不注册 ConfigLoader，不 init_asset（用户自己做）。
        // 系统泛型直接用 A（用户 Asset 本身，A::Config = A）。
        self.init_resource::<HandleEntityCache<A>>()
            .init_resource::<RefreshDebouncer<A>>()
            .init_resource::<LastSnapshot<A>>()
            .add_message::<ConfigRefresh<A::Config>>()
            .add_message::<ConfigRemoved<A::Config>>();

        #[cfg(feature = "dev")]
        {
            self.add_systems(
                Update,
                (
                    crate::binding::config_binding_registry_system::<A>,
                    crate::binding::config_binding_cleanup_system::<A>,
                    crate::core::hmr_core_system::<A>,
                    crate::debounce::flush_debounced_refresh::<A>,
                )
                    .chain(),
            )
            .add_systems(
                Update,
                crate::core::cache_validation_system::<A>.run_if(
                    bevy::time::common_conditions::on_timer(Duration::from_secs(30)),
                ),
            );
        }

        self.world_mut()
            .resource_mut::<RefreshDebouncer<A>>()
            .window = debounce_window;

        self.world_mut()
            .resource_mut::<ConfigPathRegistry>()
            .paths
            .insert(A::type_path().to_string(), path.to_string());

        self.add_systems(Startup, crate::ext::load_config_at_startup::<A>);

        self
    }

    fn setup_hmr_headless(&mut self) -> &mut Self {
        self.world_mut()
            .resource_mut::<MessageRegistry>()
            .should_update = ShouldUpdateMessages::Always;
        self
    }

    fn watch_asset<A: Asset + Clone + Send + Sync + 'static>(&mut self, path: &str) -> &mut Self {
        use crate::watcher::{
            AssetBindCache, asset_bind_cleanup_system, asset_bind_registry_system,
            asset_watcher_system,
        };

        // Per-type resources for entity binding tracking.
        self.init_resource::<AssetBindCache<A>>();

        // Register the AssetChanged<A> message.
        self.add_message::<crate::watcher::AssetChanged<A>>();

        // Systems: registry -> cleanup -> watcher, chained every frame.
        #[cfg(feature = "dev")]
        {
            self.add_systems(
                Update,
                (
                    asset_bind_registry_system::<A>,
                    asset_bind_cleanup_system::<A>,
                    asset_watcher_system::<A>,
                )
                    .chain(),
            );
        }

        // Startup: load the asset and hold a strong handle so it isn't
        // unloaded. We reuse ConfigHandle<A> (a generic handle-holder) for
        // convenience — it doesn't require HmrSource, only Asset.
        let path_owned = path.to_string();
        self.add_systems(
            Startup,
            move |asset_server: Res<bevy::asset::AssetServer>, mut commands: Commands| {
                let handle = asset_server.load::<A>(&path_owned);
                commands.insert_resource(ConfigHandle::<A> { _handle: handle });
                bevy::log::info!("[HMR] watching asset: {} -> {}", A::type_path(), path_owned);
            },
        );

        self
    }
}

/// Startup 系统：从 `ConfigPathRegistry` 读取 `A` 对应的 path，
/// 通过 `AssetServer` 加载文件，并用 `ConfigHandle<A>` Resource 持有
/// 强引用 handle 防止资产被回收。
///
/// 由 [`App::register_config`](ConfigHmrAppExt::register_config) 自动注册
/// （包装模式下 `A = ConfigAsset<T>`），用户无需手动调用。直接模式下
/// 用户可手动注册 `load_config_at_startup::<MyAsset>` 并确保
/// `ConfigPathRegistry` 中以 `MyAsset::type_path()` 为 key 存入 path。
pub fn load_config_at_startup<A: HmrSource>(
    asset_server: Res<bevy::asset::AssetServer>,
    registry: Res<ConfigPathRegistry>,
    mut commands: Commands,
) {
    let path = registry.paths.get(A::type_path()).cloned();
    if let Some(path) = path {
        let handle = asset_server.load::<A>(&path);
        commands.insert_resource(ConfigHandle::<A> { _handle: handle });
        // tracing 宏在无 subscriber 时为空操作，不会 panic；无 LogPlugin
        // 的测试环境也不会触发 IoTaskPool 依赖（bevy 0.19 已解耦）。
        bevy::log::info!("[HMR] registered config: {} -> {}", A::type_path(), path);
    } else {
        bevy::log::warn!(
            "[HMR] no path registered for {} (register_config not called?)",
            A::type_path()
        );
    }
}
