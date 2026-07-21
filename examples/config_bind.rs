//! 实体定向刷新示例：展示 `ConfigBind<T>` + `HandleEntityCache<T>` 如何
//! 让 HMR 精准定位"绑定了被修改配置"的实体，只刷新它们。
//!
//! 运行：`cargo run --example config_bind -p bevy_assets_hmr`
//!
//! 场景：每个 UI 面板实体挂一个 `ConfigBind<UiTheme>`，当主题配置变更时，
//! HMR 自动找出绑定的面板，订阅方收到 `target_entities` 后只刷新这些实体。

use bevy::asset::{Asset, AssetId, Assets};
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{
    ConfigAsset, ConfigBind, ConfigDiff, ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh,
    HandleEntityCache,
};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::time::Duration;
use uuid::Uuid;

/// UI 主题配置（单文件单对象，diff 简化为"整体修改"）。
#[derive(
    Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default,
)]
struct UiTheme {
    bg_color: String,
    text_color: String,
    font_size: u32,
}

impl ConfigDiff for UiTheme {
    fn diff(
        old: &Self,
        new: &Self,
    ) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
        if old != new {
            (HashSet::new(), HashSet::new(), ["theme".to_string()].into_iter().collect())
        } else {
            (HashSet::new(), HashSet::new(), HashSet::new())
        }
    }
}

#[derive(Component, Debug)]
struct UiPanel {
    label: String,
}

#[derive(Resource)]
struct SharedThemeId(AssetId<ConfigAsset<UiTheme>>);

/// 业务订阅方：收到 `ConfigRefresh<UiTheme>` 后，只刷新 `target_entities`
/// 中的面板。
/// `ConfigRefresh<UiTheme>` 的泛型是 Config 类型（`ConfigAsset<UiTheme>::Config = UiTheme`），
/// 所以 `refresh.new_config` 直接就是 `UiTheme`，无需 `.raw`。
fn ui_panel_refresh(
    mut reader: MessageReader<ConfigRefresh<UiTheme>>,
    mut panels: Query<&mut UiPanel>,
    cache: Res<HandleEntityCache<ConfigAsset<UiTheme>>>,
    theme_id: Res<SharedThemeId>,
) {
    for refresh in reader.read() {
        println!("\n=== 收到 UiTheme 刷新事件 ===");
        println!("target_entities 数: {}", refresh.target_entities.len());

        for entity in &refresh.target_entities {
            if let Ok(mut panel) = panels.get_mut(*entity) {
                println!(
                    "  [定向刷新] 面板 '{}' 应用新主题: bg={}, color={}, size={}",
                    panel.label,
                    refresh.new_config.bg_color,
                    refresh.new_config.text_color,
                    refresh.new_config.font_size
                );
                panel.label = format!("panel@{}", refresh.new_config.bg_color);
            }
        }

        if let Some(entities) = cache.get_entities(&theme_id.0) {
            println!("  [缓存查询] 当前绑定到该主题的实体数: {}", entities.len());
        }
    }
}

/// 模拟系统：第 2 帧创建面板并绑定主题，第 4 帧修改主题，第 9 帧退出。
fn simulate(
    mut tick: Local<u32>,
    mut commands: Commands,
    mut assets: ResMut<Assets<ConfigAsset<UiTheme>>>,
    snapshots: Res<bevy_assets_hmr::LastSnapshot<ConfigAsset<UiTheme>>>,
    theme_id: Res<SharedThemeId>,
) {
    *tick += 1;

    if *tick == 2 {
        println!("\n[第 {} 帧] 创建 3 个 UI 面板，绑定到主题", *tick);
        // 用 ConfigBind::with_id 从 AssetId 构造绑定（headless 演示用）。
        // 实际项目中应使用 ConfigBind::new(asset_server.load::<ConfigAsset<T>>(path))。
        for i in 1..=3 {
            commands.spawn((
                UiPanel {
                    label: format!("panel_{i}"),
                },
                ConfigBind::with_id(theme_id.0),
            ));
        }
    }

    if *tick == 4 {
        println!("\n[第 {} 帧] 修改主题：bg=blue -> bg=dark", *tick);
        let new_theme = UiTheme {
            bg_color: "dark".into(),
            text_color: "white".into(),
            font_size: 16,
        };
        let old_theme = snapshots
            .map
            .get(&theme_id.0)
            .cloned()
            .expect("快照应存在");
        println!("  旧主题: bg={}, size={}", old_theme.bg_color, old_theme.font_size);
        println!("  新主题: bg={}, size={}", new_theme.bg_color, new_theme.font_size);

        let _ = assets.insert(
            theme_id.0,
            ConfigAsset {
                raw: new_theme,
                source_path: "data/ui_theme.ron".into(),
            },
        );
    }

    if *tick == 9 {
        println!("\n[第 {} 帧] 示例完成，退出", *tick);
        std::process::exit(0);
    }
}

fn main() {
    let mut app = App::new();
    app.add_plugins((bevy::asset::AssetPlugin::default(), bevy::time::TimePlugin));

    app.add_plugins(ConfigHmrPlugin {
        debounce_window: Duration::from_millis(0),
    });
    app.setup_hmr_headless();
    app.register_config::<UiTheme>("data/ui_theme.ron");

    // 注入初始主题（快照自动初始化）
    let initial_theme = UiTheme {
        bg_color: "blue".into(),
        text_color: "black".into(),
        font_size: 14,
    };
    let theme_id = AssetId::Uuid {
        uuid: Uuid::from_u128(7),
    };
    app.insert_config::<UiTheme>(theme_id, initial_theme, "data/ui_theme.ron");
    app.insert_resource(SharedThemeId(theme_id));

    app.add_systems(Update, (ui_panel_refresh, simulate));

    println!("=== bevy_assets_hmr config_bind 示例启动 ===");
    println!("初始主题: bg=blue, color=black, size=14");
    println!("第 2 帧创建面板，第 4 帧修改主题，观察 target_entities 是否精准匹配");

    loop {
        app.update();
    }
}
