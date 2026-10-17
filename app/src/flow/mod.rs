//! Game flow: states in the order Unity loads its scenes, and the seam between converted UI events and
//! ported logic. Every Unity persistent call is bound by `Class.Method` to an `Action`; a call with no
//! binding is reported in-game as not yet ported and counted in `Parity`.

pub mod exploration;
pub mod main_menu;
pub mod scene3d;

use bevy::prelude::*;
use std::collections::BTreeMap;

use crate::loc::Loc;
use crate::unity_ui::{self, Fonts, UiDoc, UnityActive, UnityCall, UnityNode, UnityUiRoot};

/// Unity's scene order: Initialize → MainMenu → Intro → Exploration ⇄ Battle, with the Ark between missions.
#[derive(States, Default, Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum GameState {
    #[default]
    Boot,
    MainMenu,
    Intro,
    Exploration,
    Battle,
    Ark,
}

impl GameState {
    /// The converted Unity UI scenes each state shows.
    pub fn ui_scenes(self) -> &'static [&'static str] {
        match self {
            GameState::MainMenu => &["MainMenu"],
            GameState::Exploration => &["DungeonUI"],
            _ => &[],
        }
    }
}

/// What a bound Unity call does.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    OpenWindow(&'static str),
    CloseWindow(&'static str),
    Goto(GameState),
    Rollover,
    Quit,
    /// Deliberately does nothing in the port (analytics, store links, console-only paths).
    Ignore,
    /// Not ported yet; the name is shown to the player.
    Unported(String),
}

/// Bind a Unity call to an action.
pub fn bind(target: &str, method: &str, arg: &str) -> Action {
    main_menu::bind(target, method, arg)
        .or_else(|| exploration::bind(target, method, arg))
        .or_else(|| windows::bind(target, method))
        .unwrap_or_else(|| Action::Unported(format!("{target}.{method}")))
}

mod windows {
    use super::Action;

    /// Generic UIWindow* behaviour: close-style methods close their own window.
    pub fn bind(target: &str, method: &str) -> Option<Action> {
        let class = WINDOWS.iter().find(|w| **w == target)?;
        let m = method.to_ascii_lowercase();
        (m.contains("close") || m.contains("back") || m.contains("cancel")).then_some(Action::CloseWindow(class))
    }

    pub const WINDOWS: &[&str] = &[
        "UIWindowNewGame", "UIWindowNewGameExtraSettings", "UIWindowSettings", "UIWindowSettingsConsole",
        "UIWindowLoadGame", "UIWindowWarning", "UIWindowInfo", "UIWindowCredits", "UIWindowMenuPauseDungeon",
        "UIWindowDifficultySettings", "UIWindowSaveGame",
    ];
}

/// Parity, measured while playing: unported calls hit, by name.
#[derive(Resource, Default)]
pub struct Parity {
    pub unported: BTreeMap<String, u32>,
    pub last: Option<String>,
}

impl Parity {
    /// Record something the player reached that isn't ported yet.
    pub fn report(&mut self, name: &str) {
        warn!("not yet ported: {name}");
        *self.unported.entry(name.to_string()).or_default() += 1;
        self.last = Some(name.to_string());
    }
}

#[derive(Component)]
struct ParityToast;

pub struct FlowPlugin;

impl Plugin for FlowPlugin {
    fn build(&self, app: &mut App) {
        app.init_state::<GameState>()
            .init_resource::<Parity>()
            .add_systems(Startup, spawn_toast)
            .add_systems(OnEnter(GameState::MainMenu), ((spawn_scene_ui, main_menu::prepare).chain(), scene3d::spawn))
            .add_systems(OnEnter(GameState::Intro), exploration::skip_intro)
            .add_systems(OnEnter(GameState::Exploration), ((spawn_scene_ui, exploration::prepare).chain(), scene3d::spawn))
            .add_systems(OnEnter(GameState::Battle), unported_state)
            .add_systems(OnEnter(GameState::Ark), unported_state)
            .add_systems(Update, (run_calls, show_toast, scene3d::frame, scene3d::scale_lights));
        for state in [GameState::MainMenu, GameState::Exploration] {
            app.add_systems(OnExit(state), (despawn_scene_ui, scene3d::despawn));
        }
    }
}

pub fn spawn_scene_ui(
    mut commands: Commands,
    state: Res<State<GameState>>,
    assets: Res<AssetServer>,
    fonts: Res<Fonts>,
    loc: Res<Loc>,
    pack: Res<crate::PackDir>,
) {
    for scene in state.get().ui_scenes() {
        match UiDoc::load(&pack.0, scene) {
            Ok(doc) => {
                unity_ui::spawn(&mut commands, &doc, &assets, &fonts, &loc);
            }
            Err(e) => error!("UI scene {scene}: {e}"),
        }
    }
}

fn despawn_scene_ui(mut commands: Commands, roots: Query<Entity, With<UnityUiRoot>>) {
    for root in &roots {
        commands.entity(root).despawn();
    }
}

/// A state whose scene isn't ported yet still loads, says so, and offers the way back.
fn unported_state(state: Res<State<GameState>>, mut parity: ResMut<Parity>) {
    parity.report(&format!("Scene {:?}", state.get()));
}

fn run_calls(
    mut calls: MessageReader<UnityCall>,
    mut nodes: Query<(&UnityNode, &mut UnityActive)>,
    mut next: ResMut<NextState<GameState>>,
    mut parity: ResMut<Parity>,
    mut exit: MessageWriter<AppExit>,
) {
    for call in calls.read() {
        let e = &call.event;
        match bind(&e.target, &e.method, &e.arg) {
            Action::OpenWindow(class) => set_window(&mut nodes, class, true),
            Action::CloseWindow(class) => set_window(&mut nodes, class, false),
            Action::Goto(state) => next.set(state),
            Action::Quit => {
                exit.write(AppExit::Success);
            }
            Action::Rollover | Action::Ignore => {}
            Action::Unported(name) => parity.report(&name),
        }
    }
}

/// Show or hide the window carrying `class`, with the shared background blocker behind it.
fn set_window(nodes: &mut Query<(&UnityNode, &mut UnityActive)>, class: &str, open: bool) {
    let mut any_open = false;
    for (node, mut active) in nodes.iter_mut() {
        if node.has_script(class) {
            active.0 = open;
        }
        if open || (node.scripts.iter().any(|s| windows::WINDOWS.contains(&s.as_str())) && active.0) {
            any_open = true;
        }
    }
    for (node, mut active) in nodes.iter_mut() {
        if node.name == "UIBackgroundBlocker" {
            active.0 = any_open;
        }
    }
}

fn spawn_toast(mut commands: Commands) {
    commands.spawn((
        Text::new(""),
        TextFont::from_font_size(18.0),
        TextColor(Color::srgb(1.0, 0.55, 0.35)),
        Node { position_type: PositionType::Absolute, bottom: Val::Px(12.0), left: Val::Px(12.0), ..default() },
        GlobalZIndex(10_000),
        bevy::picking::Pickable::IGNORE,
        ParityToast,
    ));
}

fn show_toast(parity: Res<Parity>, mut toast: Query<&mut Text, With<ParityToast>>) {
    if !parity.is_changed() {
        return;
    }
    if let (Some(last), Ok(mut text)) = (&parity.last, toast.single_mut()) {
        text.0 = format!("Not yet ported: {last}   ({} distinct so far)", parity.unported.len());
    }
}
