//! A battle room: its converted scene (only the chosen template's props shown), the grid overlays, the
//! units, and the battle HUD. Rules live in `rules::battle`; this shows them and takes the player's
//! orders, pacing the Necrons' turns so they can be followed.

use bevy::gltf::{Gltf, GltfExtras};
use bevy::picking::events::PointerClick;
use bevy::prelude::*;

use super::campaign::{Campaign, GameDb};
use super::hud::{self, AMBER, DIM, GREEN, Hud, HudRoot, RED, Ui, tr};
use super::{GameState, Parity};
use crate::loc::Loc;
use crate::rules::battle::{Battle, Cell, Event, Kind, Outcome, Side, SkillSpec};
use crate::rules::explore::Next;
use crate::unity_ui::Fonts;
use crate::{MainCamera, PackDir};

#[derive(Component)]
pub struct BattlePiece;

#[derive(Component)]
pub struct FighterVis(pub usize);

#[derive(Component)]
pub struct FighterLabel(pub usize);

#[derive(Component)]
pub struct CellTile(pub Cell);

#[derive(Component)]
pub struct TargetTile(pub usize);

#[derive(Resource, Default)]
pub struct BattleView {
    selected_skill: Option<usize>,
    events_seen: usize,
    version: u64,
    drawn: u64,
    necron_wait: f32,
    floating: Vec<(usize, String, Color, f32)>,
}

/// The battle room's scene file and whether it has arrived (0 fetching, 1 ready, 2 failed).
#[derive(Resource)]
pub struct RoomScene {
    file: String,
    ready: std::sync::Arc<std::sync::atomic::AtomicU8>,
    spawned: bool,
}

pub fn spawn_room_scene(mut commands: Commands, scene: Option<ResMut<RoomScene>>, assets: Res<AssetServer>, mut parity: ResMut<Parity>) {
    let Some(mut scene) = scene else { return };
    if scene.spawned {
        return;
    }
    match scene.ready.load(std::sync::atomic::Ordering::Acquire) {
        1 if std::env::var_os("COGITATOR_NO_ROOM").is_some() => scene.spawned = true,
        1 => {
            let handle = assets.load(GltfAssetLabel::Scene(0).from_asset(format!("pack://{}", scene.file)));
            commands.spawn((WorldAssetRoot(handle), Transform::default(), BattlePiece));
            scene.spawned = true;
        }
        2 => {
            parity.report(&format!("Battle scene {} unavailable (is the pack server running?)", scene.file));
            scene.spawned = true;
        }
        _ => {}
    }
}

/// Unity world → Bevy world (x mirrored, as the glTF export does).
fn bevy_pos(p: [f32; 3]) -> Vec3 {
    Vec3::new(-p[0], p[1], p[2])
}

