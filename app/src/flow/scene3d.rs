//! The 3D half of a Unity scene: the converted glTF, viewed through the scene's own camera and lit by
//! its converted lights.

use bevy::prelude::*;

use super::GameState;
use crate::MainCamera;

/// Unity light intensities have no physical unit; these factors map the converted ones to Bevy's
/// (tuned by eye against the main menu). `COGITATOR_LIGHT_SCALE` / `COGITATOR_SUN_SCALE` override.
pub const LIGHT_SCALE: f32 = 40.0;
pub const SUN_SCALE: f32 = 1200.0;

/// (glTF in the pack, camera node, field of view in degrees) per state.
fn scene_of(state: GameState) -> Option<(&'static str, &'static str, f32)> {
    match state {
        GameState::MainMenu => Some(("models/MainMenuScene.glb", "Main Camera", 30.0)),
        GameState::Exploration => Some(("models/ExplorationScene.glb", "Main Camera", 21.0)),
        _ => None,
    }
}

#[derive(Component)]
pub struct Scene3d;

#[derive(Resource)]
pub struct Framing {
    camera: &'static str,
    fov: f32,
    done: bool,
}

pub fn spawn(mut commands: Commands, state: Res<State<GameState>>, assets: Res<AssetServer>, pack: Res<crate::PackDir>) {
    let Some((file, camera, fov)) = scene_of(*state.get()) else { return };
    if !pack.0.join(file).exists() {
        warn!("{file} missing from the pack; scene without its 3D half");
        return;
    }
    let scene = assets.load(GltfAssetLabel::Scene(0).from_asset(format!("pack://{file}")));
    commands.spawn((WorldAssetRoot(scene), Transform::default(), Scene3d));
    commands.insert_resource(Framing { camera, fov, done: false });
}

pub fn despawn(mut commands: Commands, scenes: Query<Entity, With<Scene3d>>) {
    for entity in &scenes {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<Framing>();
}

/// Once the scene is in, look through the Unity camera. Unity cameras face +Z, Bevy's face -Z.
pub fn frame(
    framing: Option<ResMut<Framing>>,
    named: Query<(&Name, &GlobalTransform)>,
    mut camera: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
) {
    let Some(mut framing) = framing else { return };
    if framing.done {
        return;
    }
    let Some((_, global)) = named.iter().find(|(name, _)| name.as_str() == framing.camera) else { return };
    if let Ok((mut transform, mut projection)) = camera.single_mut() {
        *transform = global.compute_transform() * Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI));
        transform.scale = Vec3::ONE;
        if let Projection::Perspective(p) = projection.as_mut() {
            p.fov = framing.fov.to_radians();
        }
        framing.done = true;
    }
}

pub fn scale_lights(
    mut points: Query<&mut PointLight, Added<PointLight>>,
    mut spots: Query<&mut SpotLight, Added<SpotLight>>,
    mut suns: Query<&mut DirectionalLight, Added<DirectionalLight>>,
) {
    let env = |name: &str, default: f32| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    let (k, sun) = (env("COGITATOR_LIGHT_SCALE", LIGHT_SCALE), env("COGITATOR_SUN_SCALE", SUN_SCALE));
    for mut light in &mut suns {
        light.illuminance *= sun;
    }
    for mut light in &mut points {
        light.intensity *= k;
    }
    for mut light in &mut spots {
        light.intensity *= k;
    }
}
