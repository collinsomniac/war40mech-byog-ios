//! A tomb in progress: the team's room, the Necron awakening, and the event dialogs. Everything the
//! player does is one pipeline, as in the game: a room's event opens its dialog, a choice runs its
//! actions in priority order (rewards, dialog changes, fights, the end), and moving through a door is
//! itself the "opened door" event.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

use super::rng::{Rng, lerp, round};
use super::save::{Save, Unit};
use super::tomb::{self, Tomb};
use crate::data::{Db, Record, Ref, as_bool, as_int, items, odin_type};

/// DungeonAwakeGauge: 0..35, a level every 5.
pub const AWAKE_MAX: i32 = 35;

pub fn awake_level(value: i32) -> i32 {
    (value / 5).min(7)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EventState {
    pub started: bool,
    pub ended: bool,
}

/// What a choice produced, shown under the dialog (the game's reward lines).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Outcome {
    Blackstone(i32),
    Cognition(i32),
    Damage { unit: String, amount: i32 },
    DamageGroup(i32),
    Heal { unit: String, amount: i32 },
    HealGroup(i32),
    Awakening(i32),
    AwakeLevel(i32),
    Blueprint(Ref),
    Canticle(Ref),
    Cartography,
    Initiative(i32),
    /// A line of extra dialog text (DungeonEventDialogActionDialog), as I2 terms.
    Text(Vec<String>),
}

