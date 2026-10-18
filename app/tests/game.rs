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

    // Start: the intro is skipped (reported unported) and the tomb's exploration screen comes up.
    let start = find(&mut app, "BtnNewGame").expect("window's New game button");
    app.world_mut().write_message(UnityCall {
        source: start,
        event: UEvent { on: "UiButton".into(), target: "UIWindowNewGame".into(), method: "BtnNewGame".into(), arg: String::new() },
    });
    for _ in 0..6 {
        app.update();
    }
    assert_eq!(*app.world().resource::<State<GameState>>().get(), GameState::Exploration);
    let hud = find(&mut app, "Explo").expect("exploration HUD spawned");
    assert!(app.world().get::<UnityActive>(hud).unwrap().0, "exploration HUD shown");
    let parity = app.world().resource::<flow::Parity>();
    eprintln!("parity so far: {:?}", parity.unported.keys().collect::<Vec<_>>());
}

/// New Game → the intro deploys the tutorial team → the tutorial tomb, played through its events by
/// always taking the first choice; fights are reported (the battle test covers them).
#[test]
fn new_game_reaches_and_plays_the_tutorial_tomb() {
    use cogitator::rules::explore::{Expedition, Next};
    use cogitator::rules::save::{Save, Settings};
    let Some(dir) = pack() else { return };
    let db = cogitator::data::Db::load(&dir).unwrap();
    let mut save = Save::new_game(&db, Settings::default()).unwrap();
    assert!(!save.techpriests.is_empty(), "new game has tech-priests");
    save.deploy_tutorial(&db).unwrap();
    let deployment = save.deployment.as_ref().unwrap();
    assert_eq!(db.get(deployment.mission).unwrap().name(), "TUT1");
    let mut ex = Expedition::begin(&db, &save).unwrap();
    println!("tomb: {} rooms, {} corridors, start {}, seed {}", ex.tomb.rooms.len(), ex.tomb.links.len(), ex.tomb.start, ex.tomb.seed);
    for (i, r) in ex.tomb.rooms.iter().enumerate() {
        println!("  room {i} at {:?} depth {} event {:?}", r.pos, r.depth, r.event.and_then(|e| db.get(e)).map(|e| e.name()));
    }
    let mut fights = 0;
    let mut steps = 0;
    'walk: while steps < 200 {
        steps += 1;
        while ex.dialog.is_some() {
            let choices = ex.choices(&db);
            assert!(!choices.is_empty(), "an open dialog offers choices");
            match ex.choose(&db, &mut save, 0) {
                Next::Battle { .. } => {
                    fights += 1;
                    ex.battle_won(&db, &mut save);
                }
                Next::Victory => break 'walk,
                Next::Defeat => panic!("defeated"),
                _ => {}
            }
        }
        // Head for the nearest room with an unfinished event.
        let target = (0..ex.tomb.rooms.len()).find(|&r| ex.tomb.rooms[r].event.is_some() && !ex.events[r].ended);
        let Some(target) = target else { break };
        let next = step_towards(&ex, target);
        ex.move_to(&db, &save, next);
    }
    println!("tutorial: {fights} fights, awakening {}, blackstone +{}", ex.awake, ex.blackstone);
    assert!(ex.objective_done(), "the tutorial's objective events all end");
}

fn step_towards(ex: &cogitator::rules::explore::Expedition, target: usize) -> usize {
    use std::collections::VecDeque;
    let mut prev = vec![usize::MAX; ex.tomb.rooms.len()];
    let mut queue = VecDeque::from([ex.room]);
    prev[ex.room] = ex.room;
    while let Some(r) = queue.pop_front() {
        if r == target {
            break;
        }
        for n in ex.tomb.neighbours(r) {
            if prev[n] == usize::MAX {
                prev[n] = r;
                queue.push_back(n);
            }
        }
    }
    let mut at = target;
    while prev[at] != ex.room {
        at = prev[at];
    }
    at
}

