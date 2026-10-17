//! The essential checks, run against the real converted pack (`COGITATOR_PACK`, default ../content/pack).
//! They skip, rather than fail, where no pack exists (CI has no game files).
//!
//!   cargo test --release -- --nocapture     # prints the parity report

use std::path::PathBuf;

use bevy::prelude::*;
use bevy::render::RenderPlugin;
use bevy::render::settings::WgpuSettings;
use bevy::window::ExitCondition;
use bevy::winit::WinitPlugin;

use cogitator::flow::{self, Action, GameState};
use cogitator::unity_ui::{UEvent, UiDoc, UnityActive, UnityCall, UnityNode};

fn pack() -> Option<PathBuf> {
    let dir = std::env::var_os("COGITATOR_PACK")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../content/pack"));
    if dir.join("manifest.json").exists() {
        Some(dir)
    } else {
        eprintln!("skipped: no content pack at {}", dir.display());
        None
    }
}

#[test]
fn pack_matches_its_manifest() {
    let Some(dir) = pack() else { return };
    let manifest: serde_json::Value = serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap();
    let files = manifest["files"].as_array().unwrap();
    let missing: Vec<_> = files
        .iter()
        .filter(|f| {
            let path = dir.join(f["path"].as_str().unwrap());
            std::fs::metadata(&path).ok().map(|m| m.len()) != f["size"].as_u64()
        })
        .map(|f| f["path"].as_str().unwrap().to_string())
        .collect();
    assert!(missing.is_empty(), "missing or changed: {missing:?}");
}

#[test]
fn main_menu_has_the_games_buttons_in_its_language() {
    let Some(dir) = pack() else { return };
    let doc = UiDoc::load(&dir, "MainMenu").unwrap();
    let loc = cogitator::loc::Loc::load(&dir, "en").unwrap();
    let labels: Vec<String> = doc
        .walk()
        .into_iter()
        .filter(|(path, _)| path.contains("MainMenu/Container/Menu/"))
        .filter_map(|(_, node)| node.text.as_ref())
        .map(|t| t.term.as_deref().and_then(|term| loc.get(term)).unwrap_or(&t.text).to_string())
        .collect();
    for expected in ["New game", "Load game", "Settings", "Credits", "Exit"] {
        assert!(labels.iter().any(|l| l.eq_ignore_ascii_case(expected)), "{expected:?} not in {labels:?}");
    }
}

/// Every click the main menu can fire, bound or reported. Prints the parity report for the scene.
#[test]
fn main_menu_parity() {
    let Some(dir) = pack() else { return };
    let doc = UiDoc::load(&dir, "MainMenu").unwrap();
    let mut bound = 0;
    let mut unported = std::collections::BTreeSet::new();
    for (_, node) in doc.walk() {
        for event in node.events.iter().filter(|e| !e.is_hover()) {
            match flow::bind(&event.target, &event.method, &event.arg) {
                Action::Unported(name) => {
                    unported.insert(name);
                }
                _ => bound += 1,
            }
        }
    }
    eprintln!("MainMenu: {bound} calls bound, {} unported: {unported:#?}", unported.len());
    for core in ["MainMenuNewGame.OnClick", "MainMenuSettings.OnClick", "MainMenuQuit.OnClick"] {
        assert!(!unported.contains(core), "{core} must be bound");
    }
}

fn headless(dir: PathBuf) -> App {
    let mut app = App::new();
    cogitator::register_pack_source(&mut app, &dir);
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin { primary_window: None, exit_condition: ExitCondition::DontExit, ..default() })
            .set(RenderPlugin {
                render_creation: WgpuSettings { backends: None, ..default() }.into(),
                ..default()
            })
            .disable::<WinitPlugin>(),
    )
    .add_plugins(cogitator::GamePlugin { pack: dir, sync: false });
    app
}

fn find(app: &mut App, name: &str) -> Option<Entity> {
    let mut q = app.world_mut().query::<(Entity, &UnityNode)>();
    q.iter(app.world()).find(|(_, n)| n.name == name).map(|(e, _)| e)
}

/// Boot to the main menu like the game, press New Game, and get its window.
#[test]
fn boots_to_main_menu_and_opens_new_game() {
    let Some(dir) = pack() else { return };
    let mut app = headless(dir);
    for _ in 0..10 {
        app.update();
    }
    assert_eq!(*app.world().resource::<State<GameState>>().get(), GameState::MainMenu);
    let button = find(&mut app, "ButtonNewGame").expect("New Game button spawned");

    app.world_mut().write_message(UnityCall {
        source: button,
        event: UEvent { on: "Button".into(), target: "MainMenuNewGame".into(), method: "OnClick".into(), arg: String::new() },
    });
    app.update();
    let window = find(&mut app, "NewGameWindow").expect("New Game window exists");
    assert!(app.world().get::<UnityActive>(window).unwrap().0, "New Game window opened");
}
