//! 控制台查看器：启动后显示读取到的配置内容，并提示修改 RON 文件可更新。
//!
//! 运行：`cargo run --example viewer -p bevy_assets_hmr`
//!
//! 修改 `assets/data/npc.ron` 后重新运行本示例即可看到更新内容。
//! 在 GUI 项目中使用 register_config + ConfigRefresh 可实现运行时热重载。

use bevy::asset::{Asset, AssetId, Assets};
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{ConfigAsset, ConfigDiff, ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

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

impl std::fmt::Display for NpcEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} — {} (HP: {})", self.id, self.name, self.hp)
    }
}

#[derive(Resource)]
struct SeedAssetId(AssetId<ConfigAsset<NpcDatabase>>);

fn print_initial_data(assets: Res<Assets<ConfigAsset<NpcDatabase>>>, seed: Res<SeedAssetId>) {
    if let Some(ca) = assets.get(seed.0) {
        println!("╔══════════════════════════════════════╗");
        println!("║  📦 bevy_assets_hmr 配置查看器     ║");
        println!("╠══════════════════════════════════════╣");
        println!("║  共 {} 条 NPC 数据：                ║", ca.raw.npcs.len());
        for npc in &ca.raw.npcs {
            println!("║    {}", npc);
        }
        println!("╠══════════════════════════════════════╣");
        println!("║  💡 提示：                          ║");
        println!("║  修改 assets/data/npc.ron 后         ║");
        println!("║  重新运行即可看到更新内容。          ║");
        println!("║  GUI 项目中可用 ConfigRefresh        ║");
        println!("║  实现运行时自动热重载。              ║");
        println!("╚══════════════════════════════════════╝");
    }
}

fn npc_refresh_subscriber(mut reader: MessageReader<ConfigRefresh<NpcDatabase>>) {
    for refresh in reader.read() {
        println!(
            "\n=== HMR 刷新：NpcDatabase（DiffKind={:?}）===",
            refresh.diff_kind
        );
        for npc in &refresh.new_config.npcs {
            println!("  {}", npc);
        }
    }
}

fn main() {
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(0).build());

    let mut app = App::new();
    app.add_plugins((bevy::asset::AssetPlugin::default(), bevy::time::TimePlugin))
        .add_plugins(ConfigHmrPlugin {
            debounce_window: Duration::from_millis(0),
        })
        .setup_hmr_headless()
        .register_config::<NpcDatabase>("data/npc.ron");

    let initial_db = NpcDatabase {
        npcs: vec![
            NpcEntry {
                id: "npc_1".into(),
                name: "商人".into(),
                hp: 100,
            },
            NpcEntry {
                id: "npc_2".into(),
                name: "守卫".into(),
                hp: 150,
            },
            NpcEntry {
                id: "npc_3".into(),
                name: "法师".into(),
                hp: 80,
            },
        ],
    };
    let asset_id = AssetId::Uuid {
        uuid: Uuid::from_u128(77),
    };
    app.insert_config::<NpcDatabase>(asset_id, initial_db, "data/npc.ron");
    app.insert_resource(SeedAssetId(asset_id));

    app.add_systems(Startup, print_initial_data);
    app.add_systems(Update, npc_refresh_subscriber);

    // 手动运行几帧让初始化完成
    for _ in 0..3 {
        app.update();
    }
}