/// What the presentation must do next after a player action.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Keep showing (or start showing) the open dialog.
    Dialog,
    /// Dialog closed; back to the map.
    Map,
    /// Launch a battle for the current room.
    Battle { room_data: Option<Ref>, template: Option<i64> },
    /// The mission's objective is complete.
    Victory,
    /// Everyone is dead.
    Defeat,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenDialog {
    pub room: usize,
    /// DungeonEventDialog.ID: 0 = START, 1.. = A..
    pub id: i64,
    /// The door being opened, when this is the opened-door event.
    pub door: Option<usize>,
    pub outcomes: Vec<Outcome>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Expedition {
    pub mission: Ref,
    pub tomb: Tomb,
    pub units: Vec<Unit>,
    pub canticles: Vec<Ref>,
    pub room: usize,
    pub visited: BTreeSet<usize>,
    pub revealed: BTreeSet<usize>,
    pub events: Vec<EventState>,
    pub awake: i32,
    pub cognition: i32,
    pub cognition_max: i32,
    pub blackstone: i32,
    pub initiative: i32,
    pub dialog: Option<OpenDialog>,
    /// Actions to run when the pending fight is won (DungeonEventDialogActionFight.victoryActions).
    pub victory_actions: Option<(usize, Vec<Value>)>,
    /// Battle rooms already fought in this tomb (TaskEventActionLaunchFight avoids repeats).
    pub encountered: Vec<Ref>,
    pub rng: Rng,
}

impl Expedition {
    /// TaskExploInitialize: the deployment's team enters a freshly generated tomb.
    pub fn begin(db: &Db, save: &Save) -> Result<Expedition, String> {
        let deployment = save.deployment.as_ref().ok_or("no deployment")?;
        let mission = db.get(deployment.mission).ok_or("deployment mission missing from the pack")?;
        let tomb = tomb::generate(db, save, mission, None)?;
        let cognition_max = db.single("GameplayConfig").map(|g| g.int("baseCognitionGaugeSize") as i32).unwrap_or(4);
        let mut ex = Expedition {
            mission: mission.id,
            room: tomb.start,
            visited: BTreeSet::from([tomb.start]),
            revealed: BTreeSet::new(),
            events: vec![EventState::default(); tomb.rooms.len()],
            rng: Rng::new(tomb.seed ^ 0x5eed),
            tomb,
            units: deployment.units.clone(),
            canticles: deployment.canticles.clone(),
            awake: 0,
            cognition: 0,
            cognition_max,
            blackstone: 0,
            initiative: 0,
            dialog: None,
            victory_actions: None,
            encountered: Vec::new(),
        };
        ex.reveal_around(ex.room);
        ex.arrive(db);
        Ok(ex)
    }

    /// Rooms the player can see: visited, their neighbours, and whatever cartography revealed.
    pub fn visible(&self, room: usize) -> bool {
        self.visited.contains(&room) || self.revealed.contains(&room)
    }

    fn reveal_around(&mut self, room: usize) {
        let next: Vec<usize> = self.tomb.neighbours(room).collect();
        self.revealed.extend(next);
    }

    pub fn can_move(&self, to: usize) -> bool {
        self.dialog.is_none() && self.tomb.neighbours(self.room).any(|r| r == to)
    }

    /// Clicking a neighbouring room: its door's opened-door event (TaskEventActionDoor).
    pub fn move_to(&mut self, db: &Db, save: &Save, to: usize) -> Next {
        if !self.can_move(to) {
            return Next::Map;
        }
        // GameplayConfig.GetVisitRoomAwakeCost: first visits cost the settings' awakening per room.
        if !self.visited.contains(&to) && save.settings.awakening {
            self.add_awake(save.settings.awakening_room, &mut Vec::new());
        }
        self.room = to;
        self.visited.insert(to);
        self.reveal_around(to);
        self.arrive(db);
        if self.dialog.is_some() { Next::Dialog } else { Next::Map }
    }

    /// An unfinished auto-execute event opens as the team walks in.
    fn arrive(&mut self, db: &Db) {
        let room = self.room;
        if let Some(event) = self.event_record(db, room)
            && !self.events[room].ended
            && event.flag("autoExecute")
        {
            self.open(db, room);
        }
    }

    pub fn event_record<'a>(&self, db: &'a Db, room: usize) -> Option<&'a Record> {
        self.tomb.rooms.get(room)?.event.and_then(|e| db.get(e))
    }

    /// TaskINodeDungeonEventTrigger.
    pub fn open(&mut self, db: &Db, room: usize) {
        let Some(event) = self.event_record(db, room) else { return };
        self.events[room].started = true;
        if event.flag("noDialog") {
            let actions = event.get("noDialogActions").map(|a| items(event.deref(a)).to_vec()).unwrap_or_default();
            self.dialog = Some(OpenDialog { room, id: 0, door: None, outcomes: Vec::new() });
            let _ = self.run(db, None, room, actions);
            return;
        }
        self.dialog = Some(OpenDialog { room, id: 0, door: None, outcomes: Vec::new() });
    }

    /// The open dialog's DungeonEventDialog node.
    pub fn dialog_node<'a>(&self, db: &'a Db) -> Option<(&'a Record, &'a Value)> {
        let open = self.dialog.as_ref()?;
        let event = self.event_record(db, open.room)?;
        dialog_by_id(event, open.id).map(|d| (event, d))
    }

    /// The choices of the open dialog; glyph events offer their glyphs instead (DungeonEventDialog.GetDialogInfo).
    pub fn choices<'a>(&self, db: &'a Db) -> Vec<Choice<'a>> {
        let Some((event, node)) = self.dialog_node(db) else { return Vec::new() };
        let open = self.dialog.as_ref().unwrap();
        if open.id == 0 && event.int("type") == 5 {
            return self.tomb.rooms[open.room]
                .glyphs
                .iter()
                .filter_map(|g| db.get(*g))
                .map(|g| Choice {
                    name: g.str("glyphName").to_string(),
                    desc: g.str("glyphDescription").to_string(),
                    owner: g,
                    actions: g.get("actions").map(|a| items(g.deref(a)).to_vec()).unwrap_or_default(),
                })
                .collect();
        }
        node.get("choices")
            .map(|c| items(event.deref(c)))
            .unwrap_or_default()
            .iter()
            .map(|c| event.deref(c))
            .map(|c| Choice {
                name: c.get("choiceName").and_then(Value::as_str).unwrap_or("").to_string(),
                desc: c.get("choiceDesc").and_then(Value::as_str).unwrap_or("").to_string(),
                owner: event,
                actions: c.get("actions").map(|a| items(event.deref(a)).to_vec()).unwrap_or_default(),
            })
            .collect()
    }

    /// The player picks choice `index` of the open dialog.
    pub fn choose(&mut self, db: &Db, save: &mut Save, index: usize) -> Next {
        let choices = self.choices(db);
        let Some(choice) = choices.get(index) else { return Next::Dialog };
        let (owner, actions) = (choice.owner.id, choice.actions.clone());
        let room = self.dialog.as_ref().map(|d| d.room).unwrap_or(self.room);
        if let Some(d) = self.dialog.as_mut() {
            d.outcomes.clear();
        }
        let owner = db.get(owner);
        self.run(db, Some(save), room, actions_with_owner(owner, actions))
    }

    /// TaskINodeDungeonEventActionTrigger: flatten reward factories, order by priority, run each.
    fn run(&mut self, db: &Db, mut save: Option<&mut Save>, room: usize, actions: Vec<Value>) -> Next {
        let owner = self.event_record(db, room);
        let mut flat: Vec<Value> = Vec::new();
        for a in actions {
            if odin_type(&a) == "DungeonEventDialogActionRewardFactory" {
                for (flag, key) in FACTORY {
                    if a.get(flag).map(as_bool).unwrap_or(false)
                        && let Some(sub) = a.get(key)
                    {
                        flat.push(owner.map(|o| o.deref(sub).clone()).unwrap_or_else(|| sub.clone()));
                    }
                }
            } else {
                flat.push(a);
            }
        }
        flat.sort_by_key(priority);
        let mut next = Next::Dialog;
        let mut outcomes = Vec::new();
        for action in flat {
            match odin_type(&action) {
                "DungeonEventDialogActionChangeDialog" => {
                    if let Some(d) = self.dialog.as_mut() {
                        d.id = action.get("id").map(as_int).unwrap_or(0);
                    }
                    next = Next::Dialog;
                }
                "DungeonEventDialogActionDialog" => {
                    let lines = action
                        .get("descriptions")
                        .map(items)
                        .unwrap_or_default()
                        .iter()
                        .filter_map(|d| d.get("description").and_then(Value::as_str).map(str::to_string))
                        .collect();
                    outcomes.push(Outcome::Text(lines));
                }
                "DungeonEventDialogActionEnd" => {
                    self.events[room].ended = true;
                    self.dialog = None;
                    next = if self.objective_done() { Next::Victory } else { Next::Map };
                }
                "DungeonEventDialogActionCancel" => {
                    self.dialog = None;
                    next = Next::Map;
                }
                "DungeonEventDialogActionDoor" => {
                    self.dialog = None;
                    next = Next::Map;
                }
                "DungeonEventDialogActionFight" => {
                    let victory = action.get("victoryActions").map(|v| items(v).to_vec()).unwrap_or_default();
                    self.victory_actions = Some((room, victory));
                    let template = action.get("overrideTemplate").map(as_bool).unwrap_or(false).then(|| action.get("template").map(as_int).unwrap_or(0));
                    let room_data = owner.and_then(|e| e.flag("constrainRoom").then(|| db.follow(e, "roomData")).flatten()).map(|r| r.id);
                    next = Next::Battle { room_data, template };
                }
                _ => {
                    if let Some(o) = self.reward(db, save.as_deref_mut(), &action) {
                        outcomes.push(o);
                    }
                }
            }
        }
        if let Some(d) = self.dialog.as_mut() {
            d.outcomes.extend(outcomes);
        }
        if self.units.iter().all(|u| u.hp <= 0) {
            return Next::Defeat;
        }
        next
    }

    /// The fight launched by the open event was won: run its victory actions.
    pub fn battle_won(&mut self, db: &Db, save: &mut Save) -> Next {
        let Some((room, actions)) = self.victory_actions.take() else { return Next::Map };
        self.dialog = Some(OpenDialog { room, id: 0, door: None, outcomes: Vec::new() });
        let owner = self.event_record(db, room);
        let actions = actions_with_owner(owner, actions);
        let next = self.run(db, Some(save), room, actions);
        if self.dialog.is_some() { Next::Dialog } else { next }
    }

    /// TaskExploCheckEnd: the primary objective (its specific events) is complete.
    pub fn objective_done(&self) -> bool {
        let objectives: Vec<usize> = (0..self.tomb.rooms.len()).filter(|&r| self.tomb.rooms[r].objective).collect();
        !objectives.is_empty() && objectives.iter().all(|&r| self.events[r].ended)
    }

    fn add_awake(&mut self, amount: i32, outcomes: &mut Vec<Outcome>) {
        let before = awake_level(self.awake);
        self.awake = (self.awake + amount).clamp(0, AWAKE_MAX);
        if awake_level(self.awake) != before {
            outcomes.push(Outcome::AwakeLevel(awake_level(self.awake)));
        }
    }

    /// GameplayConfig.GetEventRewardQuality.
    fn quality(&mut self, save: Option<&Save>) -> f32 {
        let countdown = save.map(|s| s.story.clamped_countdown()).unwrap_or(0.0);
        (countdown * 0.01 + self.rng.range_f(-0.2, 0.2)).clamp(0.0, 1.0)
    }

    fn reward(&mut self, db: &Db, save: Option<&mut Save>, action: &Value) -> Option<Outcome> {
        let config = db.single("GameplayConfig")?;
        let cfg = |k: &str| config.float(k);
        let multiplier = action.get("multiplier").and_then(Value::as_f64).unwrap_or(1.0) as f32;
        let overridden = |flag: &str, value: &str| -> Option<i32> {
            action.get(flag).map(as_bool).unwrap_or(false).then(|| action.get(value).map(as_int).unwrap_or(0) as i32)
        };
        let quality = self.quality(save.as_deref());
        let scaled = |min: &str, max: &str| round(lerp(cfg(min), cfg(max), quality) * multiplier);
        Some(match odin_type(action) {
            "DungeonEventDialogActionRewardData" => {
                let modifier = save.as_ref().map(|s| s.settings.blackstone_modifier).unwrap_or(0) as f32 / 100.0;
                let base = match overridden("overrideData", "overridedData") {
                    Some(v) => v as f32,
                    None => {
                        let spread = cfg("rewardRandomIntervalBlackstone");
                        let q = self.rng.range_f(quality - spread, quality + spread).clamp(0.0, 1.0);
                        lerp(cfg("rewardEventBlackstoneMin"), cfg("rewardEventBlackstoneMax"), q) * multiplier
                    }
                };
                let amount = round(base + base * modifier);
                self.blackstone += amount;
                Outcome::Blackstone(amount)
            }
            "DungeonEventDialogActionRewardCognitionPoint" => {
                let add = action.get("type").map(as_int).unwrap_or(0) == 0;
                let amount = overridden("overrideCognition", "overridedCognition").unwrap_or_else(|| {
                    if add { scaled("rewardEventCognitionAddMin", "rewardEventCognitionAddMax") } else { scaled("rewardEventCognitionSubtractMin", "rewardEventCognitionSubtractMax") }
                });
                let delta = if add { amount } else { -amount };
                self.cognition = (self.cognition + delta).clamp(0, self.cognition_max);
                Outcome::Cognition(delta)
            }
            "DungeonEventDialogActionRewardDamage" => {
                let amount = overridden("overrideDamage", "overridedDamage").unwrap_or_else(|| scaled("rewardEventDamageMin", "rewardEventDamageMax"));
                let only = action.get("constrainType").map(as_bool).unwrap_or(false).then(|| action.get("constrainEntityType").map(as_int).unwrap_or(0));
                let pool: Vec<usize> = (0..self.units.len())
                    .filter(|&i| self.units[i].hp > amount && self.units[i].hp == self.units[i].hp_max)
                    .filter(|&i| only.is_none_or(|t| (t == 0) == self.units[i].is_techpriest()))
                    .collect();
                if pool.is_empty() {
                    return None;
                }
                let i = pool[self.rng.range(0, pool.len() as i32) as usize];
                self.units[i].hp -= amount;
                Outcome::Damage { unit: self.units[i].name.clone(), amount }
            }
            "DungeonEventDialogActionRewardDamageGroup" => {
                let amount = overridden("overrideDamage", "overridedDamage").unwrap_or_else(|| scaled("rewardEventDamageGroupMin", "rewardEventDamageGroupMax"));
                for u in self.units.iter_mut().filter(|u| u.hp > amount) {
                    u.hp -= amount;
                }
                Outcome::DamageGroup(amount)
            }
            "DungeonEventDialogActionRewardHeal" => {
                let amount = overridden("overrideHeal", "overridedHeal").unwrap_or_else(|| scaled("rewardEventHealMin", "rewardEventHealMax"));
                let i = (0..self.units.len()).filter(|&i| self.units[i].hp > 0 && self.units[i].hp < self.units[i].hp_max).min_by_key(|&i| (self.units[i].hp, self.units[i].hp_max))?;
                let u = &mut self.units[i];
                u.hp = (u.hp + amount).min(u.hp_max);
                Outcome::Heal { unit: u.name.clone(), amount }
            }
            "DungeonEventDialogActionRewardHealGroup" => {
                let amount = overridden("overrideHeal", "overridedHeal").unwrap_or_else(|| scaled("rewardEventHealGroupMin", "rewardEventHealGroupMax"));
                for u in self.units.iter_mut().filter(|u| u.hp > 0) {
                    u.hp = (u.hp + amount).min(u.hp_max);
                }
                Outcome::HealGroup(amount)
            }
            "DungeonEventDialogActionRewardIncreaseAwakening" => {
                let amount = overridden("overrideAwakening", "overridedAwakening").unwrap_or_else(|| scaled("rewardEventAwakeningIncreaseMin", "rewardEventAwakeningIncreaseMax"));
                let mut extra = Vec::new();
                self.add_awake(amount, &mut extra);
                if let Some(d) = self.dialog.as_mut() {
                    d.outcomes.extend(extra);
                }
                Outcome::Awakening(amount)
            }
            "DungeonEventDialogActionRewardReduceAwakening" => {
                let amount = overridden("overrideAwakening", "overridedAwakening").unwrap_or_else(|| scaled("rewardEventAwakeningDecreaseMin", "rewardEventAwakeningDecreaseMax"));
                self.awake = (self.awake - amount).max(0);
                Outcome::Awakening(-amount)
            }
            "DungeonEventDialogActionRewardResetAwakening" => {
                let amount = -self.awake;
                self.awake = 0;
                Outcome::Awakening(amount)
            }
            "DungeonEventDialogActionRewardCartography" => {
                match action.get("roomTileReveal").map(as_int).unwrap_or(0) {
                    1 => self.revealed.extend(0..self.tomb.rooms.len()),
                    _ => {
                        let around: Vec<usize> = self.revealed.iter().flat_map(|&r| self.tomb.neighbours(r).collect::<Vec<_>>()).collect();
                        self.revealed.extend(around);
                    }
                }
                Outcome::Cartography
            }
            "DungeonEventDialogActionRewardNecronInitiative" => {
                self.initiative = match action.get("initiative").map(as_int).unwrap_or(0) {
                    1 => 1,
                    2 => -1,
                    _ => 0,
                };
                Outcome::Initiative(self.initiative)
            }
            "DungeonEventDialogActionRewardBlueprint" => {
                let save = save?;
                let owned: BTreeSet<Ref> = save.blueprints.iter().copied().collect();
                let pool: Vec<&Record> = db.all("Blueprint").filter(|b| !owned.contains(&b.id)).collect();
                let pick = self.rng.pick(&pool)?.id;
                save.blueprints.push(pick);
                Outcome::Blueprint(pick)
            }
            "DungeonEventDialogActionRewardCanticle" => {
                let save = save?;
                let canticle = self.event_record(db, self.dialog.as_ref()?.room).and_then(|e| db.resolve(e, action.get("canticle")?))?;
                if !save.canticles.contains(&canticle.id) {
                    save.canticles.push(canticle.id);
                }
                Outcome::Canticle(canticle.id)
            }
            _ => return None,
        })
    }
}

