//! 窗口版查看器：开 bevy 窗口，从 `data/npc.ron` 读取配置并显示，
//! 修改文件后窗口自动更新（需启用 `bevy/file_watcher` feature）。
//!
//! 运行：`cargo run --example basic_viewer -p bevy_assets_hmr`

use bevy::asset::{Asset, Assets};
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{ConfigAsset, ConfigDiff, ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh};
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default, ConfigDiff)]
#[config_diff(field = "npcs", id = "id")]
struct NpcDatabase {
    npcs: Vec<NpcEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
struct NpcEntry {
    id: String,
    name: String,
    hp: u32,
}

#[derive(Component)]
struct StatusText;

/// 格式化 NPC 列表文本。
fn format_npcs(npcs: &[NpcEntry]) -> String {
    let mut s = format!("共 {} 条 NPC：\n", npcs.len());
    for n in npcs {
        s.push_str(&format!("  {} - {} (HP: {})\n", n.id, n.name, n.hp));
    }
    s.push_str("\n💡 修改 assets/data/npc.ron 后窗口会自动更新。");
    s
}

/// Startup 阶段：只生成 Camera + 容器 Node + 标题。
/// 不在此生成 StatusText，因为 asset 异步加载尚未完成。
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
                Text::new("📦 bevy_assets_hmr 配置查看器"),
                TextFont {
                    font_size: FontSize::Px(28.0),
                    ..default()
                },
                TextColor(Color::WHITE),
            ));
        });
}

/// Update 阶段：等 asset 异步加载完成后，一次性生成 StatusText 实体。
/// 用 `Local<bool>` 做一次性保护，避免重复 spawn。
fn maybe_spawn_status_text(
    mut commands: Commands,
    assets: Res<Assets<ConfigAsset<NpcDatabase>>>,
    mut spawned: Local<bool>,
) {
    if *spawned {
        return;
    }
    let Some((_, ca)) = assets.iter().next() else {
        return;
    };
    commands.spawn((
        Text::new(format_npcs(&ca.raw.npcs)),
        TextFont {
            font_size: FontSize::Px(18.0),
            ..default()
        },
        TextColor(Color::srgb(0.7, 0.9, 0.7)),
        StatusText,
    ));
    *spawned = true;
}

/// HMR 刷新时更新文本。
fn refresh_text(
    mut reader: MessageReader<ConfigRefresh<NpcDatabase>>,
    mut q: Query<&mut Text, With<StatusText>>,
) {
    for refresh in reader.read() {
        if let Ok(mut t) = q.single_mut() {
            **t = format_npcs(&refresh.new_config.npcs);
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
                    title: "bevy_assets_hmr - 配置查看器".into(),
                    ..default()
                }),
                ..default()
            }),
    );
    app.add_plugins(ConfigHmrPlugin {
        debounce_window: Duration::from_millis(0),
    });
    app.setup_hmr_headless();
    app.register_config::<NpcDatabase>("data/npc.ron");

    app.add_systems(Startup, setup_camera);
    app.add_systems(Update, (maybe_spawn_status_text, refresh_text));

    app.run();
}