pub fn enter(
    mut commands: Commands,
    campaign: Res<Campaign>,
    db: Res<GameDb>,
    ambient: Option<ResMut<GlobalAmbientLight>>,
    assets: Res<AssetServer>,
    pack: Res<PackDir>,
    mut camera: Query<(&mut Transform, &mut Projection), With<MainCamera>>,
    mut parity: ResMut<Parity>,
    mut next: ResMut<NextState<GameState>>,
) {
    commands.insert_resource(BattleView { version: 1, ..default() });
    let Some(b) = campaign.battle.as_ref() else {
        parity.report("Battle without a fight");
        next.set(GameState::Exploration);
        return;
    };
    // The room's scene: synced on demand from the pack server the first time it's needed.
    let file = format!("models/battle/{}.glb", b.room);
    let ready = std::sync::Arc::new(std::sync::atomic::AtomicU8::new(0));
    if pack.0.join(&file).exists() {
        ready.store(1, std::sync::atomic::Ordering::Release);
    } else {
        let (flag, dir, path) = (ready.clone(), pack.0.clone(), file.clone());
        std::thread::spawn(move || {
            let ok = crate::pack::fetch(crate::SERVER, &dir, &path);
            if let Err(e) = &ok {
                warn!("fetching {path}: {e}");
            }
            flag.store(if ok.is_ok() { 1 } else { 2 }, std::sync::atomic::Ordering::Release);
        });
    }
    commands.insert_resource(RoomScene { file, ready, spawned: false });
    let _ = (&mut parity, &assets);
    // The room's CameraContainer rig, centred on the deployment zone (where the battle opens).
    let doc: Option<serde_json::Value> = std::fs::read(pack.0.join("battle").join(format!("{}.json", b.room))).ok().and_then(|x| serde_json::from_slice(&x).ok());
    let mut rig = doc
        .as_ref()
        .and_then(|d| d.get("camera"))
        .and_then(|c| serde_json::from_value::<super::camera::Rig>(c.clone()).ok())
        .unwrap_or(super::camera::Rig {
            position: [0.0; 3],
            pivot_euler: [60.0, 135.0, 0.0],
            zoom: -26.0,
            fov: 30.0,
            zoom_min: -40.0,
            zoom_max: -8.0,
            x_rotation_zoom: 60.0,
            bounds: None,
            target: None,
        });
    // TaskBattleLaunch: StepCameraCenter on the centre of the room's camera bounds.
    if let Some([lo, hi]) = rig.bounds {
        rig.position = [(lo[0] + hi[0]) / 2.0, average_height(b), (lo[2] + hi[2]) / 2.0];
    }
    if let Some(yaw) = base_y_rotation(&db.0, b) {
        rig.pivot_euler[1] = yaw;
    }
    if let Ok((mut t, mut projection)) = camera.single_mut() {
        *t = rig.transform();
        if let Projection::Perspective(p) = projection.as_mut() {
            p.fov = rig.fov.to_radians();
        }
    }
    commands.insert_resource(rig);
    if let Some(mut ambient) = ambient {
        super::scene3d::apply_render(doc.as_ref().and_then(|d| d.get("render")), &mut ambient);
    }
}

/// RoomTemplateData.baseYCameraRotation: some templates turn the camera.
fn base_y_rotation(db: &crate::data::Db, b: &Battle) -> Option<f32> {
    let room = db.named("RoomData", &b.room)?;
    let list = room.get("templates")?;
    let tpl = crate::data::items(room.deref(list)).iter().map(|t| room.deref(t)).find(|t| t.get("template").and_then(|v| v.as_i64()) == Some(b.template))?;
    let value = tpl.get("baseYCameraRotation")?;
    crate::data::items(value).first().and_then(|v| v.as_f64()).map(|v| v as f32)
}

fn average_height(b: &Battle) -> f32 {
    let hs: Vec<f32> = b.grid.heights.iter().flatten().copied().collect();
    if hs.is_empty() { 0.0 } else { hs.iter().sum::<f32>() / hs.len() as f32 }
}

pub fn exit(mut commands: Commands, pieces: Query<Entity, Or<(With<BattlePiece>, With<FighterVis>, With<FighterLabel>, With<CellTile>, With<TargetTile>)>>) {
    for e in &pieces {
        commands.entity(e).despawn();
    }
    commands.remove_resource::<BattleView>();
    commands.remove_resource::<RoomScene>();
    commands.remove_resource::<super::camera::Rig>();
}

/// Show only the chosen template's props; hide what Unity had inactive.
pub fn filter_template(campaign: Res<Campaign>, mut nodes: Query<(&GltfExtras, &mut Visibility), Added<GltfExtras>>) {
    let Some(b) = campaign.battle.as_ref() else { return };
    if std::env::var_os("COGITATOR_NO_FILTER").is_some() {
        return;
    }
    for (extras, mut vis) in &mut nodes {
        let Ok(v) = serde_json::from_str::<serde_json::Value>(&extras.value) else { continue };
        let wrong_template = v.get("template").and_then(|t| t.as_i64()).is_some_and(|t| t != b.template);
        let inactive = v.get("inactive").and_then(|i| i.as_bool()).unwrap_or(false);
        if wrong_template || inactive {
            *vis = Visibility::Hidden;
        }
    }
}

