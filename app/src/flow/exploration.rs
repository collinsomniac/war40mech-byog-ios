//! Exploration.unity + DungeonUI.unity: the tomb holo-table and its HUD.
//!
//! In Unity, DungeonManager builds the tomb's room map at runtime from the mission data; that is the
//! next system to port, so entering exploration reports it.

use bevy::prelude::*;

use super::{Action, GameState, Parity};
use crate::unity_ui::{UnityActive, UnityNode};

pub fn bind(target: &str, method: &str, _arg: &str) -> Option<Action> {
    Some(match (target, method) {
        ("UIWindowMenuPauseDungeon", m) if m.contains("MainMenu") || m.contains("Quit") => Action::Goto(GameState::MainMenu),
        _ => return None,
    })
}

/// DungeonUI holds both the exploration and battle HUDs; exploration shows its own and the shared one.
pub fn prepare(mut nodes: Query<(&UnityNode, &mut UnityActive)>, mut parity: ResMut<Parity>) {
    // Popups the exploration scripts open on demand.
    const CLOSED: &[&str] = &["UICanticles", "UIExploEvent", "UIExploEventEnd", "UIErrorMessage", "UIErrorMessageMiddle", "UIExploMissionStart"];
    for (node, mut active) in &mut nodes {
        match node.name.as_str() {
            "Explo" | "Common" => active.0 = true,
            "Battle" | "ControllerHelper" | "SaveInProgressWindow" | "UITurn" => active.0 = false,
            _ if CLOSED.iter().any(|c| node.has_script(c)) => active.0 = false,
            _ => {}
        }
    }
    parity.report("DungeonManager: tomb map generation");
}

/// The intro cutscene (01_Intro.unity, a Timeline) isn't ported: report it and skip, as the player can.
pub fn skip_intro(mut parity: ResMut<Parity>, mut next: ResMut<NextState<GameState>>) {
    parity.report("Cutscene 01_Intro");
    next.set(GameState::Exploration);
}
