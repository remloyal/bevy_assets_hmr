//! 直接模式示例：用户自定义 Asset + 自定义 Loader + HMR diff。
//!
//! 运行：`cargo run --example direct_mode -p bevy_assets_hmr`
//!
//! 与 `register_config`（包装模式）的区别：
//! - 用户用自己的 `AssetLoader`（这里是 `LevelAssetLoader`，加载 `.level` 格式）
//! - Asset 本身就是 Config（`LevelAsset: HmrSource<Config = LevelAsset>`）
//! - 不经过 `ConfigAsset<T>` 包装，`ConfigRefresh<LevelAsset>` 的 `new_config` 直接是 `LevelAsset`
//!
//! 适用场景：已有自定义 Asset + Loader 的项目，想在上面加 diff-based 热更新。

use bevy::asset::{Asset, AssetId, AssetLoader, Assets, LoadContext, io::Reader};
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{
    ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh, HmrSource, SimpleConfigDiff,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

/// 关卡数据：用户自定义 Asset，直接作为 HMR 数据源。
#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
struct LevelAsset {
    /// 关卡 id（用于 diff）
    id: String,
    /// 关卡名称
    name: String,
    /// 最大回合数
    max_turns: u32,
}

/// 单对象 diff：整体比较，变了就标记 "level" 为 modified。
impl SimpleConfigDiff for LevelAsset {
    fn diff_id() -> &'static str {
        "level"
    }
}

/// HmrSource 实现：Asset 本身就是 Config，直接返回 self。
impl HmrSource for LevelAsset {
    type Config = LevelAsset;
    fn config(&self) -> &Self::Config {
        self
    }
    fn from_config(config: Self::Config, _source_path: String) -> Self {
        // 直接模式：Asset 就是 Config，直接返回
        config
    }
}

/// 自定义 Loader：解析 `.level` 格式（实际就是 ron，演示用）。
#[derive(Default, TypePath)]
struct LevelAssetLoader;

#[derive(Debug, thiserror::Error)]
enum LevelLoaderError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ron parse error: {0}")]
    Ron(#[from] ron::error::SpannedError),
}

impl AssetLoader for LevelAssetLoader {
    type Asset = LevelAsset;
    type Settings = ();
    type Error = std::sync::Arc<LevelLoaderError>;

    async fn load(
        &self,
        reader: &mut dyn Reader,
        _settings: &(),
        _load_context: &mut LoadContext<'_>,
    ) -> Result<Self::Asset, Self::Error> {
        let mut bytes = Vec::new();
        reader
            .read_to_end(&mut bytes)
            .await
            .map_err(|e| std::sync::Arc::new(LevelLoaderError::Io(e)))?;
        let asset = ron::de::from_bytes::<LevelAsset>(&bytes)
            .map_err(|e| std::sync::Arc::new(LevelLoaderError::Ron(e)))?;
        Ok(asset)
    }

    fn extensions(&self) -> &[&str] {
        &["level"]
    }
}

/// 业务订阅方：监听 `ConfigRefresh<LevelAsset>`，直接读 `new_config`（无 .raw）。
fn on_level_refresh(mut reader: MessageReader<ConfigRefresh<LevelAsset>>) {
    for refresh in reader.read() {
        println!("\n=== 收到 LevelAsset 刷新事件 ===");
        println!("DiffKind: {:?}", refresh.diff_kind);
        println!(
            "新关卡: id={}, name={}, max_turns={}",
            refresh.new_config.id, refresh.new_config.name, refresh.new_config.max_turns
        );
    }
}

/// 模拟系统：第 3 帧修改关卡数据。
fn simulate(
    mut tick: Local<u32>,
    mut assets: ResMut<Assets<LevelAsset>>,
    level_id: Res<LevelAssetId>,
) {
    *tick += 1;

    if *tick == 3 {
        println!("\n[第 {} 帧] 模拟关卡文件变更：max_turns 10 -> 20", *tick);
        let _ = assets.insert(
            level_id.0,
            LevelAsset {
                id: "lv_1".into(),
                name: "新手村".into(),
                max_turns: 20,
            },
        );
    }

    if *tick == 8 {
        println!("\n[第 {} 帧] 示例完成，退出", *tick);
        std::process::exit(0);
    }
}

#[derive(Resource)]
struct LevelAssetId(AssetId<LevelAsset>);

fn main() {
    // AssetServer::load（由 register_asset 的 Startup 系统触发）需要 IoTaskPool。
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(0).build());

    let mut app = App::new();
    app.add_plugins((bevy::asset::AssetPlugin::default(), bevy::time::TimePlugin));

    // 1. HMR 插件
    app.add_plugins(ConfigHmrPlugin {
        debounce_window: Duration::from_millis(0),
    });
    app.setup_hmr_headless();

    // 2. 用户自己注册 Asset + Loader（和 bevy 官方 custom_asset.rs 一样）
    app.init_asset::<LevelAsset>();
    app.register_asset_loader(LevelAssetLoader);

    // 3. 一行注册 HMR（直接模式，不经过 ConfigAsset 包装）
    app.register_asset::<LevelAsset>("levels/level_1.level");

    // 4. 注入初始数据（直接 insert 到 Assets<LevelAsset>，不是 Assets<ConfigAsset<LevelAsset>>）
    let level_id = AssetId::Uuid {
        uuid: Uuid::from_u128(999),
    };
    let initial = LevelAsset {
        id: "lv_1".into(),
        name: "新手村".into(),
        max_turns: 10,
    };
    {
        let mut assets = app.world_mut().resource_mut::<Assets<LevelAsset>>();
        let _ = assets.insert(level_id, initial.clone());
    }
    // 手动初始化快照（直接模式没有 insert_config 辅助方法）
    {
        let mut snapshots = app
            .world_mut()
            .resource_mut::<bevy_assets_hmr::LastSnapshot<LevelAsset>>();
        snapshots.map.insert(level_id, initial);
    }
    app.insert_resource(LevelAssetId(level_id));

    // 5. 业务系统
    app.add_systems(Update, (on_level_refresh, simulate));

    println!("=== bevy_assets_hmr direct_mode 示例启动 ===");
    println!("模式：用户自定义 Asset + 自定义 Loader + register_asset（直接模式）");
    println!("初始关卡: id=lv_1, name=新手村, max_turns=10");
    println!("第 3 帧模拟文件变更：max_turns -> 20");

    loop {
        app.update();
    }
}
