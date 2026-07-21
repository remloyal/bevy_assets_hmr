//! 基础示例：定义 Asset -> 实现 ConfigDiff -> 一行注册 -> 模拟变更 -> 订阅事件。
//!
//! 运行：`cargo run --example basic -p bevy_assets_hmr`
//!
//! 由于 `bevy/file_watcher` 在当前 crate 中未启用（上游依赖版本问题），
//! 本示例用 `Assets::insert` 手动模拟文件变更。`Assets::insert` 会自动
//! 触发 `AssetEvent`，HMR 核心自动 diff + 派发，无需手动写事件。

use bevy::asset::{Asset, AssetId, Assets, Handle};
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{
    ConfigAsset, ConfigDiff, ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh,
};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

/// 一个简单的"NPC 数据表" Asset。
#[derive(
    Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default, ConfigDiff,
)]
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

impl NpcDatabase {
    fn lookup(&self, id: &str) -> Option<&NpcEntry> {
        self.npcs.iter().find(|n| n.id == id)
    }
}

/// 业务订阅方：监听 `ConfigRefresh<NpcDatabase>` 并打印变更摘要。
/// 新版 API 下，订阅方不再需要过滤空 diff（核心层已自动跳过）。
/// `ConfigRefresh<NpcDatabase>` 的泛型是 Config 类型（`ConfigAsset<NpcDatabase>::Config = NpcDatabase`），
/// 所以 `refresh.new_config` 直接就是 `NpcDatabase`，无需 `.raw`。
fn npc_refresh_subscriber(mut reader: MessageReader<ConfigRefresh<NpcDatabase>>) {
    for refresh in reader.read() {
        println!("\n=== 收到 NpcDatabase 刷新事件 ===");
        println!(
            "DiffKind: {:?}, 变更 id 数: {}",
            refresh.diff_kind,
            refresh.changed_ids.len()
        );

        let new_db = &refresh.new_config;
        for id in &refresh.changed_ids {
            if let Some(npc) = new_db.lookup(id) {
                println!("  [新增/修改] {} -> name={}, hp={}", id, npc.name, npc.hp);
            } else {
                println!("  [删除] {}", id);
            }
        }
    }
}

/// 模拟"磁盘文件被修改"的系统：用 `Assets::insert` 替换数据库内容。
fn simulate_file_change(
    mut tick: Local<u32>,
    mut assets: ResMut<Assets<ConfigAsset<NpcDatabase>>>,
    snapshots: Res<bevy_assets_hmr::LastSnapshot<ConfigAsset<NpcDatabase>>>,
    asset_id: Res<SeedAssetId>,
) {
    *tick += 1;

    if *tick == 2 {
        println!("\n[第 {} 帧] 模拟文件变更：修改 npc_1，新增 npc_3", *tick);
        let old_db = snapshots
            .map
            .get(&asset_id.0)
            .cloned()
            .expect("快照应已自动初始化");

        let mut new_npcs = old_db.npcs.clone();
        if let Some(n1) = new_npcs.iter_mut().find(|n| n.id == "npc_1") {
            n1.hp = 200;
        }
        new_npcs.push(NpcEntry {
            id: "npc_3".into(),
            name: "新角色".into(),
            hp: 80,
        });

        let _ = assets.insert(
            asset_id.0,
            ConfigAsset {
                raw: NpcDatabase { npcs: new_npcs },
                source_path: "data/npc.ron".into(),
            },
        );
    }

    if *tick == 4 {
        println!("\n[第 {} 帧] 模拟文件变更：删除 npc_2", *tick);
        let old_db = snapshots.map.get(&asset_id.0).cloned().expect("快照应存在");

        let new_npcs: Vec<_> = old_db
            .npcs
            .iter()
            .filter(|n| n.id != "npc_2")
            .cloned()
            .collect();
        let _ = assets.insert(
            asset_id.0,
            ConfigAsset {
                raw: NpcDatabase { npcs: new_npcs },
                source_path: "data/npc.ron".into(),
            },
        );
    }

    if *tick == 8 {
        println!("\n[第 {} 帧] 示例完成，退出", *tick);
        std::process::exit(0);
    }
}

#[derive(Resource)]
struct SeedAssetId(AssetId<ConfigAsset<NpcDatabase>>);

fn main() {
    let mut app = App::new();
    app.add_plugins((bevy::asset::AssetPlugin::default(), bevy::time::TimePlugin));

    // 1. 一行接入 HMR 插件（debounce_window=0 让测试立即触发）
    app.add_plugins(ConfigHmrPlugin {
        debounce_window: Duration::from_millis(0),
    });
    // 2. headless 环境一行配置（无渲染世界时启用 Messages 缓冲交换）
    app.setup_hmr_headless();
    // 3. 一行注册配置类型（自动注册 loader / 资源 / 系统 / 消息）
    app.register_config::<NpcDatabase>("data/npc.ron");

    // 4. 注入初始数据（快照由 HMR 核心自动初始化，无需手动操作 LastSnapshot）
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
        ],
    };
    let asset_id = AssetId::Uuid {
        uuid: Uuid::from_u128(42),
    };
    app.insert_config::<NpcDatabase>(asset_id, initial_db, "data/npc.ron");
    app.insert_resource(SeedAssetId(asset_id));

    // 5. 添加业务订阅系统和模拟系统
    app.add_systems(Update, (npc_refresh_subscriber, simulate_file_change));

    println!("=== bevy_assets_hmr basic 示例启动 ===");
    println!("初始数据库：npc_1(商人, hp=100), npc_2(守卫, hp=150)");
    println!("将在第 2 帧修改 npc_1 并新增 npc_3，第 4 帧删除 npc_2");

    // 用于消除未使用警告
    let _handle: Handle<ConfigAsset<NpcDatabase>> = Handle::default();

    // 手动驱动主循环（无窗口环境；simulate_file_change 会在第 8 帧退出）
    loop {
        app.update();
    }
}