impl Expedition {
    /// TaskEventActionLaunchFight / TaskExploGenerateRoom: the battle room and template for a fight in
    /// `room` — the event's own room when it constrains one, otherwise an unused room of the mission's
    /// tileset and the event's room type whose templates the campaign's awakening allows.
    pub fn battle_room<'a>(&mut self, db: &'a Db, save: &Save, room: usize, forced: Option<Ref>, template: Option<i64>) -> Option<(&'a Record, Option<i64>)> {
        let event = self.event_record(db, room);
        let countdown = save.story.clamped_countdown() as i64;
        if let Some(r) = forced.and_then(|r| db.get(r)) {
            let t = template.or_else(|| event.filter(|e| e.flag("constrainTemplate")).map(|e| e.int("roomDataTemplate")));
            self.encountered.push(r.id);
            return Some((r, t));
        }
        let mission = db.get(self.mission)?;
        let tileset = mission.int("tileset");
        let kind = event.filter(|e| e.flag("constrainRoomType")).map(|e| e.int("roomType")).unwrap_or(0);
        let available: Vec<(&Record, Vec<i64>)> = db
            .all("RoomData")
            .filter(|r| r.int("tileset") == tileset && r.int("type") == kind)
            .map(|r| (r, templates(r, countdown)))
            .filter(|(_, t)| !t.is_empty())
            .collect();
        let fresh: Vec<&(&Record, Vec<i64>)> = available.iter().filter(|(r, _)| !self.encountered.contains(&r.id)).collect();
        let pool: Vec<&(&Record, Vec<i64>)> = if fresh.is_empty() { available.iter().collect() } else { fresh };
        let (r, ts) = *self.rng.pick(&pool)?;
        let t = template.or_else(|| self.rng.pick(ts).copied());
        self.encountered.push(r.id);
        Some((r, t))
    }
}

