//! A battle: the room's grid (as the game scans it), the units on it, and turns in initiative order.
//! Skills are read from their data: range and area from `SkillBehaviourTarget`, damage values, cognition
//! cost, cooldown and uses per turn; hits follow HitInfoBasicAttack (dodge, critical = maximum damage,
//! armour pools absorbing by damage type). Necrons with reanimation protocols get back up unless their
//! body is destroyed first.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BinaryHeap};
use std::path::Path;

use super::rng::Rng;
use super::save::Unit;
use crate::data::{Db, Record, Ref, as_bool, as_int, items, odin_type};

/// Mechanicus.Dungeon.Data.Grid: nodes are 1 unit; units climb at most 0.6 between neighbours.
const MAX_CLIMB: f32 = 0.6;
const DIAGONAL: f32 = std::f32::consts::SQRT_2;
/// Turns a knocked-out Necron waits before reanimation protocols bring it back.
const REVIVE_TURNS: i32 = 2;

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Grid {
    pub width: i32,
    pub depth: i32,
    /// Unity world (x, z) of the grid's corner.
    pub origin: [f32; 2],
    pub heights: Vec<Option<f32>>,
    /// 0 no ground, 1 walkable, 2 hole, 3 blocked.
    pub kinds: Vec<u8>,
}

pub type Cell = (i32, i32);

impl Grid {
    pub fn index(&self, (i, j): Cell) -> Option<usize> {
        (i >= 0 && j >= 0 && i < self.width && j < self.depth).then(|| (j * self.width + i) as usize)
    }

    pub fn walkable(&self, c: Cell) -> bool {
        self.index(c).is_some_and(|k| self.kinds[k] == 1)
    }

    pub fn height(&self, c: Cell) -> f32 {
        self.index(c).and_then(|k| self.heights[k]).unwrap_or(0.0)
    }

    /// Unity world position of a node's centre.
    pub fn world(&self, (i, j): Cell) -> [f32; 3] {
        [self.origin[0] + i as f32 + 0.5, self.height((i, j)), self.origin[1] + j as f32 + 0.5]
    }

    pub fn cell_at(&self, x: f32, z: f32) -> Cell {
        ((x - self.origin[0]).floor() as i32, (z - self.origin[1]).floor() as i32)
    }

