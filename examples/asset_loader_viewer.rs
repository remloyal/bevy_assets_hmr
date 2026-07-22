//! bevy_asset_loader 窗口版查看器：
//! 用 `LoadingState` 加载 `ConfigAsset<MyConfig>`，
//! 加载完成后由 `HmrAutoWatch::hmr_plugin(Ready)` 自动接管 HMR，
//! 窗口 UI 显示配置内容，修改 `assets/data/config.ron` 后实时刷新。
//!
//! 运行：`cargo run --example asset_loader_viewer -p bevy_assets_hmr`

use bevy::asset::{Asset, Assets};
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_asset_loader::prelude::*;
use bevy_assets_hmr::{
    ConfigAsset, ConfigHmrPlugin, ConfigRefresh, HmrAutoWatch, SimpleConfigDiff,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;

/// 游戏状态：Loading -> Ready。
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

/// 资产集合：bevy_asset_loader 加载 + HmrAutoWatch 自动接 HMR。
#[derive(AssetCollection, Resource, HmrAutoWatch)]
struct GameAssets {
    #[asset(path = "data/config.ron")]
    cfg: Handle<ConfigAsset<MyConfig>>,
}

#[derive(Component)]
struct StatusText;

fn format_config(cfg: &MyConfig) -> String {
    format!(
        "title: {}\nmax_players: {}\n\n💡 修改 assets/data/config.ron 后窗口会自动更新。",
        cfg.title, cfg.max_players
    )
}

/// Startup：生成 Camera2d + 容器 Node + 标题（不含 StatusText，等异步加载完成）。
fn setup_camera(mut commands: Commands) {
    commands.spawn(Camera2d);
    commands
        .spawn(Node {
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            flex_direction: FlexDirection::Column,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            ..default()
        })
        .with_children(|parent| {
            parent.spawn((
                Text::new("📦 bevy_assets_hmr - asset_loader 查看器"),
                TextFont {
                    font_size: FontSize::Px(28.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
        });
}

/// Update：进入 Ready 状态后，等 Asset 加载完成生成 StatusText 实体。
fn maybe_spawn_status_text(
    mut commands: Commands,
    assets: Res<Assets<ConfigAsset<MyConfig>>>,
    game_assets: Option<Res<GameAssets>>,
    mut spawned: Local<bool>,
) {
    if *spawned {
        return;
    }
    let Some(game) = game_assets else {
        return;
    };
    let Some(ca) = assets.get(game.cfg.id()) else {
        return;
    };
    commands.spawn((
        Text::new(format_config(&ca.raw)),
        TextFont {
            font_size: FontSize::Px(20.0),
            ..default()
        },
        TextColor(Color::srgb(0.7, 0.9, 0.7)),
        StatusText,
    ));
    *spawned = true;
}

/// HMR 刷新时更新文本。
fn refresh_text(
    mut reader: MessageReader<ConfigRefresh<MyConfig>>,
    mut q: Query<&mut Text, With<StatusText>>,
) {
    for refresh in reader.read() {
        if let Ok(mut t) = q.single_mut() {
            **t = format_config(&refresh.new_config);
        }
    }
}

fn main() {
    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(bevy::asset::AssetPlugin {
                file_path: std::env::var("CARGO_MANIFEST_DIR")
                    .map(|d| format!("{}/assets", d))
                    .unwrap_or_else(|_| "assets".into()),
                ..Default::default()
            })
            .set(bevy::window::WindowPlugin {
                primary_window: Some(Window {
                    title: "bevy_assets_hmr - asset_loader 查看器".into(),
                    ..default()
                }),
                ..default()
            }),
    );
    app.add_plugins(ConfigHmrPlugin {
        debounce_window: Duration::from_millis(0),
    });
    app.init_state::<GameState>();

    // bevy_asset_loader 标准流程：Loading -> Ready
    app.add_loading_state(
        LoadingState::new(GameState::Loading)
            .continue_to_state(GameState::Ready)
            .load_collection::<GameAssets>(),
    );

    // 一行把 GameAssets 中所有 Handle<A: HmrSource> 字段接入 HMR
    app.add_plugins(GameAssets::hmr_plugin(GameState::Ready));

    app.add_systems(Startup, setup_camera);
    app.add_systems(Update, (maybe_spawn_status_text, refresh_text));

    app.run();
}
