use bevy::asset::{Asset, AssetPlugin, Assets};
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_assets_hmr::{ConfigDiff, ConfigHmrAppExt, ConfigHmrPlugin, HmrSource};
use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

#[derive(Asset, TypePath, Clone, Debug, PartialEq)]
struct SlowDb {
    value: u32,
}

static DIFF_ACTIVE: AtomicBool = AtomicBool::new(false);
static ASSET_READER_ACTIVE: AtomicBool = AtomicBool::new(false);
static OVERLAP_OBSERVED: AtomicBool = AtomicBool::new(false);

impl ConfigDiff for SlowDb {
    type Id = String;

    fn diff(old: &Self, new: &Self) -> (HashSet<String>, HashSet<String>, HashSet<String>) {
        DIFF_ACTIVE.store(true, Ordering::SeqCst);
        if ASSET_READER_ACTIVE.load(Ordering::SeqCst) {
            OVERLAP_OBSERVED.store(true, Ordering::SeqCst);
        }
        std::thread::sleep(Duration::from_millis(50));
        DIFF_ACTIVE.store(false, Ordering::SeqCst);
        if old == new {
            return Default::default();
        }
        (HashSet::new(), HashSet::new(), ["value".to_string()].into())
    }
}

impl HmrSource for SlowDb {
    type Config = SlowDb;

    fn config(&self) -> &Self::Config {
        self
    }
}

fn ordinary_asset_reader(_assets: Res<Assets<SlowDb>>) {
    ASSET_READER_ACTIVE.store(true, Ordering::SeqCst);
    if DIFF_ACTIVE.load(Ordering::SeqCst) {
        OVERLAP_OBSERVED.store(true, Ordering::SeqCst);
    }
    std::thread::sleep(Duration::from_millis(50));
    ASSET_READER_ACTIVE.store(false, Ordering::SeqCst);
}

#[test]
fn ordinary_assets_read_can_overlap_hmr_diff_read() {
    DIFF_ACTIVE.store(false, Ordering::SeqCst);
    ASSET_READER_ACTIVE.store(false, Ordering::SeqCst);
    OVERLAP_OBSERVED.store(false, Ordering::SeqCst);

    bevy::tasks::IoTaskPool::get_or_init(|| {
        bevy::tasks::TaskPoolBuilder::new().num_threads(2).build()
    });
    let mut app = App::new();
    app.add_plugins((
        bevy::app::TaskPoolPlugin::default(),
        AssetPlugin::default(),
        bevy::time::TimePlugin,
    ));
    app.add_plugins(ConfigHmrPlugin {
        debounce_window: Duration::from_millis(0),
    });
    app.setup_hmr_headless();
    app.init_asset::<SlowDb>();
    app.register_asset::<SlowDb>("data/parallelism.ron");
    app.add_systems(Update, ordinary_asset_reader);

    let asset_id = bevy::asset::AssetId::<SlowDb>::Uuid {
        uuid: uuid::Uuid::new_v4(),
    };
    let _ = app
        .world_mut()
        .resource_mut::<Assets<SlowDb>>()
        .insert(asset_id, SlowDb { value: 1 });
    for _ in 0..3 {
        app.update();
    }

    let _ = app
        .world_mut()
        .resource_mut::<Assets<SlowDb>>()
        .insert(asset_id, SlowDb { value: 2 });
    for _ in 0..3 {
        app.update();
    }

    assert!(
        OVERLAP_OBSERVED.load(Ordering::SeqCst),
        "ordinary Res<Assets<_>> reader should overlap the HMR diff path"
    );
}
