//! MainMenu.unity: the six-button menu and the windows it opens.

use bevy::prelude::*;

use super::{Action, GameState};
use crate::MainCamera;
use crate::unity_ui::{UnityActive, UnityNode};

/// The 3D half of MainMenu.unity: the Caestus Metallican, its planet and nebulae, lit as in the scene.
#[derive(Component)]
pub struct Backdrop;

const SCENE: &str = "models/MainMenuScene.glb";
const CAMERA: &str = "Main Camera";

pub fn spawn_backdrop(mut commands: Commands, assets: Res<AssetServer>, pack: Res<crate::PackDir>) {
    if !pack.0.join(SCENE).exists() {
        warn!("{SCENE} missing from the pack; menu without its 3D backdrop");
        return;
    }
    let scene = assets.load(GltfAssetLabel::Scene(0).from_asset(format!("pack://{SCENE}")));
    commands.spawn((WorldAssetRoot(scene), Transform::default(), Backdrop));
}

/// Once the scene is in, view it through the Unity camera. Unity cameras look along +Z, Bevy's along -Z.
pub fn frame_backdrop(
    named: Query<(&Name, &GlobalTransform)>,
    mut camera: Query<&mut Transform, With<MainCamera>>,
    mut done: Local<bool>,
    state: Res<State<GameState>>,
) {
    if *state.get() != GameState::MainMenu {
        *done = false;
        return;
    }
    if *done {
        return;
    }
    let Some((_, global)) = named.iter().find(|(name, _)| name.as_str() == CAMERA) else { return };
    if let Ok(mut transform) = camera.single_mut() {
        *transform = global.compute_transform() * Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI));
        transform.scale = Vec3::ONE;
        *done = true;
    }
}

/// Unity light intensities have no physical unit; scale the converted ones to Bevy's.
/// `COGITATOR_LIGHT_SCALE` overrides the factor while tuning.
pub fn scale_lights(
    mut points: Query<&mut PointLight, Added<PointLight>>,
    mut spots: Query<&mut SpotLight, Added<SpotLight>>,
    mut suns: Query<&mut DirectionalLight, Added<DirectionalLight>>,
) {
    let env = |name: &str, default: f32| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    let k = env("COGITATOR_LIGHT_SCALE", LIGHT_SCALE);
    let sun = env("COGITATOR_SUN_SCALE", SUN_SCALE);
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

pub const LIGHT_SCALE: f32 = 40.0;
pub const SUN_SCALE: f32 = 1200.0;

pub fn despawn_backdrop(mut commands: Commands, backdrop: Query<Entity, With<Backdrop>>) {
    for entity in &backdrop {
        commands.entity(entity).despawn();
    }
}

pub fn bind(target: &str, method: &str, _arg: &str) -> Option<Action> {
    Some(match (target, method) {
        ("MainMenuNewGame", "OnClick") => Action::OpenWindow("UIWindowNewGame"),
        ("MainMenuLoad", "OnClick") => Action::OpenWindow("UIWindowLoadGame"),
        ("MainMenuSettings", "OnClick") => Action::OpenWindow("UIWindowSettings"),
        ("MainMenuCredits", "OnClick") => Action::OpenWindow("UIWindowCredits"),
        ("MainMenuQuit", "OnClick") => Action::Quit,
        ("MainMenuRollover", "OnRollover") => Action::Rollover,
        ("UIWindowNewGame", "OnClickStart") | ("UIWindowNewGame", "OnClickNewGame") => Action::Goto(GameState::Intro),
        (_, "OnClickBtnBuyHeretek") => Action::Ignore,
        _ => return None,
    })
}

/// What MainMenu.cs decides at startup: no save yet, so no Continue; base-game logo; no store banners;
/// gamepad helpers hidden on touch.
pub fn prepare(mut nodes: Query<(&UnityNode, &mut UnityActive)>) {
    const HIDE: &[&str] = &[
        "BtnContinue", "LogoHeretek", "LogoHeretekPlus", "BuyDlcHeretek", "M2Artwork", "UIMainMenuBlocker",
        "ControllerHelper", "BtnGamepad", "GamepadHighlight",
    ];
    for (node, mut active) in &mut nodes {
        if HIDE.contains(&node.name.as_str()) {
            active.0 = false;
        }
    }
}