/// Unit tokens, created once and moved towards their fighter's cell every frame.
pub fn sync_fighters(
    mut commands: Commands,
    campaign: Res<Campaign>,
    mut vis: Query<(&FighterVis, &mut Transform, &mut Visibility)>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    time: Res<Time>,
    mut spawned: Local<bool>,
    pack: Res<PackDir>,
    assets: Res<AssetServer>,
) {
    let Some(b) = campaign.battle.as_ref() else {
        *spawned = false;
        return;
    };
    if !*spawned {
        *spawned = true;
        let body = meshes.add(Capsule3d::new(0.32, 0.9));
        let crate_mesh = meshes.add(Cuboid::new(0.7, 0.7, 0.7));
        for (i, f) in b.fighters.iter().enumerate() {
            let color = match (f.side, f.kind) {
                (_, Kind::Archive) => Color::srgb(0.3, 0.9, 1.0),
                (_, Kind::Prop) => Color::srgb(0.4, 0.4, 0.4),
                (Side::Mechanicus, _) => Color::srgb(0.75, 0.1, 0.08),
                (Side::Necron, _) => Color::srgb(0.25, 1.0, 0.35),
            };
            // Archives and scenery are part of the converted room; their token stays only as a pick target.
            let material = if f.kind == Kind::Unit {
                materials.add(StandardMaterial { base_color: color, emissive: LinearRgba::from(color) * 0.6, ..default() })
            } else {
                materials.add(StandardMaterial { base_color: Color::NONE, alpha_mode: AlphaMode::Blend, unlit: true, ..default() })
            };
            let pos = bevy_pos(b.grid.world(f.cell));
            let mesh = if f.kind == Kind::Unit { body.clone() } else { crate_mesh.clone() };
            let mut e = commands.spawn((
                Mesh3d(mesh),
                MeshMaterial3d(material),
                Transform::from_translation(pos + Vec3::Y * 0.8),
                FighterVis(i),
                BattlePiece,
            ));
            // The unit's own model when the converter has exported it (tech-priests share the
            // customisable body); the token stays as its pickable stand-in, made invisible.
            if let Some(file) = unit_model(&pack.0, f).filter(|_| std::env::var_os("COGITATOR_NO_UNITS").is_none()) {
                let gltf: Handle<Gltf> = assets.load(format!("pack://{file}"));
                let scene = assets.load(GltfAssetLabel::Scene(0).from_asset(format!("pack://{file}")));
                e.with_children(|c| {
                    c.spawn((
                        WorldAssetRoot(scene),
                        UnitModel(gltf),
                        Transform::from_xyz(0.0, -0.8, 0.0).with_rotation(Quat::from_rotation_y((180.0 - f.yaw).to_radians())),
                    ));
                });
                e.insert(TokenHidden);
            }
            commands.spawn((Node { position_type: PositionType::Absolute, ..default() }, FighterLabel(i), GlobalZIndex(3_000), bevy::picking::Pickable::IGNORE, BattlePiece));
        }
    }
    let step = (time.delta_secs() * 6.0).min(1.0);
    for (fv, mut t, mut visibility) in &mut vis {
        let f = &b.fighters[fv.0];
        let target = bevy_pos(b.grid.world(f.cell)) + Vec3::Y * if f.knocked_out.is_some() { 0.3 } else { 0.8 };
        t.translation = t.translation.lerp(target, step);
        t.rotation = if f.knocked_out.is_some() { Quat::from_rotation_z(1.4) } else { Quat::IDENTITY };
        *visibility = if f.dead || (f.kind == Kind::Archive && f.collected) { Visibility::Hidden } else { Visibility::Inherited };
    }
}

fn unit_model(pack: &std::path::Path, f: &crate::rules::battle::Fighter) -> Option<String> {
    let file = if f.techpriest { "models/units/techpriest.glb".to_string() } else { format!("models/units/{}_{}.glb", f.prefab?.file, f.prefab?.path_id) };
    pack.join(&file).exists().then_some(file)
}

/// A unit's glTF, so its animation player can be given the idle clip.
#[derive(Component)]
pub struct UnitModel(pub Handle<Gltf>);

/// A fighter whose model replaced its token mesh.
#[derive(Component)]
pub struct TokenHidden;

