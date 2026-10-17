//! Cogitator: a bring-your-own-copy engine for Warhammer 40,000: Mechanicus.
//!
//! This app ships no game content. On launch it syncs a content pack converted from the player's own
//! installation and served from their PC over Tailscale, then runs the game from it.

pub mod flow;
#[cfg(target_os = "ios")]
pub mod ios_scene;
pub mod loc;
pub mod pack;
pub mod unity_ui;

use bevy::asset::io::AssetSourceBuilder;
use bevy::prelude::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use flow::GameState;

/// Base URL of the pack server. Override at build time with the COGITATOR_SERVER environment variable
/// (CI passes an empty string when the repository variable is unset, so empty means default).
pub const SERVER: &str = match option_env!("COGITATOR_SERVER") {
    Some(url) if !url.is_empty() => url,
    _ => "http://desktop-3rsf4r5:8790",
};

/// Where the content pack lives.
#[derive(Resource, Clone)]
pub struct PackDir(pub PathBuf);

/// Pack sync progress, written by the sync thread and shown during Boot.
#[derive(Resource, Clone, Default)]
pub struct Status {
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

/// The pack folder, plus whether to sync it: `COGITATOR_PACK` points desktop runs and tests at a local
/// converted pack and skips the network.
pub fn pack_location() -> (PathBuf, bool) {
    match std::env::var_os("COGITATOR_PACK") {
        Some(dir) => (PathBuf::from(dir), false),
        None => (pack::pack_dir(), true),
    }
}

/// Everything except windowing and rendering, so tests can run the game headless.
pub struct GamePlugin {
    pub pack: PathBuf,
    pub sync: bool,
}

impl Plugin for GamePlugin {
    fn build(&self, app: &mut App) {
        let status = Status::default();
        if !self.sync {
            status.ready.store(true, Ordering::Release);
        }
        app.insert_resource(PackDir(self.pack.clone()))
            .insert_resource(status)
            .insert_resource(SyncWanted(self.sync))
            .add_plugins((unity_ui::UnityUiPlugin, flow::FlowPlugin))
            .add_systems(Startup, (setup, start_sync))
            .add_systems(Update, (show_status, finish_boot).run_if(in_state(GameState::Boot)))
            .add_systems(OnExit(GameState::Boot), hide_status);
    }
}

#[derive(Resource)]
struct SyncWanted(bool);

/// Register the `pack://` asset source; must happen before `DefaultPlugins`.
pub fn register_pack_source(app: &mut App, pack: &std::path::Path) {
    app.register_asset_source("pack", AssetSourceBuilder::platform_default(pack.to_str().expect("pack path is UTF-8"), None));
}

/// The full app as shipped.
pub fn run() {
    let (pack, sync) = pack_location();
    let mut app = App::new();
    #[cfg(target_os = "ios")]
    {
        ios_scene::register();
        app.add_systems(Update, ios_scene::attach_primary_window);
    }
    register_pack_source(&mut app, &pack);
    match std::env::var_os("COGITATOR_SNAPSHOT") {
        // Desktop check: save a PNG of the running game and exit. Kept small and vsynced (800x600, display
        // refresh) to spare the GPU; COGITATOR_SNAPSHOT_SOFTWARE=1 renders on the CPU instead.
        Some(path) => {
            use bevy::render::{RenderPlugin, settings::WgpuSettings};
            use bevy::window::PresentMode;
            let software = std::env::var_os("COGITATOR_SNAPSHOT_SOFTWARE").is_some();
            app.add_plugins(DefaultPlugins.set(RenderPlugin {
                render_creation: WgpuSettings { force_fallback_adapter: software, ..default() }.into(),
                ..default()
            }).set(WindowPlugin {
                primary_window: Some(Window {
                    resolution: (800, 600).into(),
                    present_mode: PresentMode::AutoVsync,
                    title: "Cogitator snapshot".into(),
                    ..default()
                }),
                ..default()
            }))
            .add_plugins(GamePlugin { pack, sync })
            .insert_resource(Snapshot(PathBuf::from(path), 0, 0.0))
            .add_systems(Update, snapshot)
            .run();
        }
        None => {
            app.add_plugins(DefaultPlugins).add_plugins(GamePlugin { pack, sync }).run();
        }
    }
}

#[derive(Resource)]
struct Snapshot(PathBuf, u32, f32);

/// After the main menu has had time to load its assets, capture it and quit.
fn snapshot(
    mut commands: Commands,
    mut shot: ResMut<Snapshot>,
    state: Res<State<GameState>>,
    mut exit: MessageWriter<AppExit>,
    time: Res<Time<Real>>,
) {
    use bevy::render::view::screenshot::{Screenshot, save_to_disk};
    if *state.get() == GameState::Boot {
        return;
    }
    // Software rendering is slow, so wait in wall-clock seconds (assets load in the background).
    let wait: f32 = std::env::var("COGITATOR_SNAPSHOT_SECS").ok().and_then(|v| v.parse().ok()).unwrap_or(25.0);
    shot.2 += time.delta_secs();
    if shot.1 == 0 && shot.2 >= wait {
        commands.spawn(Screenshot::primary_window()).observe(save_to_disk(shot.0.clone()));
        shot.1 = 1;
    } else if shot.1 > 0 {
        shot.1 += 1;
        // Leave once the capture has been written (or after a generous number of frames).
        if shot.0.exists() || shot.1 > 600 {
            exit.write(AppExit::Success);
        }
    }
}

/// The one camera: posed per scene from the converted Unity camera, and drawing the UI on top.
#[derive(Component)]
pub struct MainCamera;

fn setup(mut commands: Commands) {
    commands.spawn((
        Camera3d::default(),
        Camera { clear_color: ClearColorConfig::Custom(Color::srgb(0.01, 0.012, 0.02)), ..default() },
        Projection::Perspective(PerspectiveProjection { fov: 30f32.to_radians(), near: 0.3, far: 200_000.0, ..default() }),
        MainCamera,
    ));
    commands.insert_resource(GlobalAmbientLight { color: Color::srgb(0.55, 0.6, 0.7), brightness: 120.0, ..default() });
    commands.spawn((
        Text::new("Cogitator"),
        TextFont::from_font_size(22.0),
        Node { margin: UiRect::all(Val::Px(48.0)), ..default() },
        StatusText,
    ));
}

fn start_sync(status: Res<Status>, wanted: Res<SyncWanted>, pack: Res<PackDir>) {
    if !wanted.0 {
        return;
    }
    let status = status.clone();
    let dir = pack.0.clone();
    std::thread::spawn(move || {
        match pack::sync(SERVER, &dir, &|line| status.set(line)) {
            Ok(_) => status.ready.store(true, Ordering::Release),
            // Offline: play from the last synced pack if there is one.
            Err(error) if dir.join("manifest.json").exists() => {
                status.set(format!("Pack server unreachable ({error}); using the last synced pack."));
                status.ready.store(true, Ordering::Release);
            }
            Err(error) => status.set(format!("Pack sync failed:\n{error}\n\nIs the pack server running on your PC?")),
        }
    });
}

fn show_status(status: Res<Status>, mut texts: Query<&mut Text, With<StatusText>>) {
    let current = status.text.lock().unwrap().clone();
    for mut text in &mut texts {
        if text.0 != current {
            text.0 = current.clone();
        }
    }
}

/// Initialize.unity's job: once the pack is in place, load localization and fonts, then the main menu.
fn finish_boot(
    mut commands: Commands,
    status: Res<Status>,
    pack: Res<PackDir>,
    assets: Res<AssetServer>,
    mut next: ResMut<NextState<GameState>>,
) {
    if !status.ready.load(Ordering::Acquire) {
        return;
    }
    let language = std::env::var("COGITATOR_LANG").unwrap_or_else(|_| "en".into());
    match loc::Loc::load(&pack.0, &language) {
        Ok(loc) => {
            info!("localization {language}: {} terms", loc.len());
            commands.insert_resource(loc);
        }
        Err(e) => {
            status.set(format!("Content pack incomplete: {e}"));
            return;
        }
    }
    commands.insert_resource(unity_ui::Fonts::load(&pack.0, &assets));
    next.set(GameState::MainMenu);
}

fn hide_status(mut texts: Query<&mut Visibility, With<StatusText>>) {
    for mut visibility in &mut texts {
        *visibility = Visibility::Hidden;
    }
}
