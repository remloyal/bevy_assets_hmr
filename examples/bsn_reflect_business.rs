//! BSN business reaction that needs reflected Handle tracking.
//!
//! Bevy can reload the image and update the Sprite without this crate. The
//! extra behavior here is business state: every BSN entity that references
//! the image receives the new pixel dimensions and a reload revision. Remove
//! `ReflectHandleTrackingPlugin` and the image still changes, but
//! `AssetChanged<Image>::target_entities` is empty and this state is not
//! updated.

use bevy::prelude::*;
use bevy_assets_hmr::{
    AssetChanged, ConfigHmrAppExt, ReflectHandleTrackingPlugin, asset_watcher_system,
};

const IMAGE_PATH: &str = "textures/bg.jpg";

#[derive(Component, Reflect, FromTemplate)]
#[reflect(Component)]
struct BsnImageConsumer {
    image: Handle<Image>,
}

#[derive(Component, Reflect, FromTemplate, Default)]
#[reflect(Component)]
struct ImageLayoutState {
    reload_revision: u32,
    pixel_width: u32,
    pixel_height: u32,
}

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "bevy_assets_hmr - BSN Business Reaction".into(),
            resolution: (960, 540).into(),
            ..default()
        }),
        ..default()
    }));
    app.register_asset_reflect::<Image>();
    app.register_type::<BsnImageConsumer>();
    app.register_type::<ImageLayoutState>();
    app.add_plugins(ReflectHandleTrackingPlugin::default());
    app.watch_asset::<Image>();
    app.add_systems(Startup, spawn_scene);
    app.add_systems(
        Update,
        apply_image_layout_update.after(asset_watcher_system::<Image>),
    );
    app.run();
}

fn spawn_scene(mut commands: Commands) {
    commands.queue_spawn_scene(bsn! {
        Camera2d
        BsnImageConsumer { image: IMAGE_PATH }
        ImageLayoutState
        Sprite { image: IMAGE_PATH }
        Children [
            (
                BsnImageConsumer { image: IMAGE_PATH }
                ImageLayoutState
            ),
        ]
    });
}

fn apply_image_layout_update(
    mut reader: MessageReader<AssetChanged<Image>>,
    images: Res<Assets<Image>>,
    mut consumers: Query<(&mut ImageLayoutState, Option<&mut Sprite>)>,
) {
    for event in reader.read() {
        if event.target_entities.is_empty() {
            continue;
        }
        let Some(image) = images.get(event.asset_id) else {
            continue;
        };
        let size = image.size();
        info!(
            "Image changed: asset={:?}, targeted BSN entities={}, new_size={}x{}",
            event.asset_id,
            event.target_entities.len(),
            size.x,
            size.y,
        );

        for entity in &event.target_entities {
            let Ok((mut state, sprite)) = consumers.get_mut(*entity) else {
                continue;
            };
            state.reload_revision += 1;
            state.pixel_width = size.x;
            state.pixel_height = size.y;
            if let Some(mut sprite) = sprite {
                let natural_size = size.as_vec2().max(Vec2::ONE);
                let fit_scale = (840.0 / natural_size.x)
                    .min(460.0 / natural_size.y)
                    .min(1.0);
                sprite.custom_size = Some(natural_size * fit_scale);
            }
            info!(
                "  rebuilt derived image layout: entity={entity:?}, revision={}",
                state.reload_revision,
            );
        }
    }
}