/// RoomData.GetAvailableTemplates: templates not for testing whose awakening constraint is met.
fn templates(room: &Record, countdown: i64) -> Vec<i64> {
    let Some(list) = room.get("templates") else { return Vec::new() };
    items(room.deref(list))
        .iter()
        .map(|t| room.deref(t))
        .filter(|t| !t.get("onlyForTesting").map(as_bool).unwrap_or(false))
        .filter(|t| !t.get("awakeningConstraint").map(as_bool).unwrap_or(false) || countdown >= t.get("awakeningConstraintValue").map(as_int).unwrap_or(0))
        .map(|t| t.get("template").map(as_int).unwrap_or(0))
        .collect()
}

/// A choice in the open dialog. `owner` is the record whose Odin graph the actions belong to.
pub struct Choice<'a> {
    pub name: String,
    pub desc: String,
    pub owner: &'a Record,
    pub actions: Vec<Value>,
}

/// Resolve `$ref_internal` action entries against their owning record.
fn actions_with_owner(owner: Option<&Record>, actions: Vec<Value>) -> Vec<Value> {
    match owner {
        Some(o) => actions.iter().map(|a| o.deref(a).clone()).collect(),
        None => actions,
    }
}

const FACTORY: [(&str, &str); 12] = [
    ("hasBlueprint", "blueprint"),
    ("hasCartography", "cartography"),
    ("hasCognition", "cognition"),
    ("hasDamage", "damage"),
    ("hasDamageGroup", "damageGroup"),
    ("hasData", "data"),
    ("hasHeal", "heal"),
    ("hasHealGroup", "healGroup"),
    ("hasIncreaseAwakening", "increaseAwakening"),
    ("hasReduiceAwakening", "reduiceAwakening"),
    ("hasResetAwakening", "resetAwakening"),
    ("hasInitiative", "initiative"),
];

