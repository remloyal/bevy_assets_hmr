//! End-to-end tests for the minimal config watcher.

use bevy::prelude::*;
use bevy_assets_hmr::{ConfigChanges, ConfigWatchAppExt, ConfigWatchPlugin};
use serde::Deserialize;
use std::path::PathBuf;
use std::time::Duration;

#[derive(Resource, Deserialize, Debug, PartialEq, Default)]
struct Settings {
    title: String,
    max_players: u32,
}

const V1: &str = r#"(title: "Hello", max_players: 4)"#;
const V2: &str = r#"(title: "Hello HMR", max_players: 8)"#;
const BROKEN: &str = r#"(title: "oops", max_players: not_a_number)"#;

fn make_dir(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "bevy_assets_hmr_test_{}_{}",
        std::process::id(),
        name
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn app_for(dir: &PathBuf) -> App {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins)
        .add_plugins(ConfigWatchPlugin {
            root: dir.clone(),
            poll_interval: Duration::ZERO,
        });
    app
}

/// Counts how many times the `Settings` resource was observed as changed.
#[derive(Resource, Default)]
struct ChangeCount(u32);

fn count_changes(settings: Res<Settings>, mut count: ResMut<ChangeCount>) {
    if settings.is_changed() {
        count.0 += 1;
    }
}

fn add_change_counter(app: &mut App) {
    app.init_resource::<ChangeCount>();
    app.add_systems(Update, count_changes);
}

// ---------------------------------------------------------------------------
// RON (extension-detected)
// ---------------------------------------------------------------------------

