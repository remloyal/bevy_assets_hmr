//! Generic config Asset wrapper (包装模式).

use bevy::asset::Asset;
use bevy::prelude::{Handle, Resource};
use bevy::reflect::TypePath;

use crate::core::HmrSource;

/// Wraps a parsed config `T` together with its source path.
///
/// `T` must be an `Asset` itself (Bevy 0.19 `Asset` derive requires this).
/// The consuming crate is responsible for defining `T` (e.g. `NpcDatabase`)
/// with `#[derive(Asset, TypePath, Serialize, Deserialize, Clone, PartialEq)]`.
///
/// `ConfigAsset<T>` 自动实现 `HmrSource`（见 `core.rs`），`type Config = T`。
#[derive(Asset, TypePath, Clone, Debug)]
pub struct ConfigAsset<T: Asset + Send + Sync + 'static> {
    /// The parsed config payload.
    pub raw: T,
    /// Original disk path (relative to `assets/`), for cache mapping & logging.
    pub source_path: String,
}

/// 强引用 Handle 持有者，防止 `A` Asset 被 `AssetServer` 回收。
///
/// `App::register_config::<T>(path)` 自动加载文件并插入
/// `ConfigHandle::<ConfigAsset<T>>` Resource，用户无需手动
/// `asset_server.load` + 持有 handle。
///
/// `bevy/file_watcher` reload 时 `AssetServer` 会更新此 Handle 指向的新版本。
///
/// 在直接模式下，用户用自己的 `A` 类型插入 `ConfigHandle::<A>`。
#[derive(Resource)]
#[allow(dead_code)]
pub struct ConfigHandle<A: HmrSource> {
    /// 强引用 handle。字段下划线前缀避免 "field is never read" 警告——
    /// 它的存在本身就是作用（防止资产被回收）。
    pub _handle: Handle<A>,
}
