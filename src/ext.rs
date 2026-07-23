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
use crate::loader::{ConfigLoader, ConfigValidator};
use crate::refresh::{ConfigRefresh, ConfigReloadFailed, ConfigRemoved};
use crate::registry::ConfigPathRegistry;
use bevy::app::App;
use bevy::asset::{AssetId, Assets};
use bevy::ecs::message::{MessageRegistry, ShouldUpdateMessages};
use bevy::prelude::*;
use std::marker::PhantomData;
use std::time::Duration;

#[derive(Resource)]
struct HmrTypeInstalled<A: Asset>(PhantomData<A>);

#[derive(Resource)]
struct HmrStartupRegistered<A: Asset>(PhantomData<A>);

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

    /// Install a semantic validator for a previously registered wrapped
    /// config type. Validation errors are returned through Bevy's loader
    /// failure path, so the previous valid asset remains available.
    fn register_config_validator<T, F>(&mut self, validator: F) -> &mut Self
    where
        T: HmrAsset,
        F: Fn(&T) -> Result<(), String> + Send + Sync + 'static;

    /// Install an owned-size estimator for clone byte metrics.
    fn register_config_size_estimator<T: HmrAsset>(
        &mut self,
        estimator: fn(&T) -> usize,
    ) -> &mut Self;

    /// Register a raw asset type `A` for HMR (直接模式). 适用于用户已用
    /// 自定义 `AssetLoader` 加载的 Asset 类型，不经过 `ConfigAsset<T>` 包装。
    ///
    /// 与 `register_config` 的区别：
    /// - **不注册 ConfigLoader**：用户使用自己的配置或二进制 loader。
    ///   `Image`、`AudioSource` 等普通 Bevy Asset 通常应使用 `watch_asset`
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

    /// Insert an asset directly (bypasses `AssetServer`) - 直接模式.
    ///
    /// Direct-mode counterpart of [`insert_config`](Self::insert_config).
    /// Seeds an `Assets<A>` entry and eagerly initializes the `LastSnapshot<A>`
    /// snapshot, eliminating the ~6 lines of boilerplate otherwise required:
    ///
    /// ```ignore
    /// // Without this helper, direct-mode users must write:
    /// let mut assets = app.world_mut().resource_mut::<Assets<MyAsset>>();
    /// let _ = assets.insert(id, initial.clone());
    /// let mut snapshots = app.world_mut().resource_mut::<LastSnapshot<MyAsset>>();
    /// snapshots.map.insert(id, initial);
    ///
    /// // With this helper:
    /// app.insert_asset(id, initial, "data/my.custom");
    /// ```
    ///
    /// The snapshot is pre-populated so subscribers won't receive a spurious
    /// "first load" `AssetEvent::Added` refresh.
    ///
    /// # Example
    /// ```no_run
    /// # use bevy::asset::{Asset, AssetId};
    /// # use bevy::prelude::*;
    /// # use bevy::reflect::TypePath;
    /// # use serde::{Deserialize, Serialize};
    /// # use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin, HmrSource, SimpleConfigDiff};
    /// # #[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
    /// # struct LevelAsset { id: String, max_turns: u32 }
    /// # impl SimpleConfigDiff for LevelAsset { fn diff_id() -> &'static str { "level" } }
    /// # impl HmrSource for LevelAsset {
    /// #     type Config = LevelAsset;
    /// #     fn config(&self) -> &Self::Config { self }
    /// # }
    /// # let mut app = App::new();
    /// # app.add_plugins(bevy::asset::AssetPlugin::default());
    /// # app.add_plugins(ConfigHmrPlugin::default());
    /// # app.init_asset::<LevelAsset>();
    /// # app.register_asset::<LevelAsset>("levels/level_1.level");
    /// use uuid::Uuid;
    /// let id = AssetId::Uuid { uuid: Uuid::new_v4() };
    /// app.insert_asset(id, LevelAsset { id: "lv_1".into(), max_turns: 10 }, "levels/level_1.level");
    /// ```
    fn insert_asset<A: HmrSource<Config = A> + Clone>(
        &mut self,
        id: AssetId<A>,
        asset: A,
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

    /// Attach business-impact notification to an **arbitrary** `Asset` type —
    /// no `ConfigDiff` / `HmrSource` required.
    ///
    /// This is the "lighter" alternative to [`register_asset`](Self::register_asset)
    /// for asset types where you only need change-notification + entity
    /// binding tracking, not id-level diffing. Typical use cases: `Image`,
    /// `AudioSource`, `Mesh`, fonts, and model asset types.
    ///
    /// Bevy's `AssetServer` (with `bevy/file_watcher`) handles the actual
    /// file reloading and GPU upload for built-in types. This method adds:
    ///
    /// 1. **Entity binding** via `AssetBind<A>` — attach to entities so the
    ///    framework knows which entities depend on a given handle.
    /// 2. **Change notification** via `AssetChanged<A>` — dispatched on
    ///    `AssetEvent::Added` / `Modified`. The message carries the asset id,
    ///    not a clone of the asset; query `Assets<A>` when the value is needed.
    ///
    /// Unlike `register_asset`, this does **not**:
    /// - require `ConfigDiff` or `HmrSource`
    /// - register a `ConfigLoader` (use your own loader / Bevy's built-ins)
    /// - create a `LastSnapshot` or `RefreshDebouncer` (no diff/debounce)
    /// - dispatch `ConfigRefresh` / `ConfigRemoved`
    /// - load any file (you call `asset_server.load::<A>(path)` yourself)
    ///
    /// You **must** call `app.init_asset::<A>()` yourself (or use
    /// `DefaultPlugins` which handles it for built-in types).
    ///
    /// # Example
    /// ```ignore
    /// // Requires `bevy/file_watcher` feature + a real asset type like Image.
    /// use bevy::prelude::*;
    /// use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin, AssetBind};
    /// let mut app = App::new();
    /// app.add_plugins(DefaultPlugins); // 需在 Cargo.toml 显式启用 `bevy/file_watcher` feature
    /// app.add_plugins(ConfigHmrPlugin::default());
    /// app.watch_asset::<Image>();
    /// // Then, wherever you load:
    /// let handle = asset_server.load::<Image>("textures/player.png");
    /// commands.spawn((Sprite::from_image(handle.clone()), AssetBind::new(handle)));
    /// // Subscribe via MessageReader<AssetChanged<Image>> in your system.
    /// ```
    fn watch_asset<A: Asset>(&mut self) -> &mut Self;
}