    /// 8-neighbour steps without cutting corners, within climbing height.
    fn steps(&self, c: Cell) -> impl Iterator<Item = (Cell, f32)> + '_ {
        const DIRS: [(i32, i32); 8] = [(1, 0), (-1, 0), (0, 1), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)];
        DIRS.iter().filter_map(move |&(dx, dy)| {
            let n = (c.0 + dx, c.1 + dy);
            if !self.walkable(n) || (self.height(n) - self.height(c)).abs() > MAX_CLIMB {
                return None;
            }
            if dx != 0 && dy != 0 && (!self.walkable((c.0 + dx, c.1)) || !self.walkable((c.0, c.1 + dy))) {
                return None;
            }
            // BFS.GetDistance: straight-line distance between node centres, height included.
            let (flat, rise) = (if dx != 0 && dy != 0 { DIAGONAL } else { 1.0 }, self.height(n) - self.height(c));
            Some((n, (flat * flat + rise * rise).sqrt()))
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Side {
    Mechanicus,
    Necron,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Kind {
    Unit,
    /// A Necron archive / tactical point: walk next to it and collect cognition.
    Archive,
    /// Scenery entity that occupies its node.
    Prop,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillState {
    pub skill: Ref,
    pub cooldown: i32,
    pub used: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Fighter {
    pub name: String,
    pub side: Side,
    pub kind: Kind,
    /// The entity's data record (…DataBase).
    pub base: Option<Ref>,
    /// The unit's `prefabEntity`, for presentation.
    pub prefab: Option<Ref>,
    /// Index into the expedition's units, for Mechanicus fighters.
    pub unit: Option<usize>,
    pub cell: Cell,
    pub yaw: f32,
    pub hp: i32,
    pub hp_max: i32,
    pub armor_physical: i32,
    pub armor_energetic: i32,
    pub dodge: f32,
    pub critical: f32,
    pub initiative: i32,
    pub move_range: f32,
    /// Distance used of the current movement range (the move skill's nbUse).
    pub moved: f32,
    /// Extra movement ranges bought this turn (nbUseInTurn).
    pub move_uses: i32,
    pub skills: Vec<SkillState>,
    pub knocked_out: Option<i32>,
    pub reviveable: bool,
    pub dead: bool,
    pub collected: bool,
    pub cognition_collect: i32,
    pub techpriest: bool,
    /// NodeControllable.CanDoAction: AI units may be capped at a number of actions per turn (-1 = none).
    pub ai_actions_max: i32,
    pub ai_actions: i32,
}

impl Fighter {
    pub fn alive(&self) -> bool {
        !self.dead && self.knocked_out.is_none()
    }

    /// What remains of the current movement range (GetRealFirstMoveRange).
    pub fn moves_left(&self) -> f32 {
        (self.move_range - self.moved).max(0.0)
    }
}

/// What happened, for the presentation to animate and the HUD to report.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    TurnOf(usize),
    Moved { who: usize, path: Vec<Cell> },
    Skill { who: usize, skill: Ref, target: Cell },
    Hit { who: usize, damage: i32, critical: bool },
    Dodge { who: usize },
    KnockedOut(usize),
    Destroyed(usize),
    Revived(usize),
    Collected { who: usize, archive: usize, cognition: i32 },
    Round(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Outcome {
    Victory,
    Defeat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Battle {
    pub room: String,
    pub template: i64,
    pub grid: Grid,
    pub fighters: Vec<Fighter>,
    /// Fighter indices in initiative order for this round.
    pub order: Vec<usize>,
    pub turn: usize,
    pub round: u32,
    pub cognition: i32,
    pub cognition_max: i32,
    /// Extra movement a tech-priest can buy with cognition (Settings.extra_movement per point).
    pub extra_movement: i32,
    pub deploy_cells: Vec<Cell>,
    pub events: Vec<Event>,
    pub outcome: Option<Outcome>,
    pub rng: Rng,
    /// Behaviour-tree leaves met that this port doesn't run yet (reported as parity gaps).
    #[serde(default)]
    pub unported_ai: std::collections::BTreeSet<String>,
}

#[derive(Deserialize)]
struct RoomDoc {
    room: String,
    level: String,
    templates: Vec<TemplateDoc>,
    #[serde(default)]
    shared_entities: Vec<EntityDoc>,
}

#[derive(Deserialize)]
struct TemplateDoc {
    template: i64,
    grid: Grid,
    entities: Vec<EntityDoc>,
    spawns: Vec<SpawnDoc>,
    #[serde(default)]
    enemy_spawns: Vec<EnemySpawnDoc>,
}

#[derive(Deserialize)]
struct EnemySpawnDoc {
    pos: [f32; 3],
    #[serde(default)]
    yaw: f32,
    container: Option<i64>,
}

/// What TaskBattleSpawnEnemies needs from the tomb: the mission (for its RandomSpawnConfig), the tomb's
/// awakening level, and whether this fight is one of the mission's own events (12 points, not 8).
pub struct SpawnContext {
    pub mission: Option<Ref>,
    pub awake_level: i32,
    pub story_event: bool,
}

#[derive(Deserialize, Clone)]
struct EntityDoc {
    class: String,
    data: i64,
    pos: [f32; 3],
    #[serde(default)]
    yaw: f32,
}

#[derive(Deserialize)]
struct SpawnDoc {
    start: i64,
    reinforcement: bool,
    cells: Vec<[f32; 2]>,
}

/// The rooms a pack has, by RoomData name.
pub fn room_exists(pack: &Path, room: &str) -> bool {
    pack.join("battle").join(format!("{room}.json")).exists()
}

impl Battle {
    /// TaskBattleLaunch: the room's chosen template, its Necrons and archives, and the team deployed.
    pub fn new(db: &Db, pack: &Path, room: &str, template: Option<i64>, team: &[Unit], cognition: i32, seed: i32, settings: &super::save::Settings, spawn: &SpawnContext) -> Result<Battle, String> {
        let file = pack.join("battle").join(format!("{room}.json"));
        let doc: RoomDoc = serde_json::from_slice(&std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?)
            .map_err(|e| format!("{}: {e}", file.display()))?;
        let tpl = template
            .and_then(|t| doc.templates.iter().position(|d| d.template == t))
            .unwrap_or(0);
        let tdoc = doc.templates.get(tpl).ok_or_else(|| format!("{room}: no templates"))?;
        let level = &doc.level;
        let mut battle = Battle {
            room: doc.room.clone(),
            template: tdoc.template,
            grid: tdoc.grid.clone(),
            fighters: Vec::new(),
            order: Vec::new(),
            turn: 0,
            round: 0,
            cognition,
            cognition_max: db.single("GameplayConfig").map(|g| g.int("baseCognitionGaugeSize") as i32).unwrap_or(4) * 3,
            extra_movement: settings.extra_movement,
            deploy_cells: Vec::new(),
            events: Vec::new(),
            outcome: None,
            rng: Rng::new(seed),
            unported_ai: Default::default(),
        };
        let necron_life = settings.necron_life_modifier() as f32 / 100.0;
        for e in tdoc.entities.iter().chain(&doc.shared_entities) {
            if let Some(f) = battle.entity(db, level, e, necron_life) {
                battle.fighters.push(f);
            }
        }
        let room_record = db.named("RoomData", room);
        let army_bonus = room_record
            .and_then(|r| {
                let list = r.get("templates")?;
                items(r.deref(list)).iter().map(|t| r.deref(t)).find(|t| t.get("template").map(as_int) == Some(tdoc.template)).cloned()
            })
            .filter(|t| t.get("armyCapacityConstraint").map(as_bool).unwrap_or(false))
            .and_then(|t| t.get("armyCapacityConstraintValue").map(as_int))
            .unwrap_or(0) as i32;
        battle.spawn_enemies(db, tdoc, spawn, settings, army_bonus, necron_life);
        let mut spawn: Vec<Cell> = tdoc
            .spawns
            .iter()
            .filter(|s| !s.reinforcement && s.start == 0)
            .flat_map(|s| s.cells.iter().map(|c| battle.grid.cell_at(c[0], c[1])))
            .filter(|&c| battle.grid.walkable(c))
            .collect();
        spawn.sort();
        spawn.dedup();
        if spawn.is_empty() {
            spawn = (0..battle.grid.width).flat_map(|i| (0..battle.grid.depth).map(move |j| (i, j))).filter(|&c| battle.grid.walkable(c)).take(16).collect();
        }
        battle.deploy_cells = spawn.clone();
        let free_cells: Vec<Cell> = spawn.into_iter().filter(|&c| battle.occupant(c).is_none()).collect();
        let mut free = free_cells.into_iter();
        for (i, unit) in team.iter().enumerate().filter(|(_, u)| u.hp > 0) {
            let Some(cell) = free.next() else { break };
            let base = db.get(unit.base);
            let mut f = fighter_from(db, base, Side::Mechanicus, cell, 0.0);
            f.name = unit.name.clone();
            f.hp = unit.hp;
            f.hp_max = unit.hp_max;
            f.unit = Some(i);
            f.skills = unit_skills(db, unit).into_iter().map(|skill| SkillState { skill, cooldown: 0, used: 0 }).collect();
            battle.fighters.push(f);
        }
        battle.new_round();
        battle.check_end();
        Ok(battle)
    }

    fn entity(&self, db: &Db, level: &str, e: &EntityDoc, necron_life: f32) -> Option<Fighter> {
        let mb = db.get(Ref { file: crate::data::file_id(level), path_id: e.data });
        let base = mb.and_then(|mb| {
            let entity = mb.get("entity").map(|v| mb.deref(v))?;
            db.resolve(mb, entity.get("refData")?)
        });
        let cell = self.grid.cell_at(e.pos[0], e.pos[2]);
        let kind = entity_kind(&e.class);
        let mut f = fighter_from(db, base, Side::Necron, cell, e.yaw);
        f.kind = kind;
        if kind == Kind::Unit {
            f.hp_max = ((f.hp_max as f32) * (1.0 + necron_life)).round().max(1.0) as i32;
            f.hp = f.hp_max;
        }
        if f.name.is_empty() {
            f.name = e.class.trim_end_matches("DataMb").to_string();
        }
        Some(f)
    }

    /// TaskBattleSpawnEnemies: the mission's random deployment fills the template's spawn markers, the
    /// default group from the battle's capacity, each container's group from its own.
    fn spawn_enemies(&mut self, db: &Db, tdoc: &TemplateDoc, ctx: &SpawnContext, settings: &super::save::Settings, army_bonus: i32, necron_life: f32) {
        let Some(mission) = ctx.mission.and_then(|m| db.get(m)) else { return };
        let config = db.follow(mission, "randomSpawnConfig").or_else(|| db.single("DungeonLibrary").and_then(|l| db.follow(l, "defaultRandomSpawnConfig")));
        let Some(config) = config else { return };
        let deployments = db.follow_all(config, "deployments");
        let Some(&deployment) = self.rng.pick(&deployments) else { return };
        let entries: Vec<Value> = deployment.get("entitiesData").map(|l| items(deployment.deref(l)).iter().map(|e| deployment.deref(e).clone()).collect()).unwrap_or_default();
        let extra = settings.extra_necron_modifier();
        // BattleGameplayConfig.GetBattleDeploymentCapacity / ...InContainer.
        let mut capacity = if ctx.story_event { 12 } else { 8 } + army_bonus;
        if ctx.awake_level >= 2 {
            capacity += 1;
        }
        if ctx.awake_level >= 4 {
            capacity += 1;
        }
        capacity += (capacity * extra) / 100;
        let container_capacity = 6 + (6 * extra) / 100;
        let mut groups: BTreeMap<Option<i64>, Vec<&EnemySpawnDoc>> = BTreeMap::new();
        for s in &tdoc.enemy_spawns {
            groups.entry(s.container).or_default().push(s);
        }
        let mut spawned: Vec<usize> = Vec::new();
        for (container, mut markers) in groups {
            let mut points = if container.is_none() { capacity } else { container_capacity };
            self.rng.shuffle(&mut markers);
            while !markers.is_empty() {
                let Some(pick) = pick_entity(&entries, points, &spawned, &mut self.rng) else { break };
                spawned.push(pick);
                points -= entries[pick].get("point").map(as_int).unwrap_or(1) as i32;
                let marker = markers.remove(0);
                let entity = entries[pick].get("entity").cloned().unwrap_or(Value::Null);
                let base = entity.get("refData").and_then(|r| db.resolve(deployment, r));
                let cell = self.grid.cell_at(marker.pos[0], marker.pos[2]);
                let mut f = fighter_from(db, base, Side::Necron, cell, marker.yaw);
                f.hp_max = ((f.hp_max as f32) * (1.0 + necron_life)).round().max(1.0) as i32;
                f.hp = f.hp_max;
                if f.name.is_empty() {
                    f.name = crate::data::odin_type(&entity).to_string();
                }
                self.fighters.push(f);
            }
        }
    }

    pub fn occupant(&self, c: Cell) -> Option<usize> {
        self.fighters.iter().position(|f| f.cell == c && !f.dead && (f.kind != Kind::Archive || !f.collected))
    }

    pub fn current(&self) -> Option<usize> {
        self.order.get(self.turn).copied()
    }

    pub fn is_player_turn(&self) -> bool {
        self.current().is_some_and(|i| self.fighters[i].side == Side::Mechanicus)
    }

    fn new_round(&mut self) {
        self.round += 1;
        self.events.push(Event::Round(self.round));
        // Reanimation protocols tick at the top of the round.
        for i in 0..self.fighters.len() {
            if let Some(t) = self.fighters[i].knocked_out {
                if t <= 1 {
                    let f = &mut self.fighters[i];
                    f.knocked_out = None;
                    f.hp = f.hp_max;
                    self.events.push(Event::Revived(i));
                } else {
                    self.fighters[i].knocked_out = Some(t - 1);
                }
            }
        }
        let mut order: Vec<usize> = (0..self.fighters.len()).filter(|&i| self.fighters[i].kind == Kind::Unit && self.fighters[i].alive()).collect();
        // TurnManager: initiative descending; Mechanicus first on ties.
        order.sort_by_key(|&i| (-self.fighters[i].initiative, self.fighters[i].side == Side::Necron, i));
        self.order = order;
        self.turn = 0;
        for f in &mut self.fighters {
            f.moved = 0.0;
            f.move_uses = 0;
            f.ai_actions = 0;
            for s in &mut f.skills {
                s.used = 0;
                s.cooldown = (s.cooldown - 1).max(0);
            }
        }
        if let Some(i) = self.current() {
            self.events.push(Event::TurnOf(i));
        }
    }

    /// End the current fighter's turn and move on (a new round when everyone has acted).
    pub fn end_turn(&mut self) {
        if self.outcome.is_some() {
            return;
        }
        self.turn += 1;
        while self.turn < self.order.len() && !self.fighters[self.order[self.turn]].alive() {
            self.turn += 1;
        }
        if self.turn >= self.order.len() {
            self.new_round();
        } else {
            self.events.push(Event::TurnOf(self.order[self.turn]));
        }
    }

    /// Cells `who` can reach this turn, with their path cost (Dijkstra over the grid).
    pub fn reachable(&self, who: usize, budget: f32) -> BTreeMap<Cell, (f32, Cell)> {
        let start = self.fighters[who].cell;
        let mut best: BTreeMap<Cell, (f32, Cell)> = BTreeMap::from([(start, (0.0, start))]);
        let mut heap = BinaryHeap::from([Node(0.0, start)]);
        while let Some(Node(cost, c)) = heap.pop() {
            if cost > best.get(&c).map(|b| b.0).unwrap_or(f32::MAX) {
                continue;
            }
            for (n, step) in self.grid.steps(c) {
                let next = cost + step;
                if next > budget + 1e-3 {
                    continue;
                }
                if let Some(o) = self.occupant(n)
                    && o != who
                {
                    continue;
                }
                if next < best.get(&n).map(|b| b.0).unwrap_or(f32::MAX) {
                    best.insert(n, (next, c));
                    heap.push(Node(next, n));
                }
            }
        }
        best
    }

    pub fn path(map: &BTreeMap<Cell, (f32, Cell)>, to: Cell) -> Vec<Cell> {
        Battle::path_to(map, to)
    }

    fn path_to(map: &BTreeMap<Cell, (f32, Cell)>, to: Cell) -> Vec<Cell> {
        let mut path = vec![to];
        let mut at = to;
        while let Some(&(_, prev)) = map.get(&at) {
            if prev == at {
                break;
            }
            path.push(prev);
            at = prev;
        }
        path.reverse();
        path
    }

    /// SaveDataArkMechanicusSettings.GetExtraMovementCost.
    fn extra_cost(&self, done: i32) -> i32 {
        if self.extra_movement == -1 { done + 1 } else { self.extra_movement.max(1) }
    }

    /// How far `who` may path this move: the rest of its range, plus one more full range when it can buy
    /// one with cognition (TaskINodeSkillableSkillMovePrepare's two flood fills).
    pub fn move_budget(&self, who: usize) -> f32 {
        let f = &self.fighters[who];
        let pays = f.side == Side::Mechanicus && self.cognition >= self.extra_cost(f.move_uses);
        f.moves_left() + if pays { f.move_range } else { 0.0 }
    }

    /// Cognition a path of `distance` costs (TaskINodeSkillableSkillMoveExecute.OnMove).
    fn move_price(&self, who: usize, distance: f32) -> (i32, f32, i32) {
        let f = &self.fighters[who];
        let (mut used, mut uses, mut price) = (f.moved + (distance * 100.0).round() / 100.0, f.move_uses, 0);
        while used > f.move_range + 1e-4 {
            used -= f.move_range;
            uses += 1;
            if f.side == Side::Mechanicus {
                price += self.extra_cost(uses - 1);
            }
        }
        (price, used, uses)
    }

    /// Cognition needed to move `who` to `to`, or None when out of reach.
    pub fn move_cost(&self, who: usize, to: Cell) -> Option<i32> {
        let map = self.reachable(who, self.move_budget(who));
        let (distance, _) = *map.get(&to)?;
        let (price, _, _) = self.move_price(who, distance);
        let f = &self.fighters[who];
        (price <= self.cognition && price <= self.extra_cost(f.move_uses).max(if f.side == Side::Mechanicus { 0 } else { i32::MAX })).then_some(price)
    }

    pub fn move_to(&mut self, who: usize, to: Cell) -> bool {
        if self.outcome.is_some() || self.current() != Some(who) || to == self.fighters[who].cell {
            return false;
        }
        let Some(price) = self.move_cost(who, to) else { return false };
        let map = self.reachable(who, self.move_budget(who));
        let path = Battle::path_to(&map, to);
        let (_, used, uses) = self.move_price(who, map[&to].0);
        self.cognition -= price;
        let f = &mut self.fighters[who];
        f.moved = used;
        f.move_uses = uses;
        f.cell = to;
        self.events.push(Event::Moved { who, path });
        self.collect_adjacent(who);
        true
    }

    /// Walking next to an archive collects its cognition (tactical points).
    fn collect_adjacent(&mut self, who: usize) {
        if self.fighters[who].side != Side::Mechanicus {
            return;
        }
        let c = self.fighters[who].cell;
        for a in 0..self.fighters.len() {
            let f = &self.fighters[a];
            if f.kind == Kind::Archive && !f.collected && (f.cell.0 - c.0).abs() <= 1 && (f.cell.1 - c.1).abs() <= 1 {
                let gain = f.cognition_collect.max(1) + self.fighters[who].cognition_collect;
                self.fighters[a].collected = true;
                self.cognition = (self.cognition + gain).min(self.cognition_max);
                self.events.push(Event::Collected { who, archive: a, cognition: gain });
            }
        }
    }

    pub fn skill<'a>(&self, db: &'a Db, who: usize, index: usize) -> Option<&'a Record> {
        db.get(self.fighters[who].skills.get(index)?.skill)
    }

    /// Why a skill can't be used now, or None when it can.
    pub fn skill_blocked(&self, db: &Db, who: usize, index: usize) -> Option<&'static str> {
        let state = self.fighters[who].skills.get(index)?;
        let skill = db.get(state.skill)?;
        let spec = SkillSpec::read(skill);
        if state.cooldown > 0 {
            return Some("cooldown");
        }
        if spec.uses_per_turn > 0 && state.used >= spec.uses_per_turn {
            return Some("used");
        }
        if self.fighters[who].side == Side::Mechanicus && spec.cost > self.cognition {
            return Some("cognition");
        }
        if spec.range_max <= 0.0 && spec.damage.is_none() {
            return Some("passive");
        }
        let f = &self.fighters[who];
        if f.ai_actions_max != -1 && f.ai_actions >= f.ai_actions_max && f.side == Side::Necron {
            return Some("no actions");
        }
        None
    }

    /// Targets the skill can hit from where `who` stands.
    pub fn targets(&self, db: &Db, who: usize, index: usize) -> Vec<usize> {
        self.targets_from(db, who, index, self.fighters[who].cell)
    }

    /// Targets the skill could hit if `who` stood at `from`.
    pub fn targets_from(&self, db: &Db, who: usize, index: usize, from: Cell) -> Vec<usize> {
        let Some(skill) = self.skill(db, who, index) else { return Vec::new() };
        let spec = SkillSpec::read(skill);
        let me = &self.fighters[who];
        (0..self.fighters.len())
            .filter(|&t| {
                let f = &self.fighters[t];
                if f.dead || f.kind == Kind::Prop || (f.kind == Kind::Archive) {
                    return false;
                }
                let hostile = f.side != me.side;
                let ok = if t == who { spec.self_target } else if hostile { spec.hostile } else { spec.friendly };
                // Knocked-out Necrons can be hit to destroy their bodies.
                ok && (f.alive() || (hostile && f.knocked_out.is_some())) && {
                    let d = distance(from, f.cell);
                    d >= spec.range_min - 0.01 && d <= spec.range_max + 0.01
                }
            })
            .collect()
    }

    pub fn use_skill(&mut self, db: &Db, who: usize, index: usize, target: usize) -> bool {
        if self.outcome.is_some() || self.current() != Some(who) || self.skill_blocked(db, who, index).is_some() {
            return false;
        }
        if !self.targets(db, who, index).contains(&target) {
            return false;
        }
        let Some(skill) = self.skill(db, who, index) else { return false };
        let spec = SkillSpec::read(skill);
        if self.fighters[who].side == Side::Mechanicus {
            self.cognition -= spec.cost;
        }
        self.fighters[who].ai_actions += 1;
        let state = &mut self.fighters[who].skills[index];
        state.used += 1;
        state.cooldown = spec.cooldown;
        let skill_ref = state.skill;
        let at = self.fighters[target].cell;
        self.events.push(Event::Skill { who, skill: skill_ref, target: at });
        if let Some((min, max)) = spec.damage {
            self.hit(who, target, min, max, spec.energetic, spec.ignore_dodge);
            // Secondary areas around the target (splash): every other unit within the area radius.
            for (radius, smin, smax) in &spec.splash {
                let others: Vec<usize> = (0..self.fighters.len())
                    .filter(|&o| o != target && o != who && self.fighters[o].alive() && self.fighters[o].kind == Kind::Unit)
                    .filter(|&o| distance(self.fighters[o].cell, at) <= *radius + 0.01)
                    .collect();
                for o in others {
                    self.hit(who, o, *smin, *smax, spec.energetic, true);
                }
            }
        } else if let Some((min, max)) = spec.heal {
            let amount = self.rng.range(min, max + 1);
            let f = &mut self.fighters[target];
            f.hp = (f.hp + amount).min(f.hp_max);
        }
        self.check_end();
        true
    }

    fn hit(&mut self, from: usize, to: usize, min: i32, max: i32, energetic: bool, ignore_dodge: bool) {
        if self.fighters[to].knocked_out.is_some() {
            self.fighters[to].knocked_out = None;
            self.fighters[to].dead = true;
            self.events.push(Event::Destroyed(to));
            return;
        }
        if !ignore_dodge && self.rng.range_f(0.0, 100.0) <= self.fighters[to].dodge && self.fighters[to].dodge > 0.0 {
            self.events.push(Event::Dodge { who: to });
            return;
        }
        let critical = self.rng.range_f(0.0, 100.0) < self.fighters[from].critical;
        let mut damage = if critical { max } else { self.rng.range(min, max + 1) };
        let t = &mut self.fighters[to];
        let pool = if energetic { &mut t.armor_energetic } else { &mut t.armor_physical };
        let absorbed = (*pool).min(damage);
        *pool -= absorbed;
        damage -= absorbed;
        t.hp -= damage;
        self.events.push(Event::Hit { who: to, damage, critical });
        if t.hp <= 0 {
            t.hp = 0;
            if t.side == Side::Necron && t.reviveable {
                t.knocked_out = Some(REVIVE_TURNS);
                self.events.push(Event::KnockedOut(to));
            } else {
                t.dead = true;
                self.events.push(Event::Destroyed(to));
            }
        }
    }

    /// ObjectiveKillAllEnemies; defeat when every tech-priest is down.
    pub fn check_end(&mut self) {
        let necrons_up = self.fighters.iter().any(|f| f.side == Side::Necron && f.kind == Kind::Unit && f.alive());
        let has_priests = self.fighters.iter().any(|f| f.side == Side::Mechanicus && f.techpriest);
        let priests_up = self.fighters.iter().any(|f| f.side == Side::Mechanicus && f.alive() && (f.techpriest || !has_priests));
        if !priests_up {
            self.outcome = Some(Outcome::Defeat);
        } else if !necrons_up {
            self.outcome = Some(Outcome::Victory);
        }
    }

    /// The Necron fighter whose turn it is acts: close in on the nearest Mechanicus unit and attack with
    /// the best skill it can use (the game's behaviour trees reduce to this for most units).
    pub fn necron_act(&mut self, db: &Db) {
        let Some(who) = self.current() else { return };
        if self.fighters[who].side != Side::Necron || self.outcome.is_some() {
            return;
        }
        // The unit's own behaviour tree, as the game runs it.
        let mut ai = super::ai::Ai::new(db);
        if ai.play(self, who) {
            self.unported_ai.extend(ai.unknown);
            self.end_turn();
            return;
        }
        let pick_attack = |b: &Battle| -> Option<(usize, usize)> {
            (0..b.fighters[who].skills.len())
                .filter(|&s| b.skill_blocked(db, who, s).is_none())
                .filter_map(|s| {
                    let spec = SkillSpec::read(b.skill(db, who, s)?);
                    let targets: Vec<usize> = b.targets(db, who, s).into_iter().filter(|&t| b.fighters[t].side == Side::Mechanicus).collect();
                    let t = targets.into_iter().min_by_key(|&t| b.fighters[t].hp)?;
                    Some((spec.damage.map(|d| d.1).unwrap_or(0), s, t))
                })
                .max_by_key(|x| x.0)
                .map(|(_, s, t)| (s, t))
        };
        if let Some((s, t)) = pick_attack(self) {
            self.use_skill(db, who, s, t);
        } else {
            // Move towards the nearest living Mechanicus unit.
            let me = self.fighters[who].cell;
            let goal = self.fighters.iter().filter(|f| f.side == Side::Mechanicus && f.alive()).map(|f| f.cell).min_by(|a, b| distance(*a, me).total_cmp(&distance(*b, me)));
            if let Some(goal) = goal {
                let map = self.reachable(who, self.move_budget(who));
                if let Some((&best, _)) = map.iter().min_by(|a, b| distance(*a.0, goal).total_cmp(&distance(*b.0, goal)))
                    && best != me
                {
                    self.move_to(who, best);
                }
            }
            if let Some((s, t)) = pick_attack(self) {
                self.use_skill(db, who, s, t);
            }
        }
        self.end_turn();
    }
}

/// RandomSpawnDeployment.GetEntity: entries still under their minimum first; otherwise any entry under
/// its maximum that the remaining points afford; chosen by weight.
fn pick_entity(entries: &[Value], points: i32, spawned: &[usize], rng: &mut Rng) -> Option<usize> {
    let field = |e: &Value, k: &str| e.get(k).map(as_int).unwrap_or(0) as i32;
    let count = |i: usize| spawned.iter().filter(|&&s| s == i).count() as i32;
    let mut pool: Vec<usize> = (0..entries.len()).filter(|&i| field(&entries[i], "min") > 0 && count(i) < field(&entries[i], "min")).collect();
    if pool.is_empty() {
        pool = (0..entries.len())
            .filter(|&i| field(&entries[i], "max") <= 0 || count(i) < field(&entries[i], "max"))
            .filter(|&i| field(&entries[i], "point") <= points)
            .collect();
    }
    if pool.is_empty() {
        return None;
    }
    let total: i32 = pool.iter().map(|&i| field(&entries[i], "weight")).sum();
    let roll = rng.range(0, total + 1);
    let mut acc = 0;
    for &i in &pool {
        acc += field(&entries[i], "weight");
        if roll <= acc {
            return Some(i);
        }
    }
    pool.last().copied()
}

/// What a scene entity is, from its data component's class: fighting units, collectable archives, or
/// scenery that only occupies its node (props, sarcophagi, traps, barriers, platforms, thrones…).
pub fn entity_kind(class: &str) -> Kind {
    const UNITS: &[&str] = &[
        "NecronWarrior", "NecronImmortal", "NecronCanoptek", "NecronLord", "NecronCryptek", "NecronDeathMark", "NecronDestroyer",
        "NecronFlayedOneDataMb", "NecronLychguard", "NecronPraetorian", "NecronVargard", "NecronScarabDataMb", "NecronTurret",
        "NecronCtan", "XenariteBoss", "XenariteServitor", "XenariteTechpriest",
    ];
    if class.contains("Archive") {
        Kind::Archive
    } else if UNITS.iter().any(|u| class.starts_with(u)) {
        Kind::Unit
    } else {
        Kind::Prop
    }
}

pub fn distance(a: Cell, b: Cell) -> f32 {
    (((a.0 - b.0).pow(2) + (a.1 - b.1).pow(2)) as f32).sqrt()
}

#[derive(PartialEq)]
struct Node(f32, Cell);

impl Eq for Node {}

impl Ord for Node {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        other.0.total_cmp(&self.0).then_with(|| self.1.cmp(&other.1))
    }
}

impl PartialOrd for Node {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

/// A fighter's stats from its …DataBase record.
fn fighter_from(db: &Db, base: Option<&Record>, side: Side, cell: Cell, yaw: f32) -> Fighter {
    let life = base.and_then(|b| b.get("life")).cloned().unwrap_or(Value::Null);
    let li = |k: &str| life.get(k).map(as_int).unwrap_or(0) as i32;
    let hp_max = li("hpMax").max(1);
    Fighter {
        name: base.map(|b| b.str("unitName").to_string()).unwrap_or_default(),
        side,
        kind: Kind::Unit,
        base: base.map(|b| b.id),
        prefab: base.and_then(|b| db.follow(b, "prefabEntity")).map(|p| p.id).or_else(|| base.and_then(|b| b.get("prefabEntity")).and_then(|p| db.pptr(base.unwrap(), p))),
        unit: None,
        cell,
        yaw,
        hp: hp_max,
        hp_max,
        armor_physical: li("armorPhysicalMax"),
        armor_energetic: li("armorEnergeticMax"),
        dodge: base.map(|b| b.float("dodgeRate")).unwrap_or(0.0),
        critical: base.map(|b| b.float("criticalRate")).unwrap_or(0.0),
        initiative: base.map(|b| b.int("initiative") as i32).unwrap_or(0),
        move_range: base.map(|b| b.float("movementRangeMax")).unwrap_or(0.0),
        moved: 0.0,
        move_uses: 0,
        skills: base
            .map(|b| db.follow_all(b, "skills").iter().map(|s| s.id).chain(equipment_skills(db, b)).map(|skill| SkillState { skill, cooldown: 0, used: 0 }).collect())
            .unwrap_or_default(),
        knocked_out: None,
        reviveable: base.map(|b| b.flag("reviveable")).unwrap_or(false),
        dead: false,
        collected: false,
        cognition_collect: base.map(|b| b.int("cognitionPointCollect") as i32).unwrap_or(0),
        techpriest: base.is_some_and(|b| b.class == "TechpriestDataBase"),
        ai_actions_max: base.map(|b| b.get("aiNbActionMax").map(crate::data::as_int).unwrap_or(-1) as i32).unwrap_or(-1),
        ai_actions: 0,
    }
}

/// Skills granted by a record's `equipments` (EquipmentBehaviourSkillable).
fn equipment_skills(db: &Db, base: &Record) -> Vec<Ref> {
    db.follow_all(base, "equipments").iter().flat_map(|e| skills_of_equipment(db, e)).collect()
}

pub fn skills_of_equipment(db: &Db, e: &Record) -> Vec<Ref> {
    let Some(skills) = e.get("skills").map(|s| e.deref(s)) else { return Vec::new() };
    let list = skills.get("skills").map(|l| e.deref(l)).cloned().unwrap_or(Value::Null);
    items(&list).iter().filter_map(|s| db.resolve(e, s)).map(|s| s.id).collect()
}

/// A Mechanicus unit's battle skills: its own, then those of everything it has equipped.
pub fn unit_skills(db: &Db, unit: &Unit) -> Vec<Ref> {
    let mut out = unit.skills.clone();
    if let Some(base) = db.get(unit.base) {
        out.extend(db.follow_all(base, "skills").iter().map(|s| s.id));
    }
    for eq in unit.equipment.values() {
        if let Some(e) = db.get(*eq) {
            out.extend(skills_of_equipment(db, e));
        }
    }
    out.sort();
    out.dedup();
    out
}

/// What the battle needs from a Skill record.
pub struct SkillSpec {
    pub name: String,
    pub cost: i32,
    pub cooldown: i32,
    pub uses_per_turn: i32,
    pub range_min: f32,
    pub range_max: f32,
    pub hostile: bool,
    pub friendly: bool,
    pub self_target: bool,
    pub damage: Option<(i32, i32)>,
    pub heal: Option<(i32, i32)>,
    pub energetic: bool,
    pub ignore_dodge: bool,
    /// (radius, min, max) of follow-up target infos around the primary target.
    pub splash: Vec<(f32, i32, i32)>,
}

impl SkillSpec {
    pub fn read(skill: &Record) -> SkillSpec {
        let d = |v: &Value| skill.deref(v).clone();
        let cost = skill.get("tacticalPointCost").map(d).and_then(|c| c.get("tacticalPointCostMax").map(as_int)).unwrap_or(0) as i32;
        let cooldown = skill.get("cooldown").map(d).and_then(|c| c.get("cooldown").map(as_int)).unwrap_or(0) as i32;
        let mut uses = 0;
        if let Some(attrs) = skill.get("attributes").map(d).and_then(|a| a.get("attributes").map(|x| skill.deref(x).clone())) {
            for a in items(&attrs) {
                if odin_type(a) == "AttributeSkillLimitUseInTurn" {
                    uses = a.get("value").and_then(Value::as_f64).unwrap_or(1.0) as i32;
                }
            }
        }
        let infos: Vec<Value> = skill
            .get("target")
            .map(d)
            .and_then(|t| t.get("targetInfos").map(|x| skill.deref(x).clone()))
            .map(|l| items(&l).iter().map(|i| skill.deref(i).clone()).collect())
            .unwrap_or_default();
        let mut spec = SkillSpec {
            name: skill.name().to_string(),
            cost,
            cooldown,
            uses_per_turn: uses,
            range_min: 0.0,
            range_max: 0.0,
            hostile: false,
            friendly: false,
            self_target: false,
            damage: None,
            heal: None,
            energetic: false,
            ignore_dodge: false,
            splash: Vec::new(),
        };
        for (n, info) in infos.iter().enumerate() {
            let range = info.get("areaRange").and_then(|a| a.get("range")).cloned().unwrap_or(Value::Null);
            let (rmin, rmax) = (range.get("min").and_then(Value::as_f64).unwrap_or(0.0) as f32, range.get("max").and_then(Value::as_f64).unwrap_or(0.0) as f32);
            let mut value: Option<(i32, i32, bool, bool)> = None;
            for v in items(info.get("values").unwrap_or(&Value::Null)) {
                let r = v.get("value").cloned().unwrap_or(Value::Null);
                let (min, max) = (r.get("min").map(as_int).unwrap_or(0) as i32, r.get("max").map(as_int).unwrap_or(0) as i32);
                let damage = odin_type(v.get("valueModifier").unwrap_or(&Value::Null)).contains("Damage");
                let ty = v.get("type").map(as_int).unwrap_or(40);
                value = Some((min, max, damage, ty == 5 || ty == 30));
            }
            if n == 0 {
                spec.range_min = rmin;
                spec.range_max = rmax;
                let relations: Vec<i64> = items(info.get("targetRelations").unwrap_or(&Value::Null)).iter().map(as_int).collect();
                spec.hostile = relations.contains(&2);
                spec.friendly = relations.contains(&1) || relations.contains(&4);
                spec.self_target = info.get("canTargetOwner").map(as_bool).unwrap_or(false);
                spec.ignore_dodge = items(info.get("hitBehaviours").unwrap_or(&Value::Null)).iter().any(|h| as_int(h) == 0);
                if let Some((min, max, damage, energetic)) = value {
                    if damage || spec.hostile {
                        spec.damage = Some((min, max));
                        spec.energetic = energetic;
                    } else {
                        spec.heal = Some((min, max));
                    }
                }
            } else if let Some((min, max, true, _)) = value {
                spec.splash.push((rmax, min, max));
            }
        }
        spec
    }
}
