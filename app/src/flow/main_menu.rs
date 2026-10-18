//! MainMenu.unity: the six-button menu and the windows it opens.

use bevy::prelude::*;

use super::{Action, GameState};
use crate::unity_ui::{UnityActive, UnityNode};

pub fn bind(target: &str, method: &str, _arg: &str) -> Option<Action> {
    Some(match (target, method) {
        ("MainMenuNewGame", "OnClick") => Action::OpenWindow("UIWindowNewGame"),
        ("MainMenuLoad", "OnClick") => Action::OpenWindow("UIWindowLoadGame"),
        ("MainMenuSettings", "OnClick") => Action::OpenWindow("UIWindowSettings"),
        ("MainMenuCredits", "OnClick") => Action::OpenWindow("UIWindowCredits"),
        ("MainMenuQuit", "OnClick") => Action::Quit,
        ("MainMenuRollover", "OnRollover") => Action::Rollover,
        ("UIWindowNewGame", "BtnNewGame") => Action::Goto(GameState::Intro),
        ("MainMenu", "OnClickBtnContinue") => Action::Continue,
        ("UIWindowNewGame", "BtnClose") => Action::CloseWindow("UIWindowNewGame"),
        ("UIWindowNewGame", "BtnExtraSettings") => Action::OpenWindow("UIWindowNewGameExtraSettings"),
        ("UIWindowNewGameExtraSettings", "BtnValidate") => Action::CloseWindow("UIWindowNewGameExtraSettings"),
        (_, "OnClickBtnBuyHeretek") => Action::Ignore,
        _ => return None,
    })
}

/// What MainMenu.cs decides at startup: Continue only with a save; base-game logo; no store banners;
/// gamepad helpers hidden on touch.
pub fn prepare(mut nodes: Query<(&UnityNode, &mut UnityActive)>, pack: Res<crate::PackDir>) {
    let has_save = super::campaign::Campaign::exists(&pack.0);
    for (node, mut active) in &mut nodes {
        if node.name == "BtnContinue" {
            active.0 = has_save;
        }
    }
    const HIDE: &[&str] = &[
        "LogoHeretek", "LogoHeretekPlus", "BuyDlcHeretek", "M2Artwork", "UIMainMenuBlocker",
        "ControllerHelper", "BtnGamepad", "GamepadHighlight",
    ];
    for (node, mut active) in &mut nodes {
        if HIDE.contains(&node.name.as_str()) {
            active.0 = false;
        }
    }
}
