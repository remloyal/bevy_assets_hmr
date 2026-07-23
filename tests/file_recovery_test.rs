use bevy::asset::{Asset, AssetId, AssetPlugin, Assets};
use bevy::ecs::message::MessageReader;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{
    ConfigAsset, ConfigDiff, ConfigHandle, ConfigHmrAppExt, ConfigHmrPlugin, ConfigRefresh,
    ConfigReloadFailed, ConfigRemoved, RefreshCause,
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
    removed: Vec<ConfigRemoved<RecoveryDb>>,
}

fn capture(
    mut failures: MessageReader<ConfigReloadFailed<RecoveryDb>>,
    mut refreshes: MessageReader<ConfigRefresh<RecoveryDb>>,
    mut removed: MessageReader<ConfigRemoved<RecoveryDb>>,
    mut captured: ResMut<Captured>,
) {
    captured.failures.extend(failures.read().cloned());
    captured.refreshes.extend(refreshes.read().cloned());
    captured.removed.extend(removed.read().cloned());
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

struct FileWatcherFixture {
    app: App,
    asset_path: PathBuf,
    asset_id: AssetId<ConfigAsset<RecoveryDb>>,
    _temp_dir: TempAssetDir,
}

fn create_fixture(initial: &str, debounce_window: Duration) -> FileWatcherFixture {
    bevy::tasks::IoTaskPool::get_or_init(|| {
        bevy::tasks::TaskPoolBuilder::new().num_threads(1).build()
    });

    let root = std::env::temp_dir().join(format!("bevy_assets_hmr_{}", Uuid::new_v4()));
    fs::create_dir_all(root.join("data")).expect("create temporary asset directory");
    let asset_path = root.join("data/recovery.ron");
    fs::write(&asset_path, initial).expect("write initial valid asset");
    let temp_dir = TempAssetDir(root.clone());

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
    app.add_plugins(ConfigHmrPlugin { debounce_window });
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

    FileWatcherFixture {
        app,
        asset_path,
        asset_id,
        _temp_dir: temp_dir,
    }
}

#[test]
fn real_file_fix_after_parse_failure_emits_recovery_refresh() {
    let valid = "(items: [(id: \"same\", n: 1)])\n";
    let mut fixture = create_fixture(valid, Duration::from_millis(0));
    let asset_id = fixture.asset_id;

    fs::write(
        &fixture.asset_path,
        "(items: [(id: \"same\", n: not_a_number)])\n",
    )
    .expect("write invalid asset");
    assert!(update_until(
        &mut fixture.app,
        Duration::from_secs(5),
        |world| {
            world
                .resource::<Captured>()
                .failures
                .iter()
                .any(|event| event.asset_id == asset_id.untyped())
        }
    ));
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<Assets<ConfigAsset<RecoveryDb>>>()
            .get(asset_id)
            .expect("Bevy should retain the last valid asset")
            .raw
            .items[0]
            .n,
        1
    );

    fs::write(&fixture.asset_path, valid).expect("repair asset");
    assert!(update_until(
        &mut fixture.app,
        Duration::from_secs(5),
        |world| {
            world.resource::<Captured>().refreshes.iter().any(|event| {
                event.asset_id == asset_id.untyped() && event.cause == RefreshCause::Recovery
            })
        }
    ));
}

#[test]
fn temporary_file_rename_replacement_applies_final_asset_without_removal() {
    let mut fixture = create_fixture(
        "(items: [(id: \"same\", n: 1)])\n",
        Duration::from_millis(150),
    );
    let asset_id = fixture.asset_id;
    let temporary_path = fixture.asset_path.with_extension("ron.tmp");
    let backup_path = fixture.asset_path.with_extension("ron.backup");
    fs::write(&temporary_path, "(items: [(id: \"same\", n: 2)])\n")
        .expect("write editor temporary file");

    // This models editors that save beside the original, rename the old file
    // out of the way, then move the completed temporary file into place.
    fs::rename(&fixture.asset_path, &backup_path).expect("move original aside");
    fs::rename(&temporary_path, &fixture.asset_path).expect("install replacement");
    fs::remove_file(&backup_path).expect("remove editor backup");

    assert!(update_until(
        &mut fixture.app,
        Duration::from_secs(5),
        |world| {
            world.resource::<Captured>().refreshes.iter().any(|event| {
                event.asset_id == asset_id.untyped()
                    && event.cause == RefreshCause::Direct
                    && event.delta.modified.contains("same")
            })
        }
    ));

    std::thread::sleep(Duration::from_millis(200));
    fixture.app.update();
    let captured = fixture.app.world().resource::<Captured>();
    assert!(
        captured
            .removed
            .iter()
            .all(|event| event.asset_id != asset_id.untyped()),
        "a completed rename replacement must not be exposed as removal"
    );
    assert_eq!(
        fixture
            .app
            .world()
            .resource::<Assets<ConfigAsset<RecoveryDb>>>()
            .get(asset_id)
            .expect("replacement asset should remain loaded")
            .raw
            .items[0]
            .n,
        2
    );
}