/// The tutorial team fights the first tutorial room: the player side attacks what it can reach and
/// otherwise closes in; the Necrons run their own AI. Prints the team's skills and the result.
#[test]
fn tutorial_team_fights_the_first_tutorial_battle() {
    use cogitator::rules::battle::{Battle, Kind, Side, SkillSpec, distance};
    use cogitator::rules::save::{Save, Settings};
    let Some(dir) = pack() else { return };
    if !cogitator::rules::battle::room_exists(&dir, "Tuto_Battle_1") {
        eprintln!("skipped: battle rooms not converted");
        return;
    }
    let db = cogitator::data::Db::load(&dir).unwrap();
    let mut save = Save::new_game(&db, Settings::default()).unwrap();
    save.deploy_tutorial(&db).unwrap();
    let team = save.deployment.as_ref().unwrap().units.clone();
    let spawn = cogitator::rules::battle::SpawnContext { mission: save.deployment.as_ref().map(|d| d.mission), awake_level: 0, story_event: true };
    let mut b = Battle::new(&db, &dir, "Tuto_Battle_1", None, &team, 3, 7, &save.settings, &spawn).unwrap();
    for f in &b.fighters {
        let skills: Vec<String> = f.skills.iter().filter_map(|s| db.get(s.skill)).map(|s| {
            let spec = SkillSpec::read(s);
            format!("{} (r{} d{:?} cp{})", spec.name, spec.range_max, spec.damage, spec.cost)
        }).collect();
        println!("{:?} {:?} {} hp{} init{} mv{} at {:?}: {skills:?}", f.side, f.kind, f.name, f.hp, f.initiative, f.move_range, f.cell);
    }
    assert!(b.fighters.iter().any(|f| f.side == Side::Necron && f.kind == Kind::Unit), "the room has Necrons");
    assert!(b.fighters.iter().any(|f| f.side == Side::Mechanicus), "the team deployed");
    let mut guard = 0;
    while b.outcome.is_none() && guard < 2000 {
        guard += 1;
        let Some(who) = b.current() else { break };
        if !b.is_player_turn() {
            b.necron_act(&db);
            continue;
        }
        let attack = |b: &Battle| (0..b.fighters[who].skills.len()).find_map(|s| {
            if b.skill_blocked(&db, who, s).is_some() { return None; }
            b.targets(&db, who, s).into_iter().find(|&t| b.fighters[t].side == Side::Necron).map(|t| (s, t))
        });
        if let Some((s, t)) = attack(&b) {
            b.use_skill(&db, who, s, t);
        } else if let Some(goal) = b.fighters.iter().filter(|f| f.side == Side::Necron && (f.alive() || f.knocked_out.is_some()) && f.kind == Kind::Unit).map(|f| f.cell).next() {
            let me = b.fighters[who].cell;
            let reach = b.reachable(who, b.fighters[who].moves_left());
            if let Some((&cell, _)) = reach.iter().min_by(|x, y| distance(*x.0, goal).total_cmp(&distance(*y.0, goal))) && cell != me {
                b.move_to(who, cell);
            }
            if let Some((s, t)) = attack(&b) {
                b.use_skill(&db, who, s, t);
            }
        }
        b.end_turn();
    }
    println!("outcome {:?} after {} rounds, cognition {}, {} events, AI gaps {:?}", b.outcome, b.round, b.cognition, b.events.len(), b.unported_ai);
    let necron_moves = b.events.iter().filter(|e| matches!(e, cogitator::rules::battle::Event::Moved { who, .. } if b.fighters[*who].side == Side::Necron)).count();
    let necron_hits = b.events.iter().filter(|e| matches!(e, cogitator::rules::battle::Event::Skill { who, .. } if b.fighters[*who].side == Side::Necron)).count();
    println!("necron moves {necron_moves}, necron attacks {necron_hits}");
    assert!(b.outcome.is_some(), "the battle ends");
}

/// A campaign fight: the first Agrolekh room filled from mission C1's random deployment.
#[test]
fn campaign_room_spawns_the_missions_necrons() {
    use cogitator::rules::battle::{Battle, Kind, Side, SpawnContext};
    use cogitator::rules::save::{Save, Settings};
    let Some(dir) = pack() else { return };
    if !cogitator::rules::battle::room_exists(&dir, "Agrolekh_B_01") {
        return;
    }
    let db = cogitator::data::Db::load(&dir).unwrap();
    let save = Save::new_game(&db, Settings::default()).unwrap();
    let mission = db.named("Mission", "C1").map(|m| m.id);
    let spawn = SpawnContext { mission, awake_level: 0, story_event: false };
    let b = Battle::new(&db, &dir, "Agrolekh_B_01", None, &save.techpriests, 2, 11, &save.settings, &spawn).unwrap();
    let necrons: Vec<String> = b.fighters.iter().filter(|f| f.side == Side::Necron && f.kind == Kind::Unit).map(|f| f.name.clone()).collect();
    println!("Agrolekh_B_01 template {}: {necrons:?}", b.template);
    assert!(!necrons.is_empty(), "the mission's deployment spawned Necrons");
}