#[test]
fn loads_file_on_first_update() {
    let dir = make_dir("loads");
    std::fs::write(dir.join("settings.ron"), V1).unwrap();

    let mut app = app_for(&dir);
    app.watch_config::<Settings>("settings.ron");
    app.update();

    let settings = app.world().resource::<Settings>();
    assert_eq!(settings.title, "Hello");
    assert_eq!(settings.max_players, 4);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reloads_when_file_changes() {
    let dir = make_dir("reloads");
    std::fs::write(dir.join("settings.ron"), V1).unwrap();

    let mut app = app_for(&dir);
    app.watch_config::<Settings>("settings.ron");
    app.update();

    std::fs::write(dir.join("settings.ron"), V2).unwrap();
    app.update();

    let settings = app.world().resource::<Settings>();
    assert_eq!(settings.title, "Hello HMR");
    assert_eq!(settings.max_players, 8);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn parse_error_keeps_last_valid_value() {
    let dir = make_dir("parse_error");
    std::fs::write(dir.join("settings.ron"), V1).unwrap();

    let mut app = app_for(&dir);
    app.watch_config::<Settings>("settings.ron");
    app.update();

    std::fs::write(dir.join("settings.ron"), BROKEN).unwrap();
    app.update();
    assert_eq!(app.world().resource::<Settings>().title, "Hello");

    std::fs::write(dir.join("settings.ron"), V2).unwrap();
    app.update();
    assert_eq!(app.world().resource::<Settings>().title, "Hello HMR");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn missing_file_leaves_default_without_panicking() {
    let dir = make_dir("missing");

    let mut app = app_for(&dir);
    app.watch_config::<Settings>("settings.ron");
    app.update();

    assert_eq!(*app.world().resource::<Settings>(), Settings::default());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn file_appearing_later_loads() {
    let dir = make_dir("recovery");

    let mut app = app_for(&dir);
    app.watch_config::<Settings>("settings.ron");
    app.update();

    std::fs::write(dir.join("settings.ron"), V1).unwrap();
    app.update();

    assert_eq!(app.world().resource::<Settings>().title, "Hello");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn second_watch_for_same_type_is_ignored() {
    let dir = make_dir("single_file_per_type");
    std::fs::write(dir.join("a.ron"), V1).unwrap();
    std::fs::write(dir.join("b.ron"), V2).unwrap();

    let mut app = app_for(&dir);
    app.watch_config::<Settings>("a.ron");
    app.watch_config::<Settings>("b.ron");
    app.update();

    assert_eq!(app.world().resource::<Settings>().title, "Hello");

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn unchanged_poll_does_not_mark_resource_changed() {
    let dir = make_dir("no_spurious_change");
    std::fs::write(dir.join("settings.ron"), V1).unwrap();

    let mut app = app_for(&dir);
    app.watch_config::<Settings>("settings.ron");
    add_change_counter(&mut app);

    app.update(); // initial load -> one change
    app.update(); // nothing changed on disk -> no change
    app.update();

    assert_eq!(app.world().resource::<ChangeCount>().0, 1);

    std::fs::write(dir.join("settings.ron"), V2).unwrap();
    app.update();
    assert_eq!(app.world().resource::<ChangeCount>().0, 2);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn whitespace_only_change_does_not_fire() {
    let dir = make_dir("whitespace_only");
    std::fs::write(dir.join("settings.ron"), r#"(title:"Hello",max_players:4)"#).unwrap();

    let mut app = app_for(&dir);
    app.watch_config::<Settings>("settings.ron");
    add_change_counter(&mut app);

    app.update(); // initial load -> 1
    assert_eq!(app.world().resource::<ChangeCount>().0, 1);

    // Same values, only formatting/comments differ.
    std::fs::write(
        dir.join("settings.ron"),
        "(title: \"Hello\", max_players: 4) // comment\n",
    )
    .unwrap();
    app.update();
    assert_eq!(
        app.world().resource::<ChangeCount>().0,
        1,
        "whitespace/comment-only edit must not fire is_changed"
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn ignored_field_change_does_not_fire() {
    let dir = make_dir("ignored_field");
    // `Settings` has no `extra` field, so serde ignores it.
    std::fs::write(
        dir.join("settings.ron"),
        r#"(title: "Hello", max_players: 4, extra: 1)"#,
    )
    .unwrap();

    let mut app = app_for(&dir);
    app.watch_config::<Settings>("settings.ron");
    add_change_counter(&mut app);

    app.update(); // initial load -> 1
    assert_eq!(app.world().resource::<ChangeCount>().0, 1);

    // Only the ignored field changes.
    std::fs::write(
        dir.join("settings.ron"),
        r#"(title: "Hello", max_players: 4, extra: 999)"#,
    )
    .unwrap();
    app.update();
    assert_eq!(
        app.world().resource::<ChangeCount>().0,
        1,
        "changing a field `Settings` ignores must not fire is_changed"
    );

    // A field `Settings` does have still fires.
    std::fs::write(
        dir.join("settings.ron"),
        r#"(title: "Hello", max_players: 8, extra: 999)"#,
    )
    .unwrap();
    app.update();
    assert_eq!(app.world().resource::<ChangeCount>().0, 2);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// JSON (extension-detected)
// ---------------------------------------------------------------------------

#[test]
fn json_extension_uses_json_parser() {
    let dir = make_dir("json");
    std::fs::write(
        dir.join("settings.json"),
        r#"{"title": "Hi", "max_players": 2}"#,
    )
    .unwrap();

    let mut app = app_for(&dir);
    app.watch_config::<Settings>("settings.json");
    app.update();
    assert_eq!(app.world().resource::<Settings>().title, "Hi");

    std::fs::write(
        dir.join("settings.json"),
        r#"{"title": "Hi JSON", "max_players": 16}"#,
    )
    .unwrap();
    app.update();
    let settings = app.world().resource::<Settings>();
    assert_eq!(settings.title, "Hi JSON");
    assert_eq!(settings.max_players, 16);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Custom parser
// ---------------------------------------------------------------------------

#[derive(Resource, Debug, PartialEq, Default)]
struct KeyValues {
    title: String,
    volume: u32,
}

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
            "volume" => values.volume = value.trim().parse().map_err(|e| format!("{e}"))?,
            other => return Err(format!("未知键: {other}")),
        }
    }
    Ok(values)
}

/// A parser over raw bytes, for a format that is *not* valid UTF-8.
#[derive(Resource, Debug, PartialEq, Default)]
struct RawConfig {
    value: u8,
}

fn parse_raw(bytes: &[u8]) -> Result<RawConfig, String> {
    match bytes {
        [0xFF, value, ..] => Ok(RawConfig { value: *value }),
        _ => Err(format!("bad raw config: {bytes:?}")),
    }
}

#[test]
fn custom_parser_supports_binary_bytes() {
    let dir = make_dir("binary");
    // 0xFF is not valid UTF-8: the old `read_to_string` path could not read it.
    std::fs::write(dir.join("blob.bin"), [0xFFu8, 7]).unwrap();

    let mut app = app_for(&dir);
    app.watch_config_with::<RawConfig, _>("blob.bin", parse_raw);
    app.update();
    assert_eq!(app.world().resource::<RawConfig>().value, 7);

    std::fs::write(dir.join("blob.bin"), [0xFFu8, 9]).unwrap();
    app.update();
    assert_eq!(app.world().resource::<RawConfig>().value, 9);

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn custom_parser_loads_and_reloads() {
    let dir = make_dir("custom");
    std::fs::write(dir.join("settings.ini"), "title = Game\nvolume = 3\n").unwrap();

    let mut app = app_for(&dir);
    app.watch_config_with::<KeyValues, _>("settings.ini", parse_ini);
    app.update();
    let values = app.world().resource::<KeyValues>();
    assert_eq!(values.title, "Game");
    assert_eq!(values.volume, 3);

    std::fs::write(dir.join("settings.ini"), "title = Game\nvolume = 9\n").unwrap();
    app.update();
    assert_eq!(app.world().resource::<KeyValues>().volume, 9);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Changed-field reporting (built-in formats)
// ---------------------------------------------------------------------------

#[test]
fn reports_changed_field_ron() {
    let dir = make_dir("changed_field_ron");
    std::fs::write(dir.join("settings.ron"), V1).unwrap();

    let mut app = app_for(&dir);
    app.watch_config::<Settings>("settings.ron");
    app.update();
    // The first load has no prior version to diff against.
    assert!(app.world().resource::<ConfigChanges<Settings>>().is_empty());

    std::fs::write(dir.join("settings.ron"), r#"(title: "Hello", max_players: 8)"#).unwrap();
    app.update();

    let changes = app.world().resource::<ConfigChanges<Settings>>();
    assert_eq!(changes.changed, vec!["max_players".to_string()]);
    assert_eq!(changes.revision, 1);
    assert!(changes.contains("max_players"));
    assert!(!changes.contains("title"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn reports_changed_field_json() {
    let dir = make_dir("changed_field_json");
    std::fs::write(dir.join("settings.json"), r#"{"title":"A","max_players":1}"#).unwrap();

    let mut app = app_for(&dir);
    app.watch_config::<Settings>("settings.json");
    app.update();

    std::fs::write(dir.join("settings.json"), r#"{"title":"B","max_players":1}"#).unwrap();
    app.update();

    let changes = app.world().resource::<ConfigChanges<Settings>>();
    assert_eq!(changes.changed, vec!["title".to_string()]);

    let _ = std::fs::remove_dir_all(&dir);
}

#[derive(Resource, Deserialize, Debug, PartialEq, Default)]
struct Nested {
    title: String,
    graphics: Graphics,
}

#[derive(Deserialize, Debug, PartialEq, Default)]
struct Graphics {
    resolution: u32,
    vsync: bool,
}

#[test]
fn reports_nested_changed_field_path() {
    let dir = make_dir("changed_nested");
    std::fs::write(
        dir.join("nested.ron"),
        r#"(title: "x", graphics: (resolution: 1080, vsync: true))"#,
    )
    .unwrap();

    let mut app = app_for(&dir);
    app.watch_config::<Nested>("nested.ron");
    app.update();

    std::fs::write(
        dir.join("nested.ron"),
        r#"(title: "x", graphics: (resolution: 720, vsync: true))"#,
    )
    .unwrap();
    app.update();

    let changes = app.world().resource::<ConfigChanges<Nested>>();
    assert_eq!(changes.changed, vec!["graphics.resolution".to_string()]);

    let _ = std::fs::remove_dir_all(&dir);
}

// ---------------------------------------------------------------------------
// Unsupported extension
// ---------------------------------------------------------------------------

#[test]
#[should_panic(expected = "不支持的配置扩展名")]
fn unknown_extension_panics() {
    let dir = make_dir("unknown_ext");
    let mut app = app_for(&dir);
    app.watch_config::<Settings>("settings.yaml");
}
