//! `AssetBind<Image>` 作为组件直接写在 `bsn!` 宏内的示例。
//!
//! # 背景
//!
//! Bevy 0.19 的 `FromTemplate` / `Template` blanket impl 要求 `T: Clone + Default + Unpin`，
//! 而 `Handle<A>` 通过 `SpecializeFromTemplate` auto-trait trick 被刻意排除在 `Unpin` 之外，
//! 因此含 `Handle<A>` 的 `AssetBind<A>` 无法走 blanket，必须手写一个 `AssetBindTemplate<A>`
//! 作为 `<AssetBind<A> as FromTemplate>::Template`（见 `src/watcher.rs`）。
//!
//! 本例验证两种写法：
//! 1. `{ asset_server.load(path) }` 内联表达式作为字段值。
//! 2. 外部 `Handle<Image>` 变量作为字段值（注意：裸 ident 在 `bsn!` 中会被当成函数名，
//!    必须用 `{ }` 包裹）。
//!
//! 运行：`cargo run --example bsn_asset_bind -p bevy_assets_hmr`
//! 前提：需启用 `bevy/file_watcher` 才能看到热更通知（本例仅展示 spawn，未订阅事件）。

use bevy::prelude::*;
use bevy_assets_hmr::{AssetBind, ConfigHmrAppExt, ConfigHmrPlugin};

const BG_PATH: &str = "textures/bg.jpg";

fn main() {
    let mut app = App::new();
    app.add_plugins(MinimalPlugins);
    app.add_plugins(bevy::asset::AssetPlugin::default());
    app.add_plugins(bevy::image::ImagePlugin::default());
    app.add_plugins(bevy::ui::UiPlugin);
    app.add_plugins(ConfigHmrPlugin::default());
    app.watch_asset::<Image>();

    app.add_systems(Startup, setup);

    app.run();
}

fn setup(mut commands: Commands, asset_server: Res<AssetServer>) {
    // 复刻用户 main_menu.rs 里 bg 实体那一层的最小结构：
    // ImageNode + AssetBind<Image> + Node 放在同一括号组。
    commands.spawn_scene(bsn! {
        Node { width: percent(100), height: percent(100), position_type: PositionType::Absolute, overflow: Overflow::clip() }
        BackgroundColor(Color::BLACK)
        Children [
            (
                ImageNode { image: {asset_server.load(BG_PATH)} }
                AssetBind<Image> { handle: {asset_server.load(BG_PATH)} }
                Node { width: percent(100), height: percent(100), position_type: PositionType::Absolute }
            ),
        ]
    });

    // 用外部变量作为字段值的写法（裸 ident 必须用 { } 包裹，否则 bsn! 会把
    // 小写首字母的 ident 当成函数名并期待调用语法）
    let handle: Handle<Image> = asset_server.load(BG_PATH);
    commands.spawn_scene(bsn! {
        Node { width: percent(100), height: percent(100) }
        Children [
            (
                ImageNode { image: {handle.clone()} }
                AssetBind<Image> { handle: {handle.clone()} }
            ),
        ]
    });

    // Camera
    commands.spawn(Camera2d);
}
