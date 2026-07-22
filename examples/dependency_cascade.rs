//! Dependency cascade example.
//!
//! 运行：`cargo run --example dependency_cascade`
//!
//! Demonstrates:
//! - `ParentDb` holds a `Handle<ChildDb>` field marked with `#[dependency]`
//! - `derive(Asset)` auto-generates `VisitAssetDependencies` that visits
//!   `#[dependency]` fields
//! - When the child asset is modified (frame 5), the parent subscriber
//!   receives a `ConfigRefresh<ParentDb>` with `RefreshCause::Dependency`
//! - When the child asset is removed (frame 9), the parent subscriber
//!   also receives a cascade `ConfigRefresh` so it can clean up derived
//!   state
//!
//! Watch the stdout for the `on_parent_refresh` system to distinguish
//! between dependency cascade and direct self-modification events.

use bevy::asset::{Asset, AssetId, Assets, Handle};
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{
    ConfigDiff, ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh, HmrSource, RefreshCause,
};
use std::time::Duration;
use uuid::Uuid;

/// Child config: a simple key->value table.
#[derive(Asset, TypePath, Clone, Debug, PartialEq, Default, ConfigDiff)]
#[config_diff(field = "items", id = "id")]
struct ChildDb {
    items: Vec<ChildItem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ChildItem {
    id: String,
    v: u32,
}

impl HmrSource for ChildDb {
    type Config = ChildDb;
    fn config(&self) -> &ChildDb {
        self
    }
    fn source_path(&self) -> &str {
        ""
    }
}

/// Parent config that holds a `Handle<ChildDb>`.
///
/// The `#[dependency]` attribute on `child_handle` is what makes
/// `#[derive(Asset)]` emit a `VisitAssetDependencies` impl that visits
/// this handle. Without it, `dependency_registry_system` would never
/// observe the parent -> child edge.
#[derive(Asset, TypePath, Clone, Debug, PartialEq, Default, ConfigDiff)]
#[config_diff(field = "items", id = "id")]
struct ParentDb {
    items: Vec<ParentItem>,
    /// Marks this `Handle<ChildDb>` field as a tracked dependency.
    /// Modifying or removing `ChildDb` will cascade a
    /// dependency-caused `ConfigRefresh<ParentDb>` to this parent.
    #[dependency]
    child_handle: Handle<ChildDb>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct ParentItem {
    id: String,
}

impl HmrSource for ParentDb {
    type Config = ParentDb;
    fn config(&self) -> &ParentDb {
        self
    }
    fn source_path(&self) -> &str {
        ""
    }
}

/// Subscriber: prints whether the refresh is a dependency cascade or a
/// direct change.
fn on_parent_refresh(mut reader: MessageReader<ConfigRefresh<ParentDb>>) {
    for refresh in reader.read() {
        let kind = match &refresh.cause {
            RefreshCause::Dependency { .. } => "CASCADE (child modified or removed)",
            _ => "SELF-CHANGE",
        };
        println!(
            "\n[ConfigRefresh<ParentDb>] {kind}\n  source_path: {:?}\n  changed_ids: {:?}\n  new_config: {:?}",
            refresh.source_path, refresh.changed_ids, refresh.new_config,
        );
    }
}

/// Simulation: at frame 5 modify the child; at frame 9 remove the
/// child; at frame 13 exit.
fn simulate(
    mut tick: Local<u32>,
    mut child_assets: ResMut<Assets<ChildDb>>,
    parent_assets: Res<Assets<ParentDb>>,
    ids: Res<SimIds>,
) {
    *tick += 1;

    if *tick == 5 {
        println!("\n[第 {} 帧] 修改 ChildDb -> 触发 ParentDb 级联", *tick);
        let _ = child_assets.insert(
            ids.child_id,
            ChildDb {
                items: vec![ChildItem {
                    id: "c1".into(),
                    v: 999,
                }],
            },
        );
    }

    if *tick == 9 {
        println!(
            "\n[第 {} 帧] 删除 ChildDb -> 再次触发 ParentDb 级联（让父订阅方清理派生数据）",
            *tick
        );
        child_assets.remove(ids.child_id);
    }

    if *tick == 13 {
        println!("\n[第 {} 帧] 示例完成，退出", *tick);
        std::process::exit(0);
    }

    // Touch parent_assets to silence unused warning when no edits fire.
    let _ = parent_assets.get(ids.parent_id);
}

#[derive(Resource)]
struct SimIds {
    child_id: AssetId<ChildDb>,
    parent_id: AssetId<ParentDb>,
}

fn main() {
    use bevy::tasks::{IoTaskPool, TaskPoolBuilder};
    IoTaskPool::get_or_init(|| TaskPoolBuilder::new().num_threads(0).build());

    let mut app = App::new();
    app.add_plugins((bevy::asset::AssetPlugin::default(), bevy::time::TimePlugin));

    // 1. HMR plugin (debounce window 0 for instant dispatch in example).
    app.add_plugins(ConfigHmrPlugin {
        debounce_window: Duration::from_millis(0),
    });
    app.setup_hmr_headless();

    // 2. Init assets (直接模式 + 包装模式都不需要 init_asset 已注册的，
    //    但直接模式要求用户手动 init_asset<A>())。
    app.init_asset::<ChildDb>();
    app.init_asset::<ParentDb>();

    // 3. Register both types via register_asset (直接模式). Both end up
    //    in the dependency pipeline.
    app.register_asset::<ChildDb>("data/child.ron");
    app.register_asset::<ParentDb>("data/parent.ron");

    // 4. Insert initial child + parent with the dependency edge.
    let child_handle = app
        .world_mut()
        .resource_mut::<Assets<ChildDb>>()
        .add(ChildDb {
            items: vec![ChildItem {
                id: "c1".into(),
                v: 1,
            }],
        });

    let parent_id = AssetId::<ParentDb>::Uuid {
        uuid: Uuid::from_u128(2002),
    };
    app.world_mut()
        .resource_mut::<Assets<ParentDb>>()
        .insert(
            parent_id,
            ParentDb {
                items: vec![ParentItem { id: "p1".into() }],
                child_handle: child_handle.clone(),
            },
        )
        .ok();

    // Eagerly initialize snapshots so first frame doesn't dispatch.
    {
        let mut snapshots = app
            .world_mut()
            .resource_mut::<bevy_assets_hmr::LastSnapshot<ChildDb>>();
        snapshots.map.insert(
            child_handle.id(),
            ChildDb {
                items: vec![ChildItem {
                    id: "c1".into(),
                    v: 1,
                }],
            },
        );
        let mut snapshots = app
            .world_mut()
            .resource_mut::<bevy_assets_hmr::LastSnapshot<ParentDb>>();
        snapshots.map.insert(
            parent_id,
            ParentDb {
                items: vec![ParentItem { id: "p1".into() }],
                child_handle: child_handle.clone(),
            },
        );
    }

    app.insert_resource(SimIds {
        child_id: child_handle.id(),
        parent_id,
    });

    // 5. Subscriber + simulator.
    app.add_systems(Update, (on_parent_refresh, simulate));

    println!("=== bevy_assets_hmr dependency_cascade 示例启动 ===");
    println!("ChildDb -> ParentDb 级联关系通过 ParentDb.child_handle 上的 #[dependency] 属性建立");
    println!("第 5 帧: 修改 ChildDb -> ParentDb 订阅方收到 CASCADE 事件");
    println!("第 9 帧: 删除 ChildDb -> ParentDb 订阅方再次收到 CASCADE 事件");
    println!();
    println!("\n[第 1 帧] 等待级联...");

    loop {
        app.update();
    }
}
