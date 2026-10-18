//! Exploration.unity + DungeonUI.unity: the tomb on the holo-table. The rules (`rules::explore`) hold
//! the tomb; this draws its rooms on the table, pans the camera to the team, opens event dialogs, and
//! hands fights over to the battle screen.

use bevy::prelude::*;

use super::campaign::{Campaign, GameDb};
use super::hud::{self, AMBER, DIM, GREEN, Hud, HudRoot, RED, Ui, tr};
use super::{Action, GameState, Parity};
use crate::loc::Loc;
use crate::rules::battle::{Battle, room_exists};
use crate::rules::explore::{Expedition, Next, Outcome as Reward, awake_level, dialog_by_id};
use crate::unity_ui::{Fonts, UnityActive, UnityNode};
use crate::PackDir;

pub fn bind(target: &str, method: &str, _arg: &str) -> Option<Action> {
    Some(match (target, method) {
        ("UIWindowMenuPauseDungeon", m) if m.contains("MainMenu") || m.contains("Quit") => Action::Goto(GameState::MainMenu),
        _ => return None,
    })
}

/// DungeonUI holds both the exploration and battle HUDs; exploration shows its own and the shared one.
pub fn prepare(mut nodes: Query<(&UnityNode, &mut UnityActive)>) {
    // Popups the exploration scripts open on demand; the event dialog is drawn by the game HUD below.
    const CLOSED: &[&str] = &["UICanticles", "UIExploEvent", "UIExploEventEnd", "UIErrorMessage", "UIErrorMessageMiddle", "UIExploMissionStart"];
    for (node, mut active) in &mut nodes {
        match node.name.as_str() {
            "Explo" | "Common" => active.0 = true,
            "Battle" | "ControllerHelper" | "SaveInProgressWindow" | "UITurn" | "UIDeployment" | "EventsPreview" | "UIIndicator" | "UIInitiative" | "Event" | "EventEnd" => {
                active.0 = false
            }
            _ if CLOSED.iter().any(|c| node.has_script(c)) => active.0 = false,
            _ => {}
        }
    }
}

/// The intro cutscene (01_Intro.unity, a Timeline) isn't ported: report it, then do what its end does
/// (TutoLauncher.LaunchDeploy): send the tutorial team into the tutorial tomb.
pub fn skip_intro(mut parity: ResMut<Parity>, mut next: ResMut<NextState<GameState>>, db: Res<GameDb>, mut campaign: ResMut<Campaign>, pack: Res<PackDir>) {
    parity.report("Cutscene 01_Intro");
    // GameNewSaveState.NewGame with the New Game window's defaults.
    let mut save = match crate::rules::save::Save::new_game(&db.0, crate::rules::save::Settings::default()) {
        Ok(s) => s,
        Err(e) => {
            parity.report(&format!("New game: {e}"));
            return;
        }
    };
    if let Err(e) = save.deploy_tutorial(&db.0) {
        parity.report(&format!("Tutorial deployment: {e}"));
    }
    *campaign = Campaign { save: Some(save), ..default() };
    campaign.store(&pack.0);
    next.set(GameState::Exploration);
}

/// The map on the table: where it is anchored, its scale, and when it needs redrawing.
#[derive(Resource)]
pub struct TableMap {
    anchor: Option<Vec3>,
    scale: f32,
    drawn: u64,
    version: u64,
    /// The team's room on the floor (Unity coordinates), for the camera to centre on.
    focus: Option<Vec3>,
}

#[derive(Component)]
pub struct MapPiece;

#[derive(Component)]
pub struct RoomTile(pub usize);

pub fn enter(mut commands: Commands, db: Res<GameDb>, mut campaign: ResMut<Campaign>, pack: Res<PackDir>, mut parity: ResMut<Parity>, mut ambient: ResMut<GlobalAmbientLight>) {
    // Map.GridToWorld: one world unit per grid cell, on the table's floor at the origin.
    commands.insert_resource(TableMap { anchor: Some(Vec3::ZERO), scale: 1.0, drawn: 0, version: 1, focus: None });
    if let Some(rig) = super::camera::Rig::load(&pack.0.join("camera/Exploration.json")) {
        commands.insert_resource(rig);
    }
    let render: Option<serde_json::Value> = std::fs::read(pack.0.join("camera/Exploration.json")).ok().and_then(|b| serde_json::from_slice::<serde_json::Value>(&b).ok()).and_then(|v| v.get("render").cloned());
    super::scene3d::apply_render(render.as_ref(), &mut ambient);
    if campaign.expedition.is_some() {
        return;
    }
    let Some(save) = campaign.save.as_ref() else {
        parity.report("Exploration without a campaign");
        return;
    };
    match Expedition::begin(&db.0, save) {
        Ok(ex) => {
            campaign.expedition = Some(ex);
            campaign.store(&pack.0);
        }
        Err(e) => parity.report(&format!("Tomb generation: {e}")),
    }
}