impl ConfigHmrAppExt for App {
    fn register_config<T: HmrAsset>(&mut self, path: &str) -> &mut Self {
        register_config_impl::<T>(self, path, true);
        self
    }

    fn register_config_validator<T, F>(&mut self, validator: F) -> &mut Self
    where
        T: HmrAsset,
        F: Fn(&T) -> Result<(), String> + Send + Sync + 'static,
    {
        if let Some(mut resource) = self.world_mut().get_resource_mut::<ConfigValidator<T>>() {
            resource.set(validator);
        } else {
            bevy::log::warn!(
                "[HMR] register_config_validator::<{}> called before register_config",
                std::any::type_name::<T>()
            );
        }
        self
    }

    fn register_config_size_estimator<T: HmrAsset>(
        &mut self,
        estimator: fn(&T) -> usize,
    ) -> &mut Self {
        if let Some(mut metrics) = self
            .world_mut()
            .get_resource_mut::<crate::metrics::HmrMetrics<ConfigAsset<T>>>()
        {
            metrics.set_size_estimator(estimator);
        } else {
            bevy::log::warn!(
                "[HMR] register_config_size_estimator::<{}> called before register_config",
                std::any::type_name::<T>()
            );
        }
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

    fn insert_asset<A: HmrSource<Config = A> + Clone>(
        &mut self,
        id: AssetId<A>,
        asset: A,
        source_path: &str,
    ) -> &mut Self {
        // Insert into Assets<A>.
        {
            let mut assets = self.world_mut().resource_mut::<Assets<A>>();
            let _ = assets.insert(id, asset.clone());
        }
        // Eagerly initialize both the snapshot map and the source_path entry
        // so subscribers won't receive a spurious "first load" refresh and so
        // ConfigRemoved events can still carry the path after removal.
        {
            let mut snapshots = self.world_mut().resource_mut::<LastSnapshot<A>>();
            snapshots.map.insert(id, asset);
            if !source_path.is_empty() {
                snapshots.source_paths.insert(id, source_path.to_string());
            }
        }
        self
    }

    fn register_asset<A: HmrSource>(&mut self, path: &str) -> &mut Self {
        register_asset_impl::<A>(self, path, true);
        self
    }

    fn setup_hmr_headless(&mut self) -> &mut Self {
        self.world_mut()
            .resource_mut::<MessageRegistry>()
            .should_update = ShouldUpdateMessages::Always;
        self
    }

    fn watch_asset<A: Asset>(&mut self) -> &mut Self {
        use crate::watcher::AssetBindCache;

        // Per-type resources for entity binding tracking.
        self.init_resource::<AssetBindCache<A>>();

        // Register the AssetChanged<A> + AssetRemoved<A> messages.
        self.add_message::<crate::watcher::AssetChanged<A>>();
        self.add_message::<crate::watcher::AssetRemoved<A>>();

        // Systems: registry -> cleanup -> watcher, chained every frame.
        #[cfg(feature = "dev")]
        {
            use crate::watcher::{
                asset_bind_cleanup_system, asset_bind_registry_system, asset_watcher_system,
            };

            self.add_systems(
                Update,
                (
                    asset_bind_registry_system::<A>,
                    asset_bind_cleanup_system::<A>,
                    asset_watcher_system::<A>,
                )
                    .chain()
                    .after(crate::ReflectHandleTrackingSet),
            );
        }

        self
    }
}

/// Startup 系统：从 `ConfigPathRegistry` 读取 `A` 对应的全部 paths，
/// 通过 `AssetServer` 加载文件，并用 `ConfigHandle<A>` Resource 持有
/// 强引用 handles 防止资产被回收。
///
/// 由 [`App::register_config`](ConfigHmrAppExt::register_config) 自动注册
/// （包装模式下 `A = ConfigAsset<T>`），用户无需手动调用。直接模式下
/// 用户可手动注册 `load_config_at_startup::<MyAsset>` 并确保
/// `ConfigPathRegistry` 中以 `MyAsset::type_path()` 为 key 存入 path。
pub fn load_config_at_startup<A: HmrSource>(
    asset_server: Res<bevy::asset::AssetServer>,
    registry: Res<ConfigPathRegistry>,
    mut commands: Commands,
    existing: Option<Res<ConfigHandle<A>>>,
) {
    // `take_over_handle` means another loader already owns the handles.
    let paths = registry
        .paths
        .get(A::type_path())
        .cloned()
        .unwrap_or_default();
    if paths.is_empty() {
        bevy::log::warn!(
            "[HMR] no path registered for {} (register_config not called?)",
            A::type_path()
        );
        return;
    }
    let mut holder = existing
        .map(|resource| (*resource).clone())
        .unwrap_or_else(|| ConfigHandle {
            handles: Vec::new(),
        });
    for path in paths {
        holder.push(asset_server.load::<A>(&path));
        bevy::log::info!("[HMR] registered config: {} -> {}", A::type_path(), path);
    }
    commands.insert_resource(holder);
    // tracing 宏在无 subscriber 时为空操作，不会 panic；无 LogPlugin
    // 的测试环境也不会触发 IoTaskPool 依赖（bevy 0.19 已解耦）。
}

// ===========================================================================
// 公共 helper：注册逻辑（供 ConfigHmrAppExt 和 HmrAutoWatchPlugin 共用）
// ===========================================================================

/// 包装模式注册核心逻辑。`autoload = true` 时注册 Startup 加载系统；
/// `autoload = false` 时不注册（由 `take_over_handle` 已持有 handle）。
///
/// 设为 `pub` 是因为 `#[derive(HmrAutoWatch)]` 宏需要跨 crate 调用它。
pub fn register_config_impl<T: HmrAsset>(app: &mut App, path: &str, autoload: bool) {
    let asset_type_installed = app
        .world()
        .get_resource::<HmrTypeInstalled<ConfigAsset<T>>>()
        .is_some();
    let debounce_window = app
        .world()
        .get_resource::<HmrSettings>()
        .map(|s| s.debounce_window)
        .unwrap_or_else(|| Duration::from_millis(150));

    if !asset_type_installed {
        let (loader, validator) = ConfigLoader::<T>::with_validator();
        app.insert_resource(validator)
            .register_asset_loader(loader)
            .init_asset::<ConfigAsset<T>>()
            .init_asset::<T>()
            .init_resource::<HandleEntityCache<ConfigAsset<T>>>()
            .init_resource::<RefreshDebouncer<ConfigAsset<T>>>()
            .init_resource::<LastSnapshot<ConfigAsset<T>>>()
            .init_resource::<crate::metrics::HmrMetrics<ConfigAsset<T>>>()
            .init_resource::<crate::view::AssetRevision<ConfigAsset<T>>>()
            .add_message::<ConfigRefresh<T>>()
            .add_message::<ConfigRemoved<T>>()
            .add_message::<ConfigReloadFailed<T>>()
            .add_message::<crate::view::ConfigViewSync<ConfigAsset<T>>>();
    }

    init_shared_dependency_resources(app);

    #[cfg(feature = "dev")]
    if !asset_type_installed {
        app.add_systems(
            Update,
            (
                crate::binding::config_binding_registry_system::<ConfigAsset<T>>,
                crate::binding::config_binding_cleanup_system::<ConfigAsset<T>>,
                crate::core::hmr_core_system::<ConfigAsset<T>>,
                crate::core::asset_load_failed_system::<ConfigAsset<T>>,
                crate::debounce::flush_debounced_refresh::<ConfigAsset<T>>,
                crate::dependency::dependency_registry_system::<ConfigAsset<T>>,
                crate::dependency::dependency_cleanup_system::<ConfigAsset<T>>,
                crate::dependency::cascade_dispatch_system::<ConfigAsset<T>>,
                crate::view::route_active_config_views::<ConfigAsset<T>>,
                crate::view::sync_activated_config_views::<ConfigAsset<T>>,
            )
                .chain()
                .after(crate::ReflectHandleTrackingSet),
        )
        .add_systems(
            Update,
            crate::core::cache_validation_system::<ConfigAsset<T>>.run_if(
                bevy::time::common_conditions::on_timer(Duration::from_secs(30)),
            ),
        );
    }

    app.world_mut()
        .resource_mut::<RefreshDebouncer<ConfigAsset<T>>>()
        .window = debounce_window;

    app.world_mut()
        .resource_mut::<ConfigPathRegistry>()
        .register(ConfigAsset::<T>::type_path().to_string(), path);

    if !asset_type_installed {
        app.insert_resource(HmrTypeInstalled::<ConfigAsset<T>>(PhantomData));
    }

    let startup_registered = app
        .world()
        .get_resource::<HmrStartupRegistered<ConfigAsset<T>>>()
        .is_some();
    if autoload && !startup_registered {
        app.add_systems(Startup, load_config_at_startup::<ConfigAsset<T>>);
        app.insert_resource(HmrStartupRegistered::<ConfigAsset<T>>(PhantomData));
    }
}

/// 直接模式注册核心逻辑。`autoload` 语义同 [`register_config_impl`]。
///
/// 设为 `pub` 是因为 `#[derive(HmrAutoWatch)]` 宏需要跨 crate 调用它。
pub fn register_asset_impl<A: HmrSource>(app: &mut App, path: &str, autoload: bool) {
    let asset_type_installed = app.world().get_resource::<HmrTypeInstalled<A>>().is_some();
    let debounce_window = app
        .world()
        .get_resource::<HmrSettings>()
        .map(|s| s.debounce_window)
        .unwrap_or_else(|| Duration::from_millis(150));

    if !asset_type_installed {
        app.init_resource::<HandleEntityCache<A>>()
            .init_resource::<RefreshDebouncer<A>>()
            .init_resource::<LastSnapshot<A>>()
            .init_resource::<crate::metrics::HmrMetrics<A>>()
            .init_resource::<crate::view::AssetRevision<A>>()
            .add_message::<ConfigRefresh<A::Config>>()
            .add_message::<ConfigRemoved<A::Config>>()
            .add_message::<ConfigReloadFailed<A::Config>>()
            .add_message::<crate::view::ConfigViewSync<A>>();
    }

    init_shared_dependency_resources(app);

    #[cfg(feature = "dev")]
    if !asset_type_installed {
        app.add_systems(
            Update,
            (
                crate::binding::config_binding_registry_system::<A>,
                crate::binding::config_binding_cleanup_system::<A>,
                crate::core::hmr_core_system::<A>,
                crate::core::asset_load_failed_system::<A>,
                crate::debounce::flush_debounced_refresh::<A>,
                crate::dependency::dependency_registry_system::<A>,
                crate::dependency::dependency_cleanup_system::<A>,
                crate::dependency::cascade_dispatch_system::<A>,
                crate::view::route_active_config_views::<A>,
                crate::view::sync_activated_config_views::<A>,
            )
                .chain()
                .after(crate::ReflectHandleTrackingSet),
        )
        .add_systems(
            Update,
            crate::core::cache_validation_system::<A>.run_if(
                bevy::time::common_conditions::on_timer(Duration::from_secs(30)),
            ),
        );
    }

    app.world_mut().resource_mut::<RefreshDebouncer<A>>().window = debounce_window;

    app.world_mut()
        .resource_mut::<ConfigPathRegistry>()
        .register(A::type_path().to_string(), path);

    if !asset_type_installed {
        app.insert_resource(HmrTypeInstalled::<A>(PhantomData));
    }

    let startup_registered = app
        .world()
        .get_resource::<HmrStartupRegistered<A>>()
        .is_some();
    if autoload && !startup_registered {
        app.add_systems(Startup, load_config_at_startup::<A>);
        app.insert_resource(HmrStartupRegistered::<A>(PhantomData));
    }
}

/// 初始化共享依赖资源（幂等：已存在则跳过）。
fn init_shared_dependency_resources(app: &mut App) {
    if app
        .world()
        .get_resource::<crate::dependency::DependencyGraph>()
        .is_none()
    {
        app.init_resource::<crate::dependency::DependencyGraph>();
    }
    if app
        .world()
        .get_resource::<crate::dependency::CascadeQueue>()
        .is_none()
    {
        app.init_resource::<crate::dependency::CascadeQueue>();
    }
}

/// 接管一个已加载的 `Handle<A>`，将其接入 HMR 框架。
///
/// 用于 `bevy_asset_loader` 兼容场景：`LoadingState` 已经把文件加载完并把
/// handle 放进了 `Resource`（`AssetCollection`），本函数在加载完成的状态
/// 切入时调用，复用现有 HMR 注册逻辑（注册系统、消息、快照等），但**不**
/// 重复 `asset_server.load`——而是直接持有传入的 handle 并预热快照。
///
/// - 调用 `register_asset_impl::<A>(app, path, autoload=false)` 注册 HMR 系统。
/// - 用传入的 handle 创建 `ConfigHandle<A>` Resource（持有强引用防回收）。
/// - **预热 `LastSnapshot<A>`**：从 `Assets<A>` 取当前值塞进快照，保证首次
///   `AssetEvent::Added` 不派发伪刷新。
pub fn take_over_handle<A: HmrSource>(app: &mut App, handle: Handle<A>, source_path: &str) {
    register_asset_impl::<A>(app, source_path, false);

    let id = handle.id();

    // 持有强引用
    let mut handles = app
        .world_mut()
        .remove_resource::<ConfigHandle<A>>()
        .unwrap_or_else(|| ConfigHandle::new(handle.clone()));
    handles.push(handle.clone());
    app.insert_resource(handles);

    // 预热快照：若资产已加载则立即记录，避免首次 Added 派发伪刷新。
    if let Some(asset) = app
        .world()
        .get_resource::<Assets<A>>()
        .and_then(|assets| assets.get(id))
    {
        let config = asset.config().clone();
        let mut snapshots = app.world_mut().resource_mut::<LastSnapshot<A>>();
        snapshots.map.insert(id, config);
        if !source_path.is_empty() {
            snapshots.source_paths.insert(id, source_path.to_string());
        }
    }

    bevy::log::info!(
        "[HMR] take_over_handle: {} -> {}",
        A::type_path(),
        source_path
    );
}

/// 在 `&mut World` 上接管一个已加载的 `Handle<A>`（持有 + 预热快照）。
///
/// 这是 [`take_over_handle`] 的 "world 级别" 版本，用于 exclusive system
///（`fn(&mut World)`）内部调用。它**不**注册 HMR 框架系统（那些应在
/// `Plugin::build` 阶段通过 `register_asset_impl` 完成注册），只做：
///
/// - 用传入的 handle 创建 `ConfigHandle<A>` Resource（持有强引用防回收）。
/// - 预热 `LastSnapshot<A>`：从 `Assets<A>` 取当前值塞进快照。
pub fn adopt_handle<A: HmrSource>(
    world: &mut bevy::ecs::world::World,
    handle: Handle<A>,
    source_path: &str,
) {
    let id = handle.id();

    // 持有强引用
    let mut handles = world
        .remove_resource::<ConfigHandle<A>>()
        .unwrap_or_else(|| ConfigHandle::new(handle.clone()));
    handles.push(handle.clone());
    world.insert_resource(handles);

    // 预热快照：若资产已加载则立即记录，避免首次 Added 派发伪刷新。
    if let Some(asset) = world
        .get_resource::<Assets<A>>()
        .and_then(|assets| assets.get(id))
    {
        let config = asset.config().clone();
        let mut snapshots = world.resource_mut::<LastSnapshot<A>>();
        snapshots.map.insert(id, config);
        if !source_path.is_empty() {
            snapshots.source_paths.insert(id, source_path.to_string());
        }
    }

    bevy::log::info!("[HMR] adopt_handle: {} -> {}", A::type_path(), source_path);
}

// ===========================================================================
// HmrAutoWatch: bevy_asset_loader 兼容层
// ===========================================================================

use bevy::app::Plugin;

/// 由 `#[derive(HmrAutoWatch)]` 实现的 trait。
///
/// 用户通常不直接调用这些方法——derive 宏生成 `hmr_plugin(state)` 方法返回
/// 一个 [`HmrAutoWatchPlugin`]，用户只需 `app.add_plugins(MyAssets::hmr_plugin(GameState::Ready))`。
/// 带静态 `#[asset(path = "...")]` 的 `Handle<ConfigAsset<T>>` 会自动接入；
/// 直接模式的 `Handle<A: HmrSource>` 需要在字段上显式增加 `#[hmr_watch]`。
///
/// 该 trait 本身的作用是为宏提供一个"标记"类型，使 derive 宏可以附加到
/// 任何实现了 `Resource` 的结构体上。实际的安装逻辑全部由宏生成的
/// `hmr_plugin` 方法内联展开，不依赖 trait 方法。
pub trait HmrAutoWatch: Resource {}

/// 由 `#[derive(HmrAutoWatch)]` 生成的 `hmr_plugin(state)` 返回的 Plugin。
///
/// 内部持有一个闭包，闭包在 `Plugin::build` 时调用
/// `app.add_systems(OnEnter(state), install_system)`，其中 `install_system`
/// 从 `Res<C>` 取出每个字段的 handle 并调用 [`take_over_handle`]。
pub struct HmrAutoWatchPlugin {
    installer: Box<dyn Fn(&mut App) + Send + Sync + 'static>,
}

impl HmrAutoWatchPlugin {
    /// 构造一个 Plugin，`installer` 闭包在 `build` 时被调用。
    pub fn new(installer: Box<dyn Fn(&mut App) + Send + Sync + 'static>) -> Self {
        Self { installer }
    }
}

impl Plugin for HmrAutoWatchPlugin {
    fn build(&self, app: &mut App) {
        (self.installer)(app);
    }
}
