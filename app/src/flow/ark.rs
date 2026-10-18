//! ArkGlobal.unity + ArkUI.unity: the Ark between missions. Missions on offer, the tech-priests and
//! their equipment, the armoury of owned blueprints, and deployment into a tomb.

use bevy::prelude::*;

use super::campaign::{Campaign, GameDb};
use super::hud::{self, AMBER, DIM, GREEN, Hud, HudRoot, RED, Ui, tr};
use super::GameState;
use crate::loc::Loc;
use crate::rules::{ark, battle::unit_skills, save::Save};
use crate::unity_ui::Fonts;
use crate::PackDir;

#[derive(Resource, Default)]
pub struct ArkView {
    tab: usize,
    mission: Option<usize>,
    chosen: Vec<bool>,
    troops: Vec<usize>,
    techpriest: usize,
    message: Option<(String, Color)>,
    version: u64,
    drawn: u64,
}

pub fn set_last_result(save: &mut Save, success: bool, blackstone: i32) {
    save.story.last_result = Some((success, blackstone));
}

pub fn enter(mut commands: Commands, mut campaign: ResMut<Campaign>, pack: Res<PackDir>) {
    let mut view = ArkView { version: 1, ..default() };
    if let Some(save) = campaign.save.as_mut() {
        view.chosen = vec![true; save.techpriests.len().min(4)];
        view.chosen.resize(save.techpriests.len(), false);
        if let Some((ok, bs)) = save.story.last_result.take() {
            view.message = Some(if ok { (format!("Mission complete. +{bs} blackstone recovered."), GREEN) } else { (format!("Mission failed. +{bs} blackstone recovered."), RED) });
        }
    }
    campaign.store(&pack.0);
    commands.insert_resource(view);
}

pub fn exit(mut commands: Commands) {
    commands.remove_resource::<ArkView>();
}

pub fn act(
    mut actions: MessageReader<Hud>,
    db: Res<GameDb>,
    mut campaign: ResMut<Campaign>,
    view: Option<ResMut<ArkView>>,
    mut next: ResMut<NextState<GameState>>,
    pack: Res<PackDir>,
) {
    let Some(mut view) = view else { return };
    for action in actions.read() {
        let Some(save) = campaign.save.as_mut() else { return };
        view.message = None;
        match action {
            Hud::Tab(t) => {
                view.tab = *t;
                view.mission = None;
            }
            Hud::Mission(m) => {
                view.mission = Some(*m);
                view.troops.clear();
            }
            Hud::ToggleUnit(i) if *i < 100 => {
                if let Some(c) = view.chosen.get_mut(*i) {
                    *c = !*c;
                }
            }
            Hud::ToggleUnit(i) => {
                let t = i - 100;
                if let Some(pos) = view.troops.iter().position(|x| *x == t) {
                    view.troops.remove(pos);
                } else {
                    view.troops.push(t);
                }
            }
            Hud::Choose(tp) => view.techpriest = *tp,
            Hud::Equip(tp, bp) => {
                let blueprint = save.blueprints.get(*bp).copied();
                view.message = Some(match blueprint.map(|b| ark::equip(&db.0, save, *tp, b)) {
                    Some(Ok(())) => ("Equipped.".into(), GREEN),
                    Some(Err(e)) => (e, RED),
                    None => ("No such blueprint.".into(), RED),
                });
                campaign.store(&pack.0);
            }
            Hud::Launch => {
                let missions = ark::available_missions(&db.0, save);
                let Some(mission) = view.mission.and_then(|m| missions.get(m)).map(|m| m.id) else { continue };
                let tps: Vec<usize> = view.chosen.iter().enumerate().filter(|(_, c)| **c).map(|(i, _)| i).collect();
                let troops: Vec<_> = ark::troops(&db.0, save);
                let troop_refs: Vec<_> = view.troops.iter().filter_map(|&t| troops.get(t)).map(|(_, d)| d.id).collect();
                match ark::deploy(&db.0, save, mission, &tps, &troop_refs) {
                    Ok(()) => {
                        campaign.expedition = None;
                        campaign.store(&pack.0);
                        next.set(GameState::Exploration);
                        return;
                    }
                    Err(e) => view.message = Some((e, RED)),
                }
            }
            Hud::Back => view.mission = None,
            Hud::Menu => {
                campaign.store(&pack.0);
                next.set(GameState::MainMenu);
                return;
            }
            _ => continue,
        }
        view.version += 1;
    }
}

