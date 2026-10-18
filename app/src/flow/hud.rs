//! The game's own HUD widgets for screens whose Unity UI is filled in by code (event dialogs, battle
//! bars, the Ark's lists). Sizes are in the Unity UI's 1920x1080 reference pixels, so `UiScale` (set by
//! the converted UI's CanvasScaler) scales them with the rest of the interface.

use bevy::picking::events::PointerClick;
use bevy::prelude::*;

use crate::loc::{Loc, plain};
use crate::unity_ui::Fonts;

/// What a HUD button does; each screen handles its own.
#[derive(Debug, Clone, PartialEq, Message)]
pub enum Hud {
    Choose(usize),
    MoveRoom(usize),
    Skill(usize),
    Target(usize),
    MoveCell(i32, i32),
    EndTurn,
    Continue,
    Mission(usize),
    ToggleUnit(usize),
    Launch,
    Back,
    Tab(usize),
    Equip(usize, usize),
    Menu,
}

#[derive(Component)]
pub struct HudButton(pub Hud);

/// Everything a screen spawned for its HUD; despawned with the screen.
#[derive(Component)]
pub struct HudRoot;

pub const GREEN: Color = Color::srgb(0.45, 0.95, 0.35);
pub const AMBER: Color = Color::srgb(1.0, 0.72, 0.25);
pub const RED: Color = Color::srgb(1.0, 0.35, 0.3);
pub const DIM: Color = Color::srgb(0.6, 0.66, 0.6);
pub const PANEL: Color = Color::srgba(0.02, 0.05, 0.03, 0.92);
pub const BUTTON: Color = Color::srgba(0.08, 0.2, 0.1, 0.95);

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<Hud>().add_systems(Update, (clicks, reference_scale));
    }
}

fn clicks(mut clicks: MessageReader<PointerClick>, buttons: Query<&HudButton>, parents: Query<&ChildOf>, mut out: MessageWriter<Hud>) {
    for click in clicks.read() {
        let mut current = Some(click.entity);
        while let Some(e) = current {
            if let Ok(b) = buttons.get(e) {
                out.write(b.0.clone());
                break;
            }
            current = parents.get(e).ok().map(ChildOf::parent);
        }
    }
}

/// Screens without a converted Unity canvas still lay out against the game's 1920x1080 reference
/// (CanvasScaler "expand": the smaller of the two ratios).
fn reference_scale(roots: Query<(), With<crate::unity_ui::UnityUiRoot>>, windows: Query<&Window>, mut scale: ResMut<UiScale>) {
    if !roots.is_empty() {
        return;
    }
    let Some(w) = windows.iter().next() else { return };
    let wanted = (w.width() / 1920.0).min(w.height() / 1080.0).max(0.1);
    if (scale.0 - wanted).abs() > 1e-4 {
        scale.0 = wanted;
    }
}

/// Text helper: an I2 term (or literal text) in the player's language.
pub fn tr(loc: &Loc, term: &str) -> String {
    if term.is_empty() {
        return String::new();
    }
    loc.get(term).map(plain).unwrap_or_else(|| term.rsplit('/').next().unwrap_or(term).replace('_', " "))
}

pub struct Ui<'a> {
    pub fonts: &'a Fonts,
}

impl Ui<'_> {
    pub fn text(&self, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
        (
            Text::new(s),
            TextFont { font: self.fonts.get("Titillium-Regular SDF").into(), font_size: size.into(), ..default() },
            TextColor(color),
        )
    }

    pub fn title(&self, s: impl Into<String>, size: f32, color: Color) -> impl Bundle {
        (
            Text::new(s),
            TextFont { font: self.fonts.get("FjallaOne-Regular SDF").into(), font_size: size.into(), ..default() },
            TextColor(color),
        )
    }

    pub fn button(&self, parent: &mut ChildSpawnerCommands, action: Hud, label: impl Into<String>, size: f32, enabled: bool) -> Entity {
        let label = label.into();
        let mut e = parent.spawn((
            Node {
                padding: UiRect::axes(Val::Px(22.0), Val::Px(10.0)),
                margin: UiRect::all(Val::Px(6.0)),
                border: UiRect::all(Val::Px(2.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BorderColor::all(if enabled { GREEN } else { DIM }),
            BackgroundColor(BUTTON),
        ));
        if enabled {
            e.insert(HudButton(action));
        }
        let color = if enabled { GREEN } else { DIM };
        e.with_children(|b| {
            b.spawn((self.text(label, size, color), bevy::picking::Pickable::IGNORE));
        });
        e.id()
    }
}

pub fn panel(width: Val, top: Val, left: Val) -> impl Bundle {
    (
        Node {
            position_type: PositionType::Absolute,
            top,
            left,
            width,
            flex_direction: FlexDirection::Column,
            padding: UiRect::all(Val::Px(24.0)),
            border: UiRect::all(Val::Px(2.0)),
            row_gap: Val::Px(8.0),
            ..default()
        },
        BorderColor::all(Color::srgba(0.45, 0.95, 0.35, 0.6)),
        BackgroundColor(PANEL),
        GlobalZIndex(5_000),
    )
}

pub fn despawn_hud(mut commands: Commands, roots: Query<Entity, With<HudRoot>>) {
    for e in &roots {
        commands.entity(e).despawn();
    }
}
