//! Cogitator: a bring-your-own-copy engine for Warhammer 40,000: Mechanicus.
//!
//! This app ships no game content. On launch it syncs a content pack converted from the player's own
//! installation and served from their PC over Tailscale.

mod pack;

use bevy::prelude::*;
use std::sync::{Arc, Mutex};

/// Base URL of the pack server. Override at build time with the COGITATOR_SERVER environment variable.
const SERVER: &str = match option_env!("COGITATOR_SERVER") {
    Some(url) => url,
    None => "http://desktop-3rsf4r5:8790",
};

#[derive(Resource, Clone, Default)]
struct Status(Arc<Mutex<String>>);

impl Status {
    fn set(&self, text: String) {
        *self.0.lock().unwrap() = text;
    }
}

#[derive(Component)]
struct StatusText;

fn main() {
    App::new()
        .add_plugins(DefaultPlugins)
        .insert_resource(Status::default())
        .add_systems(Startup, (setup, start_sync))
        .add_systems(Update, show_status)
        .run();
}

fn setup(mut commands: Commands) {
    commands.spawn(Camera2d);
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
        status.set(match result {
            Ok(manifest) => summarize(&manifest, &dir),
            Err(error) => format!("Pack sync failed:\n{error}\n\nIs the pack server running on your PC?"),
        });
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
    let current = status.0.lock().unwrap().clone();
    for mut text in &mut texts {
        if text.0 != current {
            text.0 = current.clone();
        }
    }
}