pub fn exit(mut commands: Commands, pieces: Query<Entity, With<MapPiece>>) {
    for e in &pieces {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<TableMap>();
    commands.remove_resource::<super::camera::Rig>();
}

/// A room's Unity floor position: grid units, centred on the tomb (Map's offset).
fn room_unity(ex: &Expedition, pos: Vec2) -> Vec3 {
    let n = ex.tomb.rooms.len().max(1) as f32;
    let centre = ex.tomb.rooms.iter().map(|r| r.pos).sum::<Vec2>() / n;
    Vec3::new(pos.x - centre.x.round(), 0.0, pos.y - centre.y.round())
}

fn room_world(map: &TableMap, ex: &Expedition, pos: Vec2) -> Vec3 {
    let u = room_unity(ex, pos) * map.scale;
    map.anchor.unwrap_or(Vec3::ZERO) + Vec3::new(-u.x, u.y, u.z)
}

pub fn draw_map(
    mut commands: Commands,
    mut map: ResMut<TableMap>,
    campaign: Res<Campaign>,
    db: Res<GameDb>,
    pieces: Query<Entity, With<MapPiece>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if map.anchor.is_none() || map.drawn == map.version {
        return;
    }
    let Some(ex) = campaign.expedition.as_ref() else { return };
    map.drawn = map.version;
    for e in &pieces {
        commands.entity(e).despawn();
    }
    let s = map.scale;
    let glow = |materials: &mut Assets<StandardMaterial>, c: Color, strength: f32| {
        materials.add(StandardMaterial {
            base_color: c.with_alpha(0.55),
            emissive: LinearRgba::from(c) * strength,
            unlit: true,
            alpha_mode: AlphaMode::Blend,
            ..default()
        })
    };
    let tile = meshes.add(Cuboid::new(3.8 * s, 0.08 * s, 3.8 * s));
    let marker = meshes.add(Sphere::new(0.45 * s));
    let team = meshes.add(Cylinder::new(0.6 * s, 1.6 * s));
    for (i, room) in ex.tomb.rooms.iter().enumerate() {
        if !ex.visible(i) {
            continue;
        }
        let here = i == ex.room;
        let color = if here { GREEN } else if ex.visited.contains(&i) { Color::srgb(0.25, 0.7, 0.3) } else { Color::srgb(0.15, 0.4, 0.2) };
        let at = room_world(&map, ex, room.pos);
        commands.spawn((Mesh3d(tile.clone()), MeshMaterial3d(glow(&mut materials, color, 2.0)), Transform::from_translation(at), MapPiece, RoomTile(i)));
        if let Some(event) = ex.event_record(&db.0, i)
            && !ex.events[i].ended
        {
            let c = match event.int("type") {
                3 => RED,
                5 => Color::srgb(0.35, 0.6, 1.0),
                _ => AMBER,
            };
            commands.spawn((Mesh3d(marker.clone()), MeshMaterial3d(glow(&mut materials, c, 4.0)), Transform::from_translation(at + Vec3::new(1.1 * s, 0.6 * s, -1.1 * s)), MapPiece, RoomTile(i)));
        }
        if here {
            commands.spawn((Mesh3d(team.clone()), MeshMaterial3d(glow(&mut materials, Color::srgb(0.9, 1.0, 0.9), 3.0)), Transform::from_translation(at + Vec3::Y * 0.85 * s), MapPiece));
            map.focus = Some(room_unity(ex, room.pos));
        }
    }
    for link in &ex.tomb.links {
        if !(ex.visible(link.a) && ex.visible(link.b)) {
            continue;
        }
        let (a, b) = (room_world(&map, ex, ex.tomb.rooms[link.a].pos), room_world(&map, ex, ex.tomb.rooms[link.b].pos));
        let length = a.distance(b) - 3.8 * s;
        if length <= 0.0 {
            continue;
        }
        let mid = (a + b) / 2.0;
        let corridor = meshes.add(Cuboid::new(0.5 * s, 0.05 * s, length));
        let rotation = Quat::from_rotation_arc(Vec3::Z, (b - a).normalize());
        commands.spawn((Mesh3d(corridor), MeshMaterial3d(glow(&mut materials, Color::srgb(0.2, 0.55, 0.25), 1.5)), Transform::from_translation(mid).with_rotation(rotation), MapPiece));
    }
}

/// StepCameraCenter: when the team reaches a room, the rig glides over it.
pub fn follow(map: Option<ResMut<TableMap>>, rig: Option<ResMut<super::camera::Rig>>, campaign: Res<Campaign>, mut last: Local<Option<usize>>) {
    let (Some(mut map), Some(mut rig)) = (map, rig) else { return };
    let room = campaign.expedition.as_ref().map(|e| e.room);
    if let Some(focus) = map.focus.take()
        && room != *last
    {
        *last = room;
        rig.target = Some([focus.x, 0.0, focus.z]);
    }
}

pub fn click_rooms(mut clicks: MessageReader<bevy::picking::events::PointerClick>, tiles: Query<&RoomTile>, mut out: MessageWriter<Hud>, drag: Res<super::camera::CameraDrag>) {
    for c in clicks.read() {
        if drag.suppress_click {
            continue;
        }
        if let Ok(tile) = tiles.get(c.entity) {
            out.write(Hud::MoveRoom(tile.0));
        }
    }
}

/// The converted HUD's counters, filled from the campaign: the awakening gauge's level, the blackstone
/// counter (UIMeta).
pub fn fill_counters(
    campaign: Res<Campaign>,
    nodes: Query<&UnityNode>,
    parents: Query<&ChildOf>,
    mut texts: Query<(Entity, &mut Text)>,
) {
    let (Some(ex), Some(save)) = (campaign.expedition.as_ref(), campaign.save.as_ref()) else { return };
    let under = |e: Entity, name: &str| {
        let mut current = Some(e);
        while let Some(c) = current {
            if nodes.get(c).is_ok_and(|n| n.name == name) {
                return true;
            }
            current = parents.get(c).ok().map(ChildOf::parent);
        }
        false
    };
    for (e, mut text) in &mut texts {
        let wanted = if under(e, "UIAwakeGauge") {
            awake_level(ex.awake).to_string()
        } else if under(e, "UIMeta") && text.0.chars().all(|c| c.is_ascii_digit()) && !text.0.is_empty() {
            (save.blackstone + ex.blackstone).to_string()
        } else {
            continue;
        };
        if text.0 != wanted {
            text.0 = wanted;
        }
    }
}

/// Exploration HUD: the status line, the team, and the open event dialog.
pub fn draw_hud(
    mut commands: Commands,
    campaign: Res<Campaign>,
    db: Res<GameDb>,
    loc: Res<Loc>,
    fonts: Res<Fonts>,
    map: Option<Res<TableMap>>,
    mut hud_state: Local<u64>,
    roots: Query<Entity, With<HudRoot>>,
) {
    let Some(map) = map else { return };
    if *hud_state == map.version {
        return;
    }
    *hud_state = map.version;
    for e in &roots {
        commands.entity(e).despawn();
    }
    let Some(ex) = campaign.expedition.as_ref() else { return };
    let ui = Ui { fonts: &fonts };
    let db = &db.0;
    let mission = db.get(ex.mission);
    let mission_name = mission.map(|m| tr(&loc, m.str("missionName"))).unwrap_or_default();

    // Status line under the converted top bar.
    commands
        .spawn((
            Node { position_type: PositionType::Absolute, top: Val::Px(150.0), left: Val::Px(420.0), right: Val::Px(420.0), justify_content: JustifyContent::Center, column_gap: Val::Px(40.0), ..default() },
            GlobalZIndex(4_000),
            bevy::picking::Pickable::IGNORE,
            HudRoot,
        ))
        .with_children(|p| {
            p.spawn(ui.title(mission_name, 34.0, GREEN));
            p.spawn(ui.text(format!("Awakening {} / {} (level {})", ex.awake, crate::rules::explore::AWAKE_MAX, awake_level(ex.awake)), 28.0, if awake_level(ex.awake) > 0 { RED } else { DIM }));
            p.spawn(ui.text(format!("Blackstone +{}", ex.blackstone), 28.0, AMBER));
            p.spawn(ui.text(format!("Cognition {} / {}", ex.cognition, ex.cognition_max), 28.0, Color::srgb(0.4, 0.85, 1.0)));
        });

    // Team, bottom left.
    commands
        .spawn((
            Node { position_type: PositionType::Absolute, bottom: Val::Px(150.0), left: Val::Px(30.0), flex_direction: FlexDirection::Column, row_gap: Val::Px(4.0), ..default() },
            GlobalZIndex(4_000),
            bevy::picking::Pickable::IGNORE,
            HudRoot,
        ))
        .with_children(|p| {
            for u in &ex.units {
                let c = if u.hp <= 0 { RED } else if u.hp < u.hp_max { AMBER } else { GREEN };
                p.spawn(ui.text(format!("{}  {}/{}", tr(&loc, &u.name), u.hp.max(0), u.hp_max), 24.0, c));
            }
        });

    // Leave the tomb (the evacuation button).
    if ex.dialog.is_none() {
        commands
            .spawn((Node { position_type: PositionType::Absolute, top: Val::Px(220.0), right: Val::Px(30.0), ..default() }, GlobalZIndex(4_000), HudRoot))
            .with_children(|p| {
                ui.button(p, Hud::Back, "Evacuate", 26.0, !mission.is_some_and(|m| m.flag("cantEvacuate")));
            });
        if ex.objective_done() {
            return;
        }
        commands.spawn((
            Node { position_type: PositionType::Absolute, bottom: Val::Px(300.0), left: Val::Px(0.0), right: Val::Px(0.0), justify_content: JustifyContent::Center, ..default() },
            GlobalZIndex(4_000),
            bevy::picking::Pickable::IGNORE,
            HudRoot,
            children![ui.text("Select a neighbouring room to move. Rooms with a marker hold an event.", 26.0, DIM)],
        ));
        return;
    }

    // The event dialog (UIExploEvent).
    let open = ex.dialog.as_ref().unwrap();
    let Some(event) = ex.event_record(db, open.room) else { return };
    let node = dialog_by_id(event, open.id);
    let choices = ex.choices(db);
    commands.spawn((hud::panel(Val::Px(980.0), Val::Px(230.0), Val::Px(470.0)), HudRoot)).with_children(|p| {
        p.spawn(ui.title(tr(&loc, event.str("infoboxName")), 44.0, GREEN));
        if let Some(node) = node {
            for d in crate::data::items(node.get("descriptions").map(|v| event.deref(v)).unwrap_or(&serde_json::Value::Null)) {
                let d = event.deref(d);
                let speaker = d.get("character").and_then(|c| db.resolve(event, c)).map(|c| tr(&loc, c.str("name"))).filter(|s| !s.is_empty());
                let text = tr(&loc, d.get("description").and_then(|v| v.as_str()).unwrap_or(""));
                match speaker {
                    Some(s) => p.spawn(ui.text(format!("{s}: {text}"), 26.0, Color::WHITE)),
                    None => p.spawn(ui.text(text, 26.0, Color::srgb(0.85, 0.9, 0.85))),
                };
            }
        }
        for o in &open.outcomes {
            let (line, c) = describe(o, db, &loc);
            if !line.is_empty() {
                p.spawn(ui.text(line, 26.0, c));
            }
        }
        for (i, choice) in choices.iter().enumerate() {
            ui.button(p, Hud::Choose(i), tr(&loc, &choice.name), 28.0, true);
            let desc = tr(&loc, &choice.desc);
            if !desc.is_empty() {
                p.spawn(ui.text(desc, 22.0, DIM));
            }
        }
    });
}

fn describe(o: &Reward, db: &crate::data::Db, loc: &Loc) -> (String, Color) {
    match o {
        Reward::Blackstone(n) => (format!("+{n} Blackstone"), AMBER),
        Reward::Cognition(n) => (format!("{n:+} Cognition"), Color::srgb(0.4, 0.85, 1.0)),
        Reward::Damage { unit, amount } => (format!("{} takes {amount} damage", tr(loc, unit)), RED),
        Reward::DamageGroup(n) => (format!("The team takes {n} damage each"), RED),
        Reward::Heal { unit, amount } => (format!("{} repairs {amount}", tr(loc, unit)), GREEN),
        Reward::HealGroup(n) => (format!("The team repairs {n} each"), GREEN),
        Reward::Awakening(n) => (format!("Necron awakening {n:+}"), RED),
        Reward::AwakeLevel(l) => (format!("The tomb stirs: awakening level {l}"), RED),
        Reward::Blueprint(r) => (format!("Blueprint: {}", db.get(*r).map(|b| b.name().to_string()).unwrap_or_default()), AMBER),
        Reward::Canticle(r) => (format!("Canticle: {}", db.get(*r).map(|c| tr(loc, c.str("canticleName"))).unwrap_or_default()), AMBER),
        Reward::Cartography => ("The tomb's layout is revealed".into(), GREEN),
        Reward::Initiative(1) => ("The Necrons will strike first".into(), RED),
        Reward::Initiative(-1) => ("Your forces will strike first".into(), GREEN),
        Reward::Initiative(_) => (String::new(), DIM),
        Reward::Text(lines) => (lines.iter().map(|l| tr(loc, l)).collect::<Vec<_>>().join("\n"), Color::WHITE),
    }
}

/// Player actions on the exploration screen.
pub fn act(
    mut actions: MessageReader<Hud>,
    db: Res<GameDb>,
    mut campaign: ResMut<Campaign>,
    mut map: Option<ResMut<TableMap>>,
    pack: Res<PackDir>,
    mut next_state: ResMut<NextState<GameState>>,
    mut parity: ResMut<Parity>,
) {
    for action in actions.read() {
        let campaign = &mut *campaign;
        let (Some(ex), Some(save)) = (campaign.expedition.as_mut(), campaign.save.as_mut()) else { return };
        let next = match action {
            Hud::MoveRoom(r) if ex.dialog.is_none() => {
                if *r == ex.room {
                    // Re-open an unfinished event in the current room.
                    if ex.event_record(&db.0, *r).is_some() && !ex.events[*r].ended {
                        ex.open(&db.0, *r);
                    }
                    Next::Dialog
                } else {
                    ex.move_to(&db.0, save, *r)
                }
            }
            Hud::Choose(i) => ex.choose(&db.0, save, *i),
            Hud::Back if ex.dialog.is_none() => {
                // TaskExploGoToArkMechanicus(FAILED): leave with what the team carries.
                finish_mission(campaign, false);
                campaign.store(&pack.0);
                next_state.set(GameState::Ark);
                return;
            }
            _ => continue,
        };
        match next {
            Next::Battle { room_data, template } => {
                let room = ex.dialog.as_ref().map(|d| d.room).unwrap_or(ex.room);
                let picked = ex.battle_room(&db.0, save, room, room_data, template).map(|(r, t)| (r.name().to_string(), t));
                match picked {
                    Some((name, template)) if room_exists(&pack.0, &name) => {
                        let seed = ex.rng.range(0, i32::MAX);
                        let event = ex.tomb.rooms[room].event;
                        let story_event = ex.tomb.rooms[room].objective
                            || db.0.get(ex.mission).is_some_and(|m| crate::rules::tomb::MissionSpec::read(&db.0, m).custom_events.iter().any(|e| Some(e.id) == event));
                        let spawn = crate::rules::battle::SpawnContext { mission: Some(ex.mission), awake_level: awake_level(ex.awake), story_event };
                        match Battle::new(&db.0, &pack.0, &name, template, &ex.units, ex.cognition, seed, &save.settings, &spawn) {
                            Ok(b) => {
                                campaign.battle = Some(b);
                                campaign.battle_room = Some(room);
                                next_state.set(GameState::Battle);
                            }
                            Err(e) => {
                                parity.report(&format!("Battle {name}: {e}"));
                                ex.battle_won(&db.0, save);
                            }
                        }
                    }
                    other => {
                        parity.report(&format!("Battle room {} not converted", other.map(|o| o.0).unwrap_or_default()));
                        ex.battle_won(&db.0, save);
                    }
                }
            }
            Next::Victory => {
                finish_mission(campaign, true);
                next_state.set(GameState::Ark);
            }
            Next::Defeat => {
                finish_mission(campaign, false);
                next_state.set(GameState::Ark);
            }
            _ => {}
        }
        campaign.store(&pack.0);
        if let Some(m) = map.as_mut() {
            m.version += 1;
        }
    }
}

/// TaskExploVictory / TaskExploGoToArkMechanicus: bring the expedition's results back to the Ark.
pub fn finish_mission(campaign: &mut Campaign, success: bool) {
    let (Some(ex), Some(save)) = (campaign.expedition.take(), campaign.save.as_mut()) else { return };
    save.blackstone += ex.blackstone;
    let deployment = save.deployment.take();
    if success {
        save.story.completed_missions.push(ex.mission);
    }
    if !save.story.tutorial_done {
        // The tutorial's team is its own; the campaign's tech-priests start fresh on the Ark.
        save.story.tutorial_done = true;
    } else if let Some(d) = deployment {
        for unit in d.units.iter().filter(|u| u.is_techpriest()) {
            if let Some(t) = save.techpriests.iter_mut().find(|t| t.base == unit.base) {
                t.hp = unit.hp.max(1);
            }
        }
    }
    save.story.overall_necron_countdown += if success { 5 } else { 8 };
    campaign.battle = None;
    campaign.battle_room = None;
    super::ark::set_last_result(save, success, ex.blackstone);
}
