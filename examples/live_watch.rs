//! 手动热更新演示：运行后**手动编辑** `assets/data/npc.ron` 并保存，
//! 终端里会立刻打印热更新后的内容。
//!
//! 运行：`cargo run --example live_watch`（Ctrl+C 退出）
//!
//! 监听的是仓库里真实的 ron 文件，不经过 AssetServer，也不写临时文件。

use bevy::app::ScheduleRunnerPlugin;
use bevy::prelude::*;
use bevy_assets_hmr::ConfigWatchAppExt;
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

/// 消费侧：只是读自己的 Resource。
fn report(db: Res<NpcDatabase>) {
    if !db.is_changed() {
        return;
    }
    println!("[HMR] npc.ron 已热更新，当前 {} 个 NPC:", db.npcs.len());
    for npc in &db.npcs {
        println!("      {} ({}) hp={}", npc.id, npc.name, npc.hp);
    }
}

fn main() {
    println!("=== 监听 assets/data/npc.ron ===");
    println!("请编辑该文件并保存（例如把某个 hp 改掉），即可看到热更新。Ctrl+C 退出。\n");

    App::new()
        .add_plugins(MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(50))))
        .add_plugins(bevy::log::LogPlugin::default())
        // root 默认是 `assets/`，所以这里写相对路径 data/npc.ron
        .watch_config::<NpcDatabase>("data/npc.ron")
        .add_systems(Update, report)
        .run();
}
