//! Cogitator: a bring-your-own-copy engine for Warhammer 40,000: Mechanicus.
//!
//! This app ships no game content. On launch it syncs a content pack converted from the player's own
//! installation and served from their PC over Tailscale.

mod pack;

use bevy::asset::io::AssetSourceBuilder;
use bevy::prelude::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

/// Base URL of the pack server. Override at build time with the COGITATOR_SERVER environment variable.
const SERVER: &str = match option_env!("COGITATOR_SERVER") {
    Some(url) => url,
    None => "http://desktop-3rsf4r5:8790",
};

/// First model shown once the pack is in place: proof that converted geometry, textures and animation work.
const SHOWCASE_MODEL: &str = "models/Warrior.glb";
const SHOWCASE_CLIP: &str = "Idle_01";

#[derive(Resource, Clone, Default)]
struct Status {
    text: Arc<Mutex<String>>,
    ready: Arc<AtomicBool>,
}

impl Status {
    fn set(&self, text: String) {
        *self.text.lock().unwrap() = text;
    }
}

#[derive(Component)]
struct StatusText;

#[derive(Component)]
struct Showcase;

/// The showcase model's glTF, kept so its named animation clips can be looked up once loaded.
#[derive(Resource)]
struct ShowcaseGltf(Handle<Gltf>);

fn main() {
    let pack_dir = pack::pack_dir();
    App::new()
        .register_asset_source(
            "pack",
            AssetSourceBuilder::platform_default(pack_dir.to_str().expect("pack path is UTF-8"), None),
        )
        .add_plugins(DefaultPlugins)
        .insert_resource(Status::default())
        .add_systems(Startup, (setup, start_sync))
        .add_systems(Update, (show_status, spawn_showcase, turn_showcase, animate_showcase))
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn((Camera3d::default(), Transform::from_xyz(0.0, 1.3, 4.5).looking_at(Vec3::new(0.0, 1.0, 0.0), Vec3::Y)));
    commands.spawn((
        DirectionalLight {
            illuminance: 8_000.0,
            ..default()
        },
        Transform::from_xyz(2.0, 4.0, 3.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Text::new("Cogitator"),
        Node {
            margin: UiRect::all(Val::Px(64.0)),
            ..default()
        },
        StatusText,
    ));
}

fn start_sync(status: Res<Status>) {
    let status = status.clone();
    std::thread::spawn(move || {
        let dir = pack::pack_dir();
        let result = pack::sync(SERVER, &dir, &|line| status.set(line));
        match result {
            Ok(manifest) => {
                status.set(summarize(&manifest, &dir));
                status.ready.store(true, Ordering::Release);
            }
            Err(error) => status.set(format!("Pack sync failed:\n{error}\n\nIs the pack server running on your PC?")),
        }
    });
}

/// Count the records in each exported data table, as proof the pack arrived intact and parses.
fn summarize(manifest: &pack::Manifest, dir: &std::path::Path) -> String {
    let mut lines = vec![format!("Pack ready: {} files in {}", manifest.files.len(), dir.display())];
    for file in manifest.files.iter().filter(|f| f.path.starts_with("data/") && !f.path.contains("/_")) {
        let records = std::fs::read(dir.join(&file.path))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Vec<serde_json::Value>>(&bytes).ok())
            .map_or(0, |records| records.len());
        lines.push(format!("  {}: {records} records", file.path));
    }
    lines.join("\n")
}

fn show_status(status: Res<Status>, mut texts: Query<&mut Text, With<StatusText>>) {
    let current = status.text.lock().unwrap().clone();
    for mut text in &mut texts {
        if text.0 != current {
            text.0 = current.clone();
        }
    }
}

fn spawn_showcase(
    mut commands: Commands,
    status: Res<Status>,
    assets: Res<AssetServer>,
    existing: Query<(), With<Showcase>>,
) {
    if !existing.is_empty() || !status.ready.load(Ordering::Acquire) {
        return;
    }
    if !pack::pack_dir().join(SHOWCASE_MODEL).exists() {
        return;
    }
    let scene = assets.load(GltfAssetLabel::Scene(0).from_asset(format!("pack://{SHOWCASE_MODEL}")));
    commands.insert_resource(ShowcaseGltf(assets.load(format!("pack://{SHOWCASE_MODEL}"))));
    commands.spawn((WorldAssetRoot(scene), Transform::default(), Showcase));
}

/// The glTF loader puts an AnimationPlayer on the animated root; start the showcase clip on it, looping.
fn animate_showcase(
    mut commands: Commands,
    showcase: Option<Res<ShowcaseGltf>>,
    gltfs: Res<Assets<Gltf>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
    mut players: Query<(Entity, &mut AnimationPlayer), Without<AnimationGraphHandle>>,
) {
    let Some(clip) = showcase
        .and_then(|s| gltfs.get(&s.0))
        .and_then(|gltf| gltf.named_animations.get(SHOWCASE_CLIP).cloned())
    else {
        return;
    };
    for (entity, mut player) in &mut players {
        let (graph, node) = AnimationGraph::from_clip(clip.clone());
        commands.entity(entity).insert(AnimationGraphHandle(graphs.add(graph)));
        player.play(node).repeat();
    }
}

fn turn_showcase(time: Res<Time>, mut models: Query<&mut Transform, With<Showcase>>) {
    for mut transform in &mut models {
        transform.rotate_y(0.4 * time.delta_secs());
    }
}
