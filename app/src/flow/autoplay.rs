//! Development autoplay (`COGITATOR_AUTOPLAY=1`): plays the game through its own HUD actions, a beat
//! apart, so desktop snapshots and soak runs reach later screens: first choices in dialogs, walking to
//! the next event, fighting (attack what is in reach, else close in), deploying on the first mission.

use bevy::prelude::*;

use super::GameState;
use super::campaign::{Campaign, GameDb};
use super::hud::Hud;
use crate::rules::battle::{Kind, Side, distance};

pub fn enabled() -> bool {
    std::env::var_os("COGITATOR_AUTOPLAY").is_some()
}

pub fn autoplay(state: Res<State<GameState>>, campaign: Res<Campaign>, db: Option<Res<GameDb>>, time: Res<Time>, mut wait: Local<f32>, mut out: MessageWriter<Hud>, mut step: Local<u32>) {
    let Some(db) = db else { return };
    *wait += time.delta_secs();
    if *wait < 0.7 {
        return;
    }
    *wait = 0.0;
    *step += 1;
    match state.get() {
        GameState::Exploration => {
            let Some(ex) = campaign.expedition.as_ref() else { return };
            if ex.dialog.is_some() {
                out.write(Hud::Choose(0));
            } else if let Some(target) = ex.next_event_room(&db.0) {
                out.write(Hud::MoveRoom(ex.step_towards(target).unwrap_or(target)));
            }
        }
        GameState::Battle => {
            let Some(b) = campaign.battle.as_ref() else { return };
            if b.outcome.is_some() {
                out.write(Hud::Continue);
                return;
            }
            if !b.is_player_turn() {
                return;
            }
            let who = b.current().unwrap();
            let attack = (0..b.fighters[who].skills.len()).find_map(|s| {
                if b.skill_blocked(&db.0, who, s).is_some() {
                    return None;
                }
                b.targets(&db.0, who, s).into_iter().find(|&t| b.fighters[t].side == Side::Necron).map(|t| (s, t))
            });
            if let Some((s, t)) = attack {
                out.write(Hud::Skill(s));
                out.write(Hud::Target(t));
                return;
            }
            let goal = b.fighters.iter().filter(|f| f.side == Side::Necron && f.kind == Kind::Unit && (f.alive() || f.knocked_out.is_some())).map(|f| f.cell).next();
            let me = b.fighters[who].cell;
            let reach: Vec<_> = b.reachable(who, b.fighters[who].moves_left()).into_keys().filter(|&c| b.move_cost(who, c) == Some(0)).collect();
            match goal.and_then(|g| reach.iter().copied().min_by(|a, c| distance(*a, g).total_cmp(&distance(*c, g)))) {
                Some(cell) if cell != me && b.fighters[who].moved == 0.0 => {
                    out.write(Hud::MoveCell(cell.0, cell.1));
                }
                _ => {
                    out.write(Hud::EndTurn);
                }
            }
        }
        GameState::Ark => {
            match *step % 3 {
                0 => out.write(Hud::Mission(0)),
                1 => out.write(Hud::Launch),
                _ => out.write(Hud::Tab(0)),
            };
        }
        _ => {}
    }
}
