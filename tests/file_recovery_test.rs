use bevy::asset::{Asset, AssetPlugin, Assets};
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{
    ConfigAsset, ConfigDiff, ConfigHandle, ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh,
    ConfigReloadFailed, RefreshCause,
};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use uuid::Uuid;

#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default, ConfigDiff)]
#[config_diff(field = "items", id = "id")]
struct RecoveryDb {
    items: Vec<RecoveryItem>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
struct RecoveryItem {
    id: String,
    n: u32,
}

#[derive(Resource, Default)]
struct Captured {
    failures: Vec<ConfigReloadFailed<RecoveryDb>>,
    refreshes: Vec<ConfigRefresh<RecoveryDb>>,
}

fn capture(
    mut failures: MessageReader<ConfigReloadFailed<RecoveryDb>>,
    mut refreshes: MessageReader<ConfigRefresh<RecoveryDb>>,
    mut captured: ResMut<Captured>,
) {
    captured.failures.extend(failures.read().cloned());
    captured.refreshes.extend(refreshes.read().cloned());
}

struct TempAssetDir(PathBuf);

impl Drop for TempAssetDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn update_until(app: &mut App, timeout: Duration, ready: impl Fn(&World) -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        app.update();
        if ready(app.world()) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn real_file_fix_after_parse_failure_emits_recovery_refresh() {
    bevy::tasks::IoTaskPool::get_or_init(|| {
        bevy::tasks::TaskPoolBuilder::new().num_threads(1).build()
    });

    let root = std::env::temp_dir().join(format!("bevy_assets_hmr_{}", Uuid::new_v4()));
    fs::create_dir_all(root.join("data")).expect("create temporary asset directory");
    let asset_path = root.join("data/recovery.ron");
    let valid = "(items: [(id: \"same\", n: 1)])\n";
    fs::write(&asset_path, valid).expect("write initial valid asset");
    let _temp_dir = TempAssetDir(root.clone());

    let mut app = App::new();
    app.add_plugins((
        bevy::app::TaskPoolPlugin::default(),
        AssetPlugin {
            file_path: root.to_string_lossy().into_owned(),
            watch_for_changes_override: Some(true),
            use_asset_processor_override: Some(false),
            ..Default::default()
        },
        bevy::time::TimePlugin,
    ));
    app.add_plugins(ConfigHmrPlugin {
        debounce_window: Duration::from_millis(0),
    });
    app.setup_hmr_headless();
    app.register_config::<RecoveryDb>("data/recovery.ron");
    app.insert_resource(Captured::default());
    app.add_systems(Update, capture);

    assert!(update_until(&mut app, Duration::from_secs(5), |world| {
        world
            .get_resource::<ConfigHandle<ConfigAsset<RecoveryDb>>>()
            .is_some_and(|holder| !holder.handles.is_empty())
    }));
    let asset_id = app
        .world()
        .resource::<ConfigHandle<ConfigAsset<RecoveryDb>>>()
        .handles[0]
        .id();
    assert!(update_until(&mut app, Duration::from_secs(5), |world| {
        world
            .resource::<bevy_assets_hmr::LastSnapshot<ConfigAsset<RecoveryDb>>>()
            .map
            .contains_key(&asset_id)
    }));

    fs::write(&asset_path, "(items: [(id: \"same\", n: not_a_number)])\n")
        .expect("write invalid asset");
    assert!(update_until(&mut app, Duration::from_secs(5), |world| {
        world
            .resource::<Captured>()
            .failures
            .iter()
            .any(|event| event.asset_id == asset_id.untyped())
    }));
    assert_eq!(
        app.world()
            .resource::<Assets<ConfigAsset<RecoveryDb>>>()
            .get(asset_id)
            .expect("Bevy should retain the last valid asset")
            .raw
            .items[0]
            .n,
        1
    );

    fs::write(&asset_path, valid).expect("repair asset");
    assert!(update_until(&mut app, Duration::from_secs(5), |world| {
        world.resource::<Captured>().refreshes.iter().any(|event| {
            event.asset_id == asset_id.untyped() && event.cause == RefreshCause::Recovery
        })
    }));
}
