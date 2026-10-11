//! 最简示例：监听一个 ron 文件，在任何地方读 `Res<你的类型>` 就能拿到热更新后的值。
//!
//! 运行：`cargo run --example simplest`
//!
//! 注意：整个 crate 只暴露 `watch_config` 一个入口，没有包装类型、没有事件、
//! 没有 diff、没有钩子。`NpcDatabase` 就是普通 Resource。

use bevy::prelude::*;
use bevy_assets_hmr::{ConfigChanges, ConfigWatchAppExt};
use serde::Deserialize;
use std::time::Duration;

#[derive(Resource, Deserialize, Debug, PartialEq, Default)]
struct NpcDatabase {
    npcs: Vec<NpcEntry>,
}

#[derive(Deserialize, Debug, PartialEq)]
struct NpcEntry {
    id: String,
    name: String,
    hp: u32,
}

const V1: &str = r#"(
    npcs: [
        (id: "npc_1", name: "商人", hp: 100),
        (id: "npc_2", name: "守卫", hp: 150),
    ],
)"#;

const V2: &str = r#"(
    npcs: [
        (id: "npc_1", name: "商人", hp: 100),
        (id: "npc_2", name: "守卫", hp: 999),
        (id: "npc_3", name: "法师", hp: 80),
    ],
)"#;

const BROKEN: &str = r#"(npcs: [(id: "x", name: "y", hp: oops)])"#;

/// 消费侧：读自己的类型；`ConfigChanges` 告诉你哪些字段变了。
fn report(db: Res<NpcDatabase>, changes: Res<ConfigChanges<NpcDatabase>>) {
    if !db.is_changed() {
        return;
    }
    if !changes.is_empty() {
        println!("[业务] 变更字段: {}", changes.changed.join(", "));
    }
    println!("[业务] 当前 {} 个 NPC:", db.npcs.len());
    for npc in &db.npcs {
        println!("      {} ({}) hp={}", npc.id, npc.name, npc.hp);
    }
}

fn main() {
    let dir = std::env::temp_dir().join(format!("bevy_assets_hmr_simplest_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("npc.ron");
    std::fs::write(&path, V1).unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::log::LogPlugin::default())
        .add_plugins(bevy_assets_hmr::ConfigWatchPlugin {
            root: dir.clone(),
            poll_interval: Duration::ZERO,
        })
        // 格式由扩展名决定（.ron），root 默认是 assets/，这里被插件覆盖为临时目录
        .watch_config::<NpcDatabase>("npc.ron")
        .add_systems(Update, report);

    println!("=== 启动（文件含 2 个 NPC）===");
    app.update();

    println!("\n=== 模拟修改文件（npc_2 hp -> 999，新增 npc_3）===");
    std::fs::write(&path, V2).unwrap();
    app.update();

    println!("\n=== 模拟写入损坏的 ron（上一份好值保留，错误写日志）===");
    std::fs::write(&path, BROKEN).unwrap();
    app.update();

    println!("\n=== 修复文件（恢复为 V1）===");
    std::fs::write(&path, V1).unwrap();
    app.update();

    let _ = std::fs::remove_dir_all(&dir);
    println!("\n完成。");
}