pub fn draw(
    mut commands: Commands,
    campaign: Res<Campaign>,
    db: Res<GameDb>,
    loc: Res<Loc>,
    fonts: Res<Fonts>,
    view: Option<ResMut<ArkView>>,
    roots: Query<Entity, With<HudRoot>>,
) {
    let (Some(save), Some(mut view)) = (campaign.save.as_ref(), view) else { return };
    if view.drawn == view.version {
        return;
    }
    view.drawn = view.version;
    for e in &roots {
        commands.entity(e).despawn();
    }
    let ui = Ui { fonts: &fonts };
    let db = &db.0;
    // Header: resources and tabs.
    commands
        .spawn((
            Node { position_type: PositionType::Absolute, top: Val::Px(20.0), left: Val::Px(30.0), right: Val::Px(30.0), align_items: AlignItems::Center, column_gap: Val::Px(16.0), ..default() },
            GlobalZIndex(4_000),
            HudRoot,
        ))
        .with_children(|p| {
            p.spawn(ui.title("Ark Mechanicus", 44.0, GREEN));
            p.spawn(ui.text(format!("Blackstone {}", save.blackstone), 28.0, AMBER));
            p.spawn(ui.text(format!("Necron awakening {}%", save.story.overall_necron_countdown.clamp(0, 100)), 28.0, RED));
            for (i, name) in ["Missions", "Tech-priests", "Armoury"].iter().enumerate() {
                ui.button(p, Hud::Tab(i), if view.tab == i { format!("[{name}]") } else { name.to_string() }, 26.0, true);
            }
            ui.button(p, Hud::Menu, "Main menu", 24.0, true);
        });
    if let Some((m, c)) = &view.message {
        commands.spawn((
            Node { position_type: PositionType::Absolute, top: Val::Px(100.0), left: Val::Px(30.0), ..default() },
            GlobalZIndex(4_000),
            HudRoot,
            children![ui.text(m.clone(), 28.0, *c)],
        ));
    }
    let body = commands.spawn((hud::panel(Val::Px(1300.0), Val::Px(150.0), Val::Px(310.0)), HudRoot)).id();
    let missions = ark::available_missions(db, save);
    commands.entity(body).with_children(|p| match view.tab {
        0 => match view.mission {
            None => {
                p.spawn(ui.title("Missions", 36.0, GREEN));
                if missions.is_empty() {
                    p.spawn(ui.text("No missions are available.", 26.0, DIM));
                }
                for (i, m) in missions.iter().enumerate() {
                    let (name, desc) = ark::mission_terms(m);
                    let giver = db.follow(m, "missionGiver").map(|c| tr(&loc, c.str("name"))).unwrap_or_default();
                    ui.button(p, Hud::Mission(i), format!("{}  —  {}", tr(&loc, &name), giver), 28.0, true);
                    p.spawn(ui.text(tr(&loc, &desc), 22.0, DIM));
                }
            }
            Some(mi) => {
                let Some(m) = missions.get(mi) else { return };
                let (name, desc) = ark::mission_terms(m);
                p.spawn(ui.title(format!("Deploy: {}", tr(&loc, &name)), 36.0, GREEN));
                p.spawn(ui.text(tr(&loc, &desc), 24.0, DIM));
                p.spawn(ui.text("Tech-priests", 28.0, Color::WHITE));
                for (i, tp) in save.techpriests.iter().enumerate() {
                    let on = view.chosen.get(i).copied().unwrap_or(false);
                    ui.button(p, Hud::ToggleUnit(i), format!("{} {}  {}/{}", if on { "[x]" } else { "[ ]" }, tp.name, tp.hp, tp.hp_max), 24.0, true);
                }
                let troops = ark::troops(db, save);
                let used: i32 = view.troops.iter().filter_map(|&t| troops.get(t)).map(|(_, d)| ark::troop_cost(d)).sum();
                p.spawn(ui.text(format!("Troops ({used} / {} capacity)", ark::capacity(db)), 28.0, Color::WHITE));
                for (i, (bp, data)) in troops.iter().enumerate() {
                    let count = view.troops.iter().filter(|&&t| t == i).count();
                    let can_add = used + ark::troop_cost(data) <= ark::capacity(db);
                    ui.button(p, Hud::ToggleUnit(100 + i), format!("{} {}  (cost {})", if count > 0 { "[x]" } else { "[ ]" }, tr(&loc, data.str("unitName")).trim().to_string() + if bp.name().is_empty() { "" } else { "" }, ark::troop_cost(data)), 24.0, count > 0 || can_add);
                }
                p.spawn(Node { column_gap: Val::Px(12.0), ..default() }).with_children(|row| {
                    ui.button(row, Hud::Launch, "Launch mission", 30.0, true);
                    ui.button(row, Hud::Back, "Back", 26.0, true);
                });
            }
        },
        1 => {
            p.spawn(ui.title("Tech-priests", 36.0, GREEN));
            for tp in &save.techpriests {
                p.spawn(ui.text(format!("{}  level {}  {}/{} hp", tp.name, tp.level, tp.hp, tp.hp_max), 28.0, Color::WHITE));
                let gear: Vec<String> = tp.equipment.values().filter_map(|e| db.get(*e)).map(|e| e.name().to_string()).collect();
                p.spawn(ui.text(format!("Equipment: {}", if gear.is_empty() { "none".into() } else { gear.join(", ") }), 22.0, DIM));
                let skills: Vec<String> = unit_skills(db, tp).iter().filter_map(|s| db.get(*s)).map(|s| s.name().to_string()).collect();
                p.spawn(ui.text(format!("Skills: {}", skills.join(", ")), 22.0, DIM));
            }
        }
        _ => {
            p.spawn(ui.title("Armoury", 36.0, GREEN));
            p.spawn(Node { column_gap: Val::Px(8.0), flex_wrap: FlexWrap::Wrap, ..default() }).with_children(|row| {
                for (i, tp) in save.techpriests.iter().enumerate() {
                    ui.button(row, Hud::Choose(i), if view.techpriest == i { format!("[{}]", tp.name) } else { tp.name.clone() }, 22.0, true);
                }
            });
            for (bi, b) in save.blueprints.iter().enumerate() {
                let Some(bp) = db.get(*b) else { continue };
                let cost = bp.int("cost");
                ui.button(p, Hud::Equip(view.techpriest, bi), format!("{}  ({} blackstone)", bp.name(), cost), 24.0, save.blackstone as i64 >= cost);
            }
        }
    });
}
