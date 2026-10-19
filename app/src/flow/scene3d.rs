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
        GameState::Ark => Some(("models/ArkScene.glb", "ArkMissionCamera", 42.0)),
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

impl Framing {
    pub fn done(&self) -> bool {
        self.done
    }
}

pub fn spawn(mut commands: Commands, state: Res<State<GameState>>, assets: Res<AssetServer>, pack: Res<crate::PackDir>) {
    let Some((file, camera, fov)) = scene_of(*state.get()) else { return };
    if !pack.0.join(file).exists() {
        warn!("{file} missing from the pack; scene without its 3D half");
        return;
    }
    let scene = assets.load(GltfAssetLabel::Scene(0).from_asset(format!("pack://{file}")));
    let gltf: Handle<bevy::gltf::Gltf> = assets.load(format!("pack://{file}"));
    commands.spawn((WorldAssetRoot(scene), Transform::default(), Scene3d, super::battle::UnitModel(gltf)));
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
    rig: Option<Res<super::camera::Rig>>,
    named: Query<(&Name, &GlobalTransform)>,
    mut camera: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
) {
    let Some(mut framing) = framing else { return };
    if framing.done || rig.is_some() {
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

/// Local-light scale for scenes lit by their own baked ambient (tombs and battles, which are near-black
/// without their lights).
pub const DARK_SCENE_LIGHT_SCALE: f32 = 300.0;

pub fn scale_lights(
    mut points: Query<&mut PointLight, Added<PointLight>>,
    mut spots: Query<&mut SpotLight, Added<SpotLight>>,
    mut suns: Query<&mut DirectionalLight, Added<DirectionalLight>>,
    state: Res<State<GameState>>,
) {
    let env = |name: &str, default: f32| std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default);
    let base = if matches!(state.get(), GameState::Battle | GameState::Exploration) { DARK_SCENE_LIGHT_SCALE } else { LIGHT_SCALE };
    let (k, sun) = (env("COGITATOR_LIGHT_SCALE", base), env("COGITATOR_SUN_SCALE", SUN_SCALE));
    for mut light in &mut suns {
        light.illuminance *= sun;
        if std::env::var_os("COGITATOR_NO_SUN_SHADOW").is_some() {
            light.shadow_maps_enabled = false;
        }
    }
    if std::env::var_os("COGITATOR_NO_LOCAL_LIGHTS").is_some() {
        for mut light in &mut points {
            light.intensity = 0.0;
            light.range = 0.01;
        }
        for mut light in &mut spots {
            light.intensity = 0.0;
            light.range = 0.01;
        }
    }
    // Only the sun casts shadows: battle rooms carry dozens of point lights, and a cube shadow map each
    // over hundreds of meshes is far beyond a phone (and stalled a desktop GPU).
    for mut light in &mut points {
        light.intensity *= k;
        light.shadow_maps_enabled = false;
    }
    for mut light in &mut spots {
        light.intensity *= k;
        light.shadow_maps_enabled = false;
    }
}

/// Unity effect shaders have no PBR equivalent: light shafts, portal cards and glows draw additive and
/// unlit (faintly, as their textures are masks), and editor/HUD helper meshes are hidden.
/// The tomb table rides on the camera container (CameraContainer's children: table, holo grid, planes), so
/// it follows the rig across the tomb as in the game.
pub fn carry_table(rig: Option<Res<super::camera::Rig>>, mut nodes: Query<(&Name, &mut Transform)>, state: Res<State<GameState>>) {
    if *state.get() != GameState::Exploration {
        return;
    }
    let Some(rig) = rig else { return };
    for (name, mut t) in &mut nodes {
        if name.as_str() == "CameraContainer" {
            t.translation = Vec3::new(-rig.position[0], rig.position[1], rig.position[2]);
        }
    }
}

/// Geometry drawn with the game's "Opti Fade" shader, which cuts away whatever is nearer the camera than
/// the global GLOBAL_WALL_FADE distance (set to 20 by TaskExploInitialize) so walls never hide the floor.
#[derive(Component)]
pub struct FadeWall;

pub const WALL_FADE: f32 = 20.0;

/// Per-object version of that cut (the shader does it per pixel): hide fade walls whose nearest point is
/// within WALL_FADE of the camera; re-checked when the camera moves.
pub fn fade_walls(
    camera: Query<&GlobalTransform, (With<crate::MainCamera>, Changed<GlobalTransform>)>,
    mut walls: Query<(&GlobalTransform, &bevy::camera::primitives::Aabb, &mut Visibility), With<FadeWall>>,
    added: Query<(), Added<FadeWall>>,
    cam_any: Query<&GlobalTransform, With<crate::MainCamera>>,
) {
    let cam = match camera.single() {
        Ok(c) => c,
        Err(_) if !added.is_empty() => match cam_any.single() {
            Ok(c) => c,
            Err(_) => return,
        },
        Err(_) => return,
    };
    let eye = cam.translation();
    let fade = std::env::var("COGITATOR_WALL_FADE").ok().and_then(|v| v.parse().ok()).unwrap_or(WALL_FADE);
    for (t, aabb, mut vis) in &mut walls {
        let m = t.affine();
        let centre = m.transform_point3a(aabb.center);
        let half: Vec3 = (m.matrix3.x_axis.abs() * aabb.half_extents.x + m.matrix3.y_axis.abs() * aabb.half_extents.y + m.matrix3.z_axis.abs() * aabb.half_extents.z).into();
        let lo = Vec3::from(centre) - Vec3::from(half);
        let hi = Vec3::from(centre) + Vec3::from(half);
        let nearest = eye.clamp(lo, hi);
        let wanted = if nearest.distance(eye) < fade { Visibility::Hidden } else { Visibility::Inherited };
        if *vis != wanted {
            *vis = wanted;
        }
    }
}

/// The scene's baked ambient (RenderSettings' ambient probe, spherical-harmonic L0 per channel) as Bevy's
/// global ambient. AMBIENT_SCALE maps the probe into Bevy's brightness units (tuned against the game).
pub const AMBIENT_SCALE: f32 = 2500.0;

/// The ambient the main menu and the Ark were tuned with (their scenes have no usable baked probe).
pub fn default_ambient(mut ambient: ResMut<GlobalAmbientLight>) {
    ambient.color = Color::srgb(0.55, 0.6, 0.7);
    ambient.brightness = 120.0;
}

pub fn apply_render(render: Option<&serde_json::Value>, ambient: &mut GlobalAmbientLight) {
    let Some(sh) = render.and_then(|r| r.get("ambient_sh0")).and_then(|v| v.as_array()) else { return };
    let c: Vec<f32> = sh.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
    if c.len() < 3 {
        return;
    }
    let peak = c[0].max(c[1]).max(c[2]);
    let scale = std::env::var("COGITATOR_AMBIENT_SCALE").ok().and_then(|v| v.parse().ok()).unwrap_or(AMBIENT_SCALE);
    if peak <= 0.0 {
        ambient.brightness = 0.0;
        return;
    }
    ambient.color = Color::linear_rgb(c[0] / peak, c[1] / peak, c[2] / peak);
    ambient.brightness = peak * scale;
}

/// Unity emission, as the game's shaders compute it: `_EmissionColor` x `_EmissionIntensity`, masked by the
/// emission channel of the packed `_MetallicAOEmiSmooth` texture, and switched off when `_Emission` is 0.
/// EMISSION_SCALE maps Unity's HDR emission into Bevy's units (tuned against the game's look).
pub const EMISSION_SCALE: f32 = 6.0;

pub fn effect_materials(
    mut nodes: Query<(Entity, &bevy::gltf::GltfMaterialExtras, &MeshMaterial3d<StandardMaterial>, &mut Visibility), Added<bevy::gltf::GltfMaterialExtras>>,
    mut commands: Commands,
    mut materials: ResMut<Assets<StandardMaterial>>,
    assets: Res<AssetServer>,
    mut done: Local<std::collections::HashSet<AssetId<StandardMaterial>>>,
) {
    if std::env::var_os("COGITATOR_NO_FX").is_some() {
        return;
    }
    for (entity, extras, material, mut visibility) in &mut nodes {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&extras.value) else { continue };
        let shader = v.get("unity_shader").and_then(|s| s.as_str()).unwrap_or("");
        // Portal cards are layered fog quads tuned for Unity's post-processing; without it they wash out.
        if shader.contains("Opti Fade") {
            commands.entity(entity).insert(FadeWall);
        }
        // Unity's texture tiling (exported as extras.tiling = [scale x, scale y, offset x, offset y]); glTF's
        // flipped v puts the offset at 1 - scale - offset.
        if let Some(t) = v.get("tiling").and_then(|t| t.as_array())
            && t.len() == 4
            && let Some(mut m) = materials.get_mut(&material.0)
        {
            let f: Vec<f32> = t.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect();
            m.uv_transform = bevy::math::Affine2::from_scale_angle_translation(Vec2::new(f[0], f[1]), 0.0, Vec2::new(f[2], 1.0 - f[1] - f[3]));
        }
        let holo = shader.contains("Grid Explo") || shader.contains("Props/Ark Mechanicus") || shader.contains("Nebulae") || shader.contains("Table Light");
        if holo {
            // The tomb table's holograms: grid, layered holo planes, nebula floor, table light.
            if let Some(mut m) = materials.get_mut(&material.0) {
                m.alpha_mode = AlphaMode::Add;
                m.unlit = true;
                // Faint until material tiling and these shaders are ported (the grid texture tiles in Unity).
                let tint = if shader.contains("Grid") {
                    Color::srgba(0.1, 0.35, 0.45, 0.25)
                } else if shader.contains("Table Light") {
                    Color::srgba(0.1, 0.3, 0.4, 0.03)
                } else {
                    Color::srgba(0.05, 0.15, 0.22, 0.06)
                };
                m.base_color = tint;
                if !shader.contains("Grid") {
                    m.base_color_texture = None;
                }
            }
        } else if shader.contains("/HUD/") || shader.contains("Hidden/") || shader.contains("Portal Cards") || shader.contains("Light Shaft") {
            *visibility = Visibility::Hidden;
        } else if shader.contains("/FX/") || shader.contains("Particles/") || shader.contains("Additive") {
            if let Some(mut m) = materials.get_mut(&material.0) {
                m.alpha_mode = AlphaMode::Add;
                m.unlit = true;
                let strength = if shader.contains("Light Shaft") { 0.05 } else { 0.3 };
                m.base_color = m.base_color.with_alpha(strength);
            }
        } else if done.insert(material.0.id()) {
            let colour = v.pointer("/colors/_EmissionColor").and_then(|c| c.as_array()).map(|c| c.iter().map(|x| x.as_f64().unwrap_or(0.0) as f32).collect::<Vec<_>>());
            let floats = v.get("floats");
            let get = |k: &str| floats.and_then(|f| f.get(k)).and_then(|x| x.as_f64()).map(|x| x as f32);
            let enabled = get("_Emission").is_none_or(|e| e > 0.0);
            let intensity = get("_EmissionIntensity").unwrap_or(1.0);
            if let (Some(c), true) = (colour, enabled)
                && c.len() >= 3
                && c[0] + c[1] + c[2] > 0.0
                && intensity > 0.0
                && let Some(mut m) = materials.get_mut(&material.0)
            {
                let k = intensity * EMISSION_SCALE;
                m.emissive = LinearRgba::rgb(c[0] * k, c[1] * k, c[2] * k);
                if let Some(mask) = v.get("emission_mask").and_then(|x| x.as_str()).filter(|_| std::env::var_os("COGITATOR_NO_EMISSION_MASKS").is_none()) {
                    m.emissive_texture = Some(assets.load(format!("pack://models/{mask}")));
                }
            }
        }
    }
}