/// Make token meshes under loaded models invisible but still pickable.
pub fn hide_tokens(mut tokens: Query<&mut MeshMaterial3d<StandardMaterial>, Added<TokenHidden>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    for mut m in &mut tokens {
        m.0 = materials.add(StandardMaterial { base_color: Color::NONE, alpha_mode: AlphaMode::Blend, unlit: true, ..default() });
    }
}

/// Play each unit model's idle clip (the Animator's default state) on loop.
pub fn animate_units(
    mut commands: Commands,
    mut players: Query<(Entity, &mut AnimationPlayer), Added<AnimationPlayer>>,
    parents: Query<&ChildOf>,
    models: Query<&UnitModel>,
    gltfs: Res<Assets<Gltf>>,
    mut graphs: ResMut<Assets<AnimationGraph>>,
) {
    for (entity, mut player) in &mut players {
        let mut current = Some(entity);
        let mut model = None;
        while let Some(e) = current {
            if let Ok(m) = models.get(e) {
                model = Some(m);
                break;
            }
            current = parents.get(e).ok().map(ChildOf::parent);
        }
        let Some(gltf) = model.and_then(|m| gltfs.get(&m.0)) else { continue };
        let clip = gltf
            .named_animations
            .iter()
            .find(|(name, _)| name.to_lowercase().starts_with("idle"))
            .or_else(|| gltf.named_animations.iter().find(|(name, _)| name.to_lowercase().contains("idle")))
            .map(|(_, h)| h.clone());
        let Some(clip) = clip else { continue };
        let (graph, node) = AnimationGraph::from_clip(clip);
        commands.entity(entity).insert(AnimationGraphHandle(graphs.add(graph)));
        player.play(node).repeat();
    }
}

/// Floating names and health over each unit (screen-space labels following their 3D positions).
pub fn place_labels(
    campaign: Res<Campaign>,
    view: Option<Res<BattleView>>,
    camera: Query<(&Camera, &GlobalTransform), With<MainCamera>>,
    vis: Query<(&FighterVis, &GlobalTransform)>,
    mut labels: Query<(Entity, &FighterLabel, &mut Node)>,
    mut commands: Commands,
    loc: Res<Loc>,
    fonts: Res<Fonts>,
    ui_scale: Res<UiScale>,
    mut texts: Local<std::collections::HashMap<usize, String>>,
) {
    let (Some(b), Some(view)) = (campaign.battle.as_ref(), view) else { return };
    let Ok((cam, cam_t)) = camera.single() else { return };
    let ui = Ui { fonts: &fonts };
    let positions: std::collections::HashMap<usize, Vec3> = vis.iter().map(|(v, t)| (v.0, t.translation())).collect();
    for (entity, label, mut node) in &mut labels {
        let f = &b.fighters[label.0];
        let Some(&world) = positions.get(&label.0) else { continue };
        let shown = !(f.dead || (f.kind == Kind::Archive && f.collected) || f.kind == Kind::Prop);
        let Ok(screen) = cam.world_to_viewport(cam_t, world + Vec3::Y * 1.2) else { continue };
        node.left = Val::Px(screen.x / ui_scale.0 - 60.0);
        node.top = Val::Px(screen.y / ui_scale.0 - 40.0);
        node.display = if shown { Display::Flex } else { Display::None };
        let floating: Vec<String> = view.floating.iter().filter(|x| x.0 == label.0).map(|x| x.1.clone()).collect();
        let text = match f.kind {
            Kind::Archive => "Archive".to_string(),
            _ if f.knocked_out.is_some() => format!("{} (reanimating {})", tr(&loc, &f.name), f.knocked_out.unwrap()),
            _ => format!("{} {}/{}", tr(&loc, &f.name), f.hp, f.hp_max),
        } + &floating.iter().map(|s| format!("\n{s}")).collect::<String>();
        if texts.get(&label.0) != Some(&text) {
            texts.insert(label.0, text.clone());
            let color = if f.side == Side::Mechanicus { Color::srgb(1.0, 0.75, 0.7) } else { GREEN };
            commands.entity(entity).despawn_related::<Children>().with_children(|c| {
                c.spawn((ui.text(text, 20.0, color), bevy::picking::Pickable::IGNORE));
            });
        }
    }
}

