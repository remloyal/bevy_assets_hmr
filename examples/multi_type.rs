//! 多类型注册示例：同时为多种数据库启用 HMR，每种类型独立 diff、独立订阅。
//!
//! 运行：`cargo run --example multi_type -p bevy_assets_hmr`
//!
//! 场景：一个游戏同时有 NpcDatabase 和 ItemDatabase 两种配置表，
//! 两种表都有各自的订阅系统。修改任意一个表，只有对应的订阅系统收到事件。

use bevy::asset::{Asset, AssetId, Assets};
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{ConfigAsset, ConfigDiff, ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh};
use serde::{Deserialize, Serialize};
use std::time::Duration;
use uuid::Uuid;

// ============ NpcDatabase ============

#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default, ConfigDiff)]
#[config_diff(field = "npcs", id = "id")]
struct NpcDatabase {
    npcs: Vec<NpcEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
struct NpcEntry {
    id: String,
    hp: u32,
}

// ============ ItemDatabase ============

#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default, ConfigDiff)]
#[config_diff(field = "items", id = "id")]
struct ItemDatabase {
    items: Vec<ItemEntry>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
struct ItemEntry {
    id: String,
    price: u32,
}

// ============ 订阅方 ============

fn npc_subscriber(mut reader: MessageReader<ConfigRefresh<NpcDatabase>>) {
    for refresh in reader.read() {
        println!("[NPC] 收到刷新，变更 id: {:?}", refresh.changed_ids);
    }
}

fn item_subscriber(mut reader: MessageReader<ConfigRefresh<ItemDatabase>>) {
    for refresh in reader.read() {
        println!("[ITEM] 收到刷新，变更 id: {:?}", refresh.changed_ids);
    }
}

// ============ 模拟系统 ============

#[derive(Resource)]
struct SeedIds {
    npc: AssetId<ConfigAsset<NpcDatabase>>,
    item: AssetId<ConfigAsset<ItemDatabase>>,
}

fn simulate(
    mut tick: Local<u32>,
    ids: Res<SeedIds>,
    mut npc_assets: ResMut<Assets<ConfigAsset<NpcDatabase>>>,
    mut item_assets: ResMut<Assets<ConfigAsset<ItemDatabase>>>,
    npc_snapshots: Res<bevy_assets_hmr::LastSnapshot<ConfigAsset<NpcDatabase>>>,
    item_snapshots: Res<bevy_assets_hmr::LastSnapshot<ConfigAsset<ItemDatabase>>>,
) {
    *tick += 1;

    if *tick == 2 {
        println!("\n[第 {} 帧] 只修改 NpcDatabase（新增 npc_2）", *tick);
        let old = npc_snapshots
            .map
            .get(&ids.npc)
            .cloned()
            .expect("npc 快照应存在");
        let mut new_npcs = old.npcs.clone();
        new_npcs.push(NpcEntry {
            id: "npc_2".into(),
            hp: 200,
        });
        let _ = npc_assets.insert(
            ids.npc,
            ConfigAsset {
                raw: NpcDatabase { npcs: new_npcs },
                source_path: "data/npc.ron".into(),
            },
        );
    }

    if *tick == 4 {
        println!(
            "\n[第 {} 帧] 只修改 ItemDatabase（修改 item_1 价格）",
            *tick
        );
        let old = item_snapshots
            .map
            .get(&ids.item)
            .cloned()
            .expect("item 快照应存在");
        let new_items: Vec<_> = old
            .items
            .iter()
            .map(|i| {
                if i.id == "item_1" {
                    ItemEntry {
                        id: i.id.clone(),
                        price: i.price + 50,
                    }
                } else {
                    i.clone()
                }
            })
            .collect();
        let _ = item_assets.insert(
            ids.item,
            ConfigAsset {
                raw: ItemDatabase { items: new_items },
                source_path: "data/item.ron".into(),
            },
        );
    }

    if *tick == 8 {
        println!("\n[第 {} 帧] 示例完成，退出", *tick);
        std::process::exit(0);
    }
}

fn main() {
    // AssetServer::load（由 register_config 的 Startup 系统触发）需要 IoTaskPool。
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(0).build());

    let mut app = App::new();
    app.add_plugins((bevy::asset::AssetPlugin::default(), bevy::time::TimePlugin));

    app.add_plugins(ConfigHmrPlugin {
        debounce_window: Duration::from_millis(0),
    });
    app.setup_hmr_headless();
    // 一行注册一种类型，调用两次注册两种类型
    app.register_config::<NpcDatabase>("data/npc.ron")
        .register_config::<ItemDatabase>("data/item.ron");

    let npc_id = AssetId::Uuid {
        uuid: Uuid::from_u128(101),
    };
    let item_id = AssetId::Uuid {
        uuid: Uuid::from_u128(202),
    };
    let initial_npc = NpcDatabase {
        npcs: vec![NpcEntry {
            id: "npc_1".into(),
            hp: 100,
        }],
    };
    let initial_item = ItemDatabase {
        items: vec![ItemEntry {
            id: "item_1".into(),
            price: 100,
        }],
    };
    app.insert_config::<NpcDatabase>(npc_id, initial_npc, "data/npc.ron")
        .insert_config::<ItemDatabase>(item_id, initial_item, "data/item.ron");
    app.insert_resource(SeedIds {
        npc: npc_id,
        item: item_id,
    });

    app.add_systems(Update, (npc_subscriber, item_subscriber, simulate));

    println!("=== bevy_assets_hmr multi_type 示例启动 ===");
    println!("已注册 NpcDatabase + ItemDatabase 两种类型");
    println!("第 2 帧只改 NpcDatabase，第 4 帧只改 ItemDatabase");
    println!("观察每个订阅方是否只收到对应类型的事件");

    loop {
        app.update();
    }
}