/// Each action class's GetPriorityLevel.
fn priority(action: &Value) -> i64 {
    match odin_type(action) {
        "DungeonEventDialogActionChangeDialog" => i64::MIN,
        "DungeonEventDialogActionDialog" => -10_000,
        "DungeonEventDialogActionDoor" | "DungeonEventDialogActionFight" | "DungeonEventDialogActionCancel" => -100,
        "DungeonEventDialogActionEnd" => i64::MAX,
        "DungeonEventDialogActionRewardIncreaseAwakening" => -40,
        "DungeonEventDialogActionRewardDamage" | "DungeonEventDialogActionRewardHeal" | "DungeonEventDialogActionRewardNecronInitiative" => -10,
        _ => action.get("priorityLevel").map(as_int).unwrap_or(-10),
    }
}

/// DungeonEvent.GetDialog(id): START is `startDialog`, the rest are in `dialogs`.
pub fn dialog_by_id(event: &Record, id: i64) -> Option<&Value> {
    let start = event.get("startDialog").map(|d| event.deref(d));
    if id == 0 {
        return start;
    }
    let dialogs = event.get("dialogs")?;
    items(event.deref(dialogs)).iter().map(|d| event.deref(d)).find(|d| d.get("id").map(as_int) == Some(id))
}

impl Expedition {
    /// The next room on the shortest corridor route from the team's room to `target`.
    pub fn step_towards(&self, target: usize) -> Option<usize> {
        let n = self.tomb.rooms.len();
        let mut prev = vec![usize::MAX; n];
        let mut queue = std::collections::VecDeque::from([self.room]);
        prev[self.room] = self.room;
        while let Some(r) = queue.pop_front() {
            for o in self.tomb.neighbours(r) {
                if prev[o] == usize::MAX {
                    prev[o] = r;
                    queue.push_back(o);
                }
            }
        }
        if prev.get(target).is_none_or(|&p| p == usize::MAX) || target == self.room {
            return None;
        }
        let mut at = target;
        while prev[at] != self.room {
            at = prev[at];
        }
        Some(at)
    }

    /// The nearest room whose event hasn't ended (objective rooms first).
    pub fn next_event_room(&self, db: &Db) -> Option<usize> {
        let open = |r: &usize| self.event_record(db, *r).is_some() && !self.events[*r].ended;
        (0..self.tomb.rooms.len()).filter(open).min_by_key(|&r| (!self.tomb.rooms[r].objective, self.tomb.rooms[r].depth))
    }
}