/// Grid overlays for the unit whose turn it is: where it can move (green free, amber costs cognition)
/// and, with a skill selected, whom it can hit.
pub fn draw_overlays(
    mut commands: Commands,
    campaign: Res<Campaign>,
    db: Res<GameDb>,
    view: Option<ResMut<BattleView>>,
    old: Query<Entity, Or<(With<CellTile>, With<TargetTile>)>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    vis: Query<(&FighterVis, &Transform)>,
) {
    let (Some(b), Some(mut view)) = (campaign.battle.as_ref(), view) else { return };
    if view.drawn == view.version {
        return;
    }
    view.drawn = view.version;
    for e in &old {
        commands.entity(e).despawn();
    }
    let Some(who) = b.current() else { return };
    if !b.is_player_turn() || b.outcome.is_some() {
        return;
    }
    let quad = meshes.add(Cuboid::new(0.9, 0.04, 0.9));
    let glow = |m: &mut Assets<StandardMaterial>, c: Color| m.add(StandardMaterial { base_color: c.with_alpha(0.45), emissive: LinearRgba::from(c), unlit: true, alpha_mode: AlphaMode::Blend, ..default() });
    let free_mat = glow(&mut materials, GREEN);
    let cost_mat = glow(&mut materials, AMBER);
    let target_mat = glow(&mut materials, RED);
    match view.selected_skill {
        None => {
            let f = &b.fighters[who];
            for (cell, _) in b.reachable(who, b.move_budget(who)) {
                if cell == f.cell {
                    continue;
                }
                let Some(price) = b.move_cost(who, cell) else { continue };
                let mat = if price == 0 { free_mat.clone() } else { cost_mat.clone() };
                commands.spawn((Mesh3d(quad.clone()), MeshMaterial3d(mat), Transform::from_translation(bevy_pos(b.grid.world(cell)) + Vec3::Y * 0.03), CellTile(cell), BattlePiece));
            }
        }
        Some(s) => {
            let ring = meshes.add(Cylinder::new(0.6, 0.05));
            for t in b.targets(&db.0, who, s) {
                let at = vis.iter().find(|(v, _)| v.0 == t).map(|(_, tr)| tr.translation - Vec3::Y * 0.75).unwrap_or(bevy_pos(b.grid.world(b.fighters[t].cell)));
                commands.spawn((Mesh3d(ring.clone()), MeshMaterial3d(target_mat.clone()), Transform::from_translation(at), TargetTile(t), BattlePiece));
            }
        }
    }
}

pub fn clicks(mut clicks: MessageReader<PointerClick>, cells: Query<&CellTile>, targets: Query<&TargetTile>, fighters: Query<&FighterVis>, mut out: MessageWriter<Hud>, drag: Res<super::camera::CameraDrag>) {
    for c in clicks.read() {
        if drag.suppress_click {
            continue;
        }
        if let Ok(cell) = cells.get(c.entity) {
            out.write(Hud::MoveCell(cell.0.0, cell.0.1));
        } else if let Ok(t) = targets.get(c.entity) {
            out.write(Hud::Target(t.0));
        } else if let Ok(f) = fighters.get(c.entity) {
            out.write(Hud::Target(f.0));
        }
    }
}

