//! 兼容 `bevy_asset_loader` 示例：用 `#[derive(AssetCollection, HmrAutoWatch)]`
//! 声明资产集合，加载完成后自动接入 HMR。
//!
//! 运行：`cargo run --example asset_loader_compat -p bevy_assets_hmr`
//!
//! 修改 `assets/data/config.ron` 后，控制台会收到 `ConfigRefresh<MyConfig>`。

use bevy::app::ScheduleRunnerPlugin;
use bevy::asset::Asset;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_asset_loader::prelude::*;
use bevy_assets_hmr::{
    ConfigAsset, ConfigHmrPlugin, ConfigRefresh, HmrAutoWatch, SimpleConfigDiff,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(States, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
enum GameState {
    #[default]
    Loading,
    Ready,
}

/// 配置数据类型（包装模式）。
#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
struct MyConfig {
    title: String,
    max_players: u32,
}

impl SimpleConfigDiff for MyConfig {
    fn diff_id() -> &'static str {
        "my_config"
    }
}

/// 资产集合：`bevy_asset_loader` 加载 + `HmrAutoWatch` 自动接 HMR。
#[derive(AssetCollection, Resource, HmrAutoWatch)]
struct GameAssets {
    #[asset(path = "data/config.ron")]
    cfg: Handle<ConfigAsset<MyConfig>>,
}

fn print_loaded_config(game_assets: Res<GameAssets>, configs: Res<Assets<ConfigAsset<MyConfig>>>) {
    let config = configs
        .get(&game_assets.cfg)
        .expect("LoadingState entered Ready without the config asset");
    println!("=== bevy_asset_loader 加载完成 ===");
    println!("  title: {}", config.raw.title);
    println!("  max_players: {}", config.raw.max_players);
    println!("现在修改 assets/data/config.ron 可触发 HMR。\n");
}

/// 订阅 HMR 刷新事件。
fn on_config_refresh(mut reader: bevy::ecs::message::MessageReader<ConfigRefresh<MyConfig>>) {
    for refresh in reader.read() {
        println!("\n=== 收到 HMR 刷新 ===");
        println!("  source_path: {}", refresh.source_path);
        println!("  new title: {}", refresh.new_config.title);
        println!("  new max_players: {}", refresh.new_config.max_players);
        println!("  changed_ids: {:?}", refresh.changed_ids);
    }
}

fn main() {
    App::new()
        .add_plugins((
            MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(16))),
            bevy::state::app::StatesPlugin,
            bevy::asset::AssetPlugin {
                file_path: std::env::var("CARGO_MANIFEST_DIR")
                    .map(|d| format!("{}/assets", d))
                    .unwrap_or_else(|_| "assets".into()),
                ..Default::default()
            },
        ))
        .add_plugins(ConfigHmrPlugin::default())
        .init_state::<GameState>()
        .add_loading_state(
            LoadingState::new(GameState::Loading)
                .continue_to_state(GameState::Ready)
                .load_collection::<GameAssets>(),
        )
        .add_plugins(GameAssets::hmr_plugin(GameState::Ready))
        .add_systems(OnEnter(GameState::Ready), print_loaded_config)
        .add_systems(Update, on_config_refresh.run_if(in_state(GameState::Ready)))
        .run();
}
