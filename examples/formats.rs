//! 多格式演示：三种类型、三种解析方式共存于同一个 App。
//!
//! - `.ron`  → `watch_config`（按扩展名自动选 ron）
//! - `.json` → `watch_config`（按扩展名自动选 json）
//! - `.ini`  → `watch_config_with`（自定义解析函数）
//!
//! 运行：`cargo run --example formats`

use bevy::prelude::*;
use bevy_assets_hmr::ConfigWatchAppExt;
use serde::Deserialize;
use std::time::Duration;

#[derive(Resource, Deserialize, Debug, PartialEq, Default)]
struct Gameplay {
    difficulty: String,
}

#[derive(Resource, Deserialize, Debug, PartialEq, Default)]
struct ServerConfig {
    host: String,
    port: u16,
}

/// 非内置格式：自己提供解析函数。
#[derive(Resource, Debug, PartialEq, Default)]
struct KeyValues {
    title: String,
    volume: u32,
}

/// 极简 `key = value` 解析器，演示 `watch_config_with`（拿到的是原始字节）。
fn parse_ini(bytes: &[u8]) -> Result<KeyValues, String> {
    let text = std::str::from_utf8(bytes).map_err(|error| error.to_string())?;
    let mut values = KeyValues::default();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let (key, value) = line
            .split_once('=')
            .ok_or_else(|| format!("缺少 `=`: {line}"))?;
        match key.trim() {
            "title" => values.title = value.trim().to_string(),
            "volume" => {
                values.volume = value
                    .trim()
                    .parse()
                    .map_err(|error| format!("volume 解析失败: {error}"))?
            }
            other => return Err(format!("未知键: {other}")),
        }
    }
    Ok(values)
}

fn report_all(ron: Res<Gameplay>, json: Res<ServerConfig>, ini: Res<KeyValues>) {
    if ron.is_changed() {
        println!("[.ron ] difficulty = {}", ron.difficulty);
    }
    if json.is_changed() {
        println!("[.json] {}:{}", json.host, json.port);
    }
    if ini.is_changed() {
        println!("[.ini ] title = {}, volume = {}", ini.title, ini.volume);
    }
}

fn main() {
    let dir = std::env::temp_dir().join(format!("bevy_assets_hmr_formats_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();

    std::fs::write(dir.join("gameplay.ron"), r#"(difficulty: "easy")"#).unwrap();
    std::fs::write(dir.join("server.json"), r#"{"host": "127.0.0.1", "port": 8080}"#).unwrap();
    std::fs::write(dir.join("settings.ini"), "title = 我的游戏\nvolume = 5\n").unwrap();

    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(bevy::log::LogPlugin::default())
        .add_plugins(bevy_assets_hmr::ConfigWatchPlugin {
            root: dir.clone(),
            poll_interval: Duration::ZERO,
        })
        .watch_config::<Gameplay>("gameplay.ron")
        .watch_config::<ServerConfig>("server.json")
        .watch_config_with::<KeyValues, _>("settings.ini", parse_ini)
        .add_systems(Update, report_all);

    println!("=== 三种格式首次加载 ===");
    app.update();

    println!("\n=== 同时修改三个文件 ===");
    std::fs::write(dir.join("gameplay.ron"), r#"(difficulty: "nightmare")"#).unwrap();
    std::fs::write(dir.join("server.json"), r#"{"host": "10.0.0.2", "port": 9000}"#).unwrap();
    std::fs::write(dir.join("settings.ini"), "title = 我的游戏\nvolume = 11\n").unwrap();
    app.update();

    let _ = std::fs::remove_dir_all(&dir);
    println!("\n完成。");
}
