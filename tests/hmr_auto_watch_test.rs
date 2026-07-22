use bevy::asset::{Asset, AssetPlugin};
use bevy::ecs::message::Messages;
use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy_asset_loader::prelude::AssetCollection;
use bevy_assets_hmr::{
    ConfigAsset, ConfigHmrPlugin, ConfigRefresh, HmrAutoWatch, HmrSource, LastSnapshot,
    SimpleConfigDiff,
};
use serde::{Deserialize, Serialize};

#[derive(States, Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
enum TestState {
    #[default]
    Loading,
    Ready,
}

#[derive(Asset, TypePath, Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
struct WrappedConfig {
    value: u32,
}

impl SimpleConfigDiff for WrappedConfig {}

#[derive(AssetCollection, Resource, HmrAutoWatch)]
#[allow(dead_code)]
struct MixedCollection {
    #[asset(path = "data/config.ron")]
    config: Handle<ConfigAsset<WrappedConfig>>,
    #[asset(path = "textures/icon.png")]
    image: Handle<Image>,
}

#[derive(Asset, TypePath, Clone, Debug, PartialEq, Default)]
struct DirectConfig {
    value: u32,
}

impl SimpleConfigDiff for DirectConfig {}

impl HmrSource for DirectConfig {
    type Config = Self;

    fn config(&self) -> &Self::Config {
        self
    }
}

#[derive(AssetCollection, Resource, HmrAutoWatch)]
#[allow(dead_code)]
struct DirectCollection {
    #[asset(path = "configs/direct.custom")]
    #[hmr_watch]
    config: Handle<DirectConfig>,
    #[asset(path = "textures/icon.png")]
    image: Handle<Image>,
}

fn test_app() -> App {
    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins,
        bevy::state::app::StatesPlugin,
        AssetPlugin::default(),
    ))
    .add_plugins(ConfigHmrPlugin::default())
    .init_state::<TestState>();
    app
}

#[test]
fn config_asset_is_watched_while_regular_assets_are_ignored() {
    let mut app = test_app();
    app.add_plugins(MixedCollection::hmr_plugin(TestState::Ready));

    assert!(
        app.world()
            .contains_resource::<LastSnapshot<ConfigAsset<WrappedConfig>>>()
    );
    assert!(
        app.world()
            .contains_resource::<Messages<ConfigRefresh<WrappedConfig>>>()
    );
}

#[test]
fn hmr_watch_opts_a_direct_asset_into_hmr() {
    let mut app = test_app();
    app.init_asset::<DirectConfig>()
        .add_plugins(DirectCollection::hmr_plugin(TestState::Ready));

    assert!(
        app.world()
            .contains_resource::<LastSnapshot<DirectConfig>>()
    );
    assert!(
        app.world()
            .contains_resource::<Messages<ConfigRefresh<DirectConfig>>>()
    );
}
