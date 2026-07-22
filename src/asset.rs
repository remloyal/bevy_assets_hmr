//! Generic config Asset wrapper (包装模式).

use bevy::asset::Asset;
use bevy::prelude::{Handle, Resource};
use bevy::reflect::TypePath;

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
/// `asset_server.load` + 持有 handle。多次注册同一类型会保留所有去重后的
/// handles。
///
/// `bevy/file_watcher` reload 时 `AssetServer` 会更新此 Handle 指向的新版本。
///
/// 在直接模式下，用户用自己的 `A` 类型插入 `ConfigHandle::<A>`。
///
/// `watch_asset` 模式下也用此 Resource 持有非 `HmrSource` 资产（如 `Image`）
/// 的强引用。
#[derive(Resource)]
#[allow(dead_code)]
pub struct ConfigHandle<A: Asset> {
    /// Strong handles retained for this asset type.
    pub handles: Vec<Handle<A>>,
}

impl<A: Asset> Clone for ConfigHandle<A> {
    fn clone(&self) -> Self {
        Self {
            handles: self.handles.clone(),
        }
    }
}

impl<A: Asset> ConfigHandle<A> {
    /// Create a holder with one strong handle.
    pub fn new(handle: Handle<A>) -> Self {
        Self {
            handles: vec![handle],
        }
    }

    /// Retain a handle unless the same AssetId is already present.
    pub fn push(&mut self, handle: Handle<A>) {
        if !self
            .handles
            .iter()
            .any(|existing| existing.id() == handle.id())
        {
            self.handles.push(handle);
        }
    }
}