pub fn act(
    mut actions: MessageReader<Hud>,
    db: Res<GameDb>,
    mut campaign: ResMut<Campaign>,
    view: Option<ResMut<BattleView>>,
    mut next: ResMut<NextState<GameState>>,
    pack: Res<PackDir>,
) {
    let Some(mut view) = view else { return };
    for action in actions.read() {
        let campaign = &mut *campaign;
        let Some(b) = campaign.battle.as_mut() else { return };
        let Some(who) = b.current() else { continue };
        match action {
            Hud::MoveCell(i, j) if b.is_player_turn() => {
                b.move_to(who, (*i, *j));
                view.selected_skill = None;
            }
            Hud::Skill(s) if b.is_player_turn() => {
                view.selected_skill = if view.selected_skill == Some(*s) { None } else { Some(*s) };
            }
            Hud::Target(t) if b.is_player_turn() => {
                if let Some(s) = view.selected_skill
                    && b.use_skill(&db.0, who, s, *t)
                {
                    view.selected_skill = None;
                }
            }
            Hud::EndTurn if b.is_player_turn() => {
                view.selected_skill = None;
                b.end_turn();
            }
            Hud::Continue if b.outcome.is_some() => {
                let won = b.outcome == Some(Outcome::Victory);
                let battle = campaign.battle.take().unwrap();
                let (Some(ex), Some(save)) = (campaign.expedition.as_mut(), campaign.save.as_mut()) else { return };
                // The team comes back with its wounds and the cognition it holds.
                for f in battle.fighters.iter().filter(|f| f.side == Side::Mechanicus) {
                    if let Some(u) = f.unit.and_then(|i| ex.units.get_mut(i)) {
                        u.hp = if f.dead || f.knocked_out.is_some() { 0 } else { f.hp };
                    }
                }
                ex.cognition = battle.cognition.min(ex.cognition_max);
                if won {
                    match ex.battle_won(&db.0, save) {
                        Next::Victory => {
                            super::exploration::finish_mission(campaign, true);
                            next.set(GameState::Ark);
                        }
                        _ => next.set(GameState::Exploration),
                    }
                } else {
                    super::exploration::finish_mission(campaign, false);
                    next.set(GameState::Ark);
                }
                campaign.store(&pack.0);
                return;
            }
            _ => continue,
        }
        view.version += 1;
    }
}

/// The Necrons act one unit at a time, a beat apart; event feedback floats over units.
pub fn pace(db: Res<GameDb>, mut campaign: ResMut<Campaign>, view: Option<ResMut<BattleView>>, time: Res<Time>) {
    let (Some(b), Some(mut view)) = (campaign.battle.as_mut(), view) else { return };
    let dt = time.delta_secs();
    view.floating.retain_mut(|x| {
        x.3 -= dt;
        x.3 > 0.0
    });
    if b.events.len() > view.events_seen {
        for e in &b.events[view.events_seen..] {
            match e {
                Event::Hit { who, damage, critical } => view.floating.push((*who, if *critical { format!("-{damage} CRITICAL") } else { format!("-{damage}") }, RED, 2.0)),
                Event::Dodge { who } => view.floating.push((*who, "Dodged".into(), DIM, 2.0)),
                Event::KnockedOut(w) => view.floating.push((*w, "Reanimation protocols".into(), AMBER, 2.5)),
                Event::Revived(w) => view.floating.push((*w, "Reanimated".into(), RED, 2.5)),
                Event::Collected { who, cognition, .. } => view.floating.push((*who, format!("+{cognition} Cognition"), Color::srgb(0.4, 0.85, 1.0), 2.5)),
                _ => {}
            }
        }
        view.events_seen = b.events.len();
        view.version += 1;
    }
    if b.outcome.is_some() || b.is_player_turn() {
        return;
    }
    view.necron_wait += dt;
    if view.necron_wait >= 0.8 {
        view.necron_wait = 0.0;
        b.necron_act(&db.0);
        view.version += 1;
    }
}

