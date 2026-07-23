//! BSN code-first scene with automatic reflected Handle tracking.
//!
//! Unlike `bsn_asset_bind`, this example deliberately does not attach
//! `AssetBind<Image>`. The final ECS component is registered with
//! `#[reflect(Component)]`, so `ReflectHandleTrackingPlugin` discovers both
//! the root and nested scene entities after deferred scene commands apply.

use bevy::{asset::HandleTemplate, ecs::template::OptionTemplate, prelude::*};
// use bevy_assets_hmr::{ConfigHmrAppExt, ConfigHmrPlugin, ReflectHandleTrackingPlugin};

const BASE_COLOR: &str = "textures/bg.jpg";

#[derive(Component, Reflect, FromTemplate)]
// #[reflect(Component)]
struct BsnRenderComponent {
    base_color: Handle<Image>,
    #[template(OptionTemplate<HandleTemplate<Image>>)]
    normal: Option<Handle<Image>>,
}

fn main() {
    let mut app = App::new();
    app.add_plugins(DefaultPlugins.set(WindowPlugin {
        primary_window: Some(Window {
            title: "bevy_assets_hmr - BSN Reflect Handle Tracking".into(),
            resolution: (960, 540).into(),
            ..default()
        }),
        ..default()
    }));
    app.register_type::<BsnRenderComponent>();

    // app.register_asset_reflect::<Image>();
    // app.add_plugins(ConfigHmrPlugin::default());
    // app.add_plugins(ReflectHandleTrackingPlugin::default());
    // app.watch_asset::<Image>();

    app.add_systems(Startup, spawn_scene);
    app.run();
}

fn spawn_scene(mut commands: Commands) {
    // `bsn!` resolves the Handle<Image> path through FromTemplate. The
    // scanner observes the resulting component values, not the BSN syntax.
    commands.queue_spawn_scene(bsn! {
        Camera2d
        BsnRenderComponent {
            base_color: BASE_COLOR,
        }
        Sprite { image: BASE_COLOR }
        Children [
            (
                BsnRenderComponent {
                    base_color: BASE_COLOR,
                }
            ),
        ]
    });
}