pub fn draw_hud(
    mut commands: Commands,
    campaign: Res<Campaign>,
    db: Res<GameDb>,
    loc: Res<Loc>,
    fonts: Res<Fonts>,
    view: Option<Res<BattleView>>,
    roots: Query<Entity, With<HudRoot>>,
    mut drawn: Local<u64>,
) {
    let (Some(b), Some(view)) = (campaign.battle.as_ref(), view) else { return };
    if *drawn == view.version {
        return;
    }
    *drawn = view.version;
    for e in &roots {
        commands.entity(e).despawn();
    }
    let ui = Ui { fonts: &fonts };
    let db = &db.0;
    // Top: round, cognition, turn order.
    commands
        .spawn((
            Node { position_type: PositionType::Absolute, top: Val::Px(20.0), left: Val::Px(20.0), right: Val::Px(20.0), column_gap: Val::Px(14.0), flex_wrap: FlexWrap::Wrap, align_items: AlignItems::Center, ..default() },
            GlobalZIndex(4_000),
            bevy::picking::Pickable::IGNORE,
            HudRoot,
        ))
        .with_children(|p| {
            p.spawn(ui.title(format!("Round {}", b.round), 34.0, GREEN));
            p.spawn(ui.text(format!("Cognition {} / {}", b.cognition, b.cognition_max), 28.0, Color::srgb(0.4, 0.85, 1.0)));
            for (n, &i) in b.order.iter().enumerate() {
                let f = &b.fighters[i];
                if !f.alive() {
                    continue;
                }
                let c = if n == b.turn { Color::WHITE } else if f.side == Side::Mechanicus { Color::srgb(0.9, 0.5, 0.45) } else { Color::srgb(0.4, 0.8, 0.45) };
                p.spawn(ui.text(tr(&loc, &f.name), 22.0, c));
            }
        });
    if let Some(outcome) = b.outcome {
        commands.spawn((hud::panel(Val::Px(700.0), Val::Px(380.0), Val::Px(610.0)), HudRoot)).with_children(|p| {
            let (title, c) = if outcome == Outcome::Victory { ("Victory", GREEN) } else { ("Defeat", RED) };
            p.spawn(ui.title(title, 64.0, c));
            ui.button(p, Hud::Continue, "Continue", 32.0, true);
        });
        return;
    }
    let Some(who) = b.current() else { return };
    let f = &b.fighters[who];
    if !b.is_player_turn() {
        commands.spawn((
            Node { position_type: PositionType::Absolute, bottom: Val::Px(40.0), left: Val::Px(0.0), right: Val::Px(0.0), justify_content: JustifyContent::Center, ..default() },
            GlobalZIndex(4_000),
            HudRoot,
            children![ui.title(format!("{} acts", tr(&loc, &f.name)), 36.0, GREEN)],
        ));
        return;
    }
    commands
        .spawn((
            Node { position_type: PositionType::Absolute, bottom: Val::Px(20.0), left: Val::Px(20.0), right: Val::Px(20.0), flex_direction: FlexDirection::Column, align_items: AlignItems::Center, ..default() },
            GlobalZIndex(4_000),
            HudRoot,
        ))
        .with_children(|p| {
            p.spawn(ui.title(format!("{}  {}/{}  moves {:.0}", tr(&loc, &f.name), f.hp, f.hp_max, f.moves_left()), 32.0, Color::WHITE));
            let hint = match view.selected_skill {
                Some(_) => "Select a target (red), or the skill again to cancel.",
                None => "Select a cell to move (amber costs cognition), a skill to attack, or end the turn.",
            };
            p.spawn(ui.text(hint, 22.0, DIM));
            p.spawn(Node { flex_wrap: FlexWrap::Wrap, justify_content: JustifyContent::Center, ..default() }).with_children(|bar| {
                for (s, state) in f.skills.iter().enumerate() {
                    let Some(skill) = db.get(state.skill) else { continue };
                    let spec = SkillSpec::read(skill);
                    if spec.damage.is_none() && spec.heal.is_none() {
                        continue;
                    }
                    let blocked = b.skill_blocked(db, who, s);
                    let mut label = skill_name(skill, &loc);
                    if let Some((lo, hi)) = spec.damage {
                        label += &format!("  {lo}-{hi}");
                    }
                    if spec.cost > 0 {
                        label += &format!("  {}cp", spec.cost);
                    }
                    if let Some(why) = blocked {
                        label += &format!(" ({why})");
                    }
                    if view.selected_skill == Some(s) {
                        label = format!("> {label} <");
                    }
                    ui.button(bar, Hud::Skill(s), label, 24.0, blocked.is_none() && !b.targets(db, who, s).is_empty());
                }
                ui.button(bar, Hud::EndTurn, "End turn", 26.0, true);
            });
        });
}

fn skill_name(skill: &crate::data::Record, loc: &Loc) -> String {
    let term = skill
        .get("description")
        .map(|d| skill.deref(d))
        .and_then(|d| d.get("skillName").or_else(|| d.get("name")))
        .and_then(|v| v.as_str())
        .unwrap_or("");
    let name = tr(loc, term);
    if name.is_empty() { skill.name().to_string() } else { name }
}
