//! Necron AI: the game's own Behavior Designer trees (each unit's `behaviors`, stored as Behavior
//! Designer JSON in ExternalBehaviorTree records), run by an interpreter. Composites behave as in
//! Behavior Designer; leaves are the Mechanicus tasks (BehaviorDesigner.Runtime.Mechanicus), re-written
//! here against the battle rules. A leaf this port lacks fails, so the tree falls through to its other
//! branches, and is reported once.

use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use super::battle::{Battle, Cell, Kind, Side, distance};
use crate::data::{Db, Record, as_int, items};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Success,
    Failure,
}

#[derive(Debug, Clone)]
struct TargetAction {
    skill: Option<usize>,
    target: usize,
    /// Where to move first, and the walk's length.
    dest: Option<(Cell, f32)>,
}

#[derive(Default)]
struct Board {
    actions: Vec<TargetAction>,
    selected: Option<TargetAction>,
    bools: BTreeMap<String, bool>,
    ints: BTreeMap<String, i64>,
    ended: bool,
}

pub struct Ai<'a> {
    db: &'a Db,
    tags: BTreeMap<String, i64>,
    pub unknown: BTreeSet<String>,
}

impl<'a> Ai<'a> {
    pub fn new(db: &'a Db) -> Ai<'a> {
        let tags = db
            .enums()
            .and_then(|e| e.iter().find(|(k, _)| k.ends_with("SkillBehaviourTag+TAG")).map(|(_, v)| v.clone()))
            .unwrap_or_default();
        Ai { db, tags, unknown: BTreeSet::new() }
    }

    /// The fighter's behaviour tree (its data record's first `behaviors` entry).
    pub fn tree_of(&self, b: &Battle, who: usize) -> Option<(&'a Record, Value)> {
        let base = self.db.get(b.fighters[who].base?)?;
        let first = items(base.get("behaviors")?).first()?.clone();
        let tree = self.db.resolve(base, first.get("behaviorTree")?)?;
        Some((tree, parse_tree(tree)?))
    }

    /// Run `who`'s turn by its tree. Returns false when it has no tree (the caller falls back).
    pub fn play(&mut self, b: &mut Battle, who: usize) -> bool {
        let Some((record, root)) = self.tree_of(b, who) else { return false };
        let mut board = Board::default();
        self.run(b, who, record, &root, &mut board, 0);
        true
    }

    fn run(&mut self, b: &mut Battle, who: usize, tree: &Record, node: &Value, board: &mut Board, depth: usize) -> Status {
        if depth > 40 || board.ended && depth > 0 && b.outcome.is_some() {
            return Status::Failure;
        }
        if node.get("Disabled").and_then(Value::as_bool).unwrap_or(false) {
            return Status::Success;
        }
        let ty = node.get("Type").and_then(Value::as_str).unwrap_or("");
        let name = ty.rsplit('.').next().unwrap_or(ty);
        let children: Vec<&Value> = node.get("Children").and_then(Value::as_array).map(|c| c.iter().collect()).unwrap_or_default();
        use Status::*;
        match name {
            "Sequence" => {
                for c in children {
                    if self.run(b, who, tree, c, board, depth + 1) == Failure {
                        return Failure;
                    }
                }
                Success
            }
            "Selector" => {
                for c in children {
                    if self.run(b, who, tree, c, board, depth + 1) == Success {
                        return Success;
                    }
                }
                Failure
            }
            "Inverter" => match children.first().map(|c| self.run(b, who, tree, c, board, depth + 1)) {
                Some(Success) => Failure,
                _ => Success,
            },
            "ReturnSuccess" => {
                if let Some(c) = children.first() {
                    self.run(b, who, tree, c, board, depth + 1);
                }
                Success
            }
            "ReturnFailure" => {
                if let Some(c) = children.first() {
                    self.run(b, who, tree, c, board, depth + 1);
                }
                Failure
            }
            "UntilFailure" => {
                for _ in 0..12 {
                    let Some(c) = children.first() else { break };
                    if self.run(b, who, tree, c, board, depth + 1) == Failure || b.outcome.is_some() {
                        break;
                    }
                }
                Success
            }
            "BehaviorTreeReference" => {
                let refs: Vec<i64> = node.get("ExternalBehavior[]externalBehaviors").and_then(Value::as_array).map(|a| a.iter().map(as_int).collect()).unwrap_or_default();
                let objects = tree.get("mBehaviorSource").and_then(|s| s.get("mTaskData")).and_then(|t| t.get("fieldSerializationData")).and_then(|f| f.get("unityObjects")).cloned();
                let mut status = Success;
                for r in refs {
                    let sub = objects.as_ref().and_then(|o| o.as_array()?.get(r as usize).cloned()).and_then(|p| self.db.resolve(tree, &p));
                    if let Some(sub) = sub
                        && let Some(root) = parse_tree(sub)
                    {
                        status = self.run(b, who, sub, &root, board, depth + 1);
                    }
                }
                status
            }
            "Log" | "ActionINodeControllableFocus" | "Wait" | "CancelIfVulnerableToOpportunity" => if name == "CancelIfVulnerableToOpportunity" { Failure } else { Success },
            "ActionSetBool" => {
                let key = shared_name(node, "SharedBoolsharedBool").unwrap_or_default();
                let value = node.get("SharedBoolvalue").and_then(|v| v.get("mValue")).and_then(Value::as_bool).unwrap_or(false);
                board.bools.insert(key, value);
                Success
            }
            "CompareSharedBool" => {
                let key = shared_name(node, "SharedBoolvariable").unwrap_or_default();
                let wanted = node.get("SharedBoolcompareTo").and_then(|v| v.get("mValue")).and_then(Value::as_bool).unwrap_or(false);
                if board.bools.get(&key).copied().unwrap_or(false) == wanted { Success } else { Failure }
            }
            "CompareSharedInt" => {
                let key = shared_name(node, "SharedIntvariable").unwrap_or_default();
                let wanted = node.get("SharedIntcompareTo").and_then(|v| v.get("mValue")).map(as_int).unwrap_or(0);
                if board.ints.get(&key).copied().unwrap_or(0) == wanted { Success } else { Failure }
            }
            "ActionSetRandomInt" => {
                let key = shared_name(node, "SharedIntstoreResult").unwrap_or_default();
                let lo = node.get("SharedIntmin").and_then(|v| v.get("mValue")).map(as_int).unwrap_or(0);
                let hi = node.get("SharedIntmax").and_then(|v| v.get("mValue")).map(as_int).unwrap_or(1);
                let v = b.rng.range(lo as i32, hi as i32 + 1) as i64;
                board.ints.insert(key, v);
                Success
            }
            "CanPlayINodeControllable" | "CanDoUtilityAction" => {
                if b.fighters[who].alive() && !board.ended && b.outcome.is_none() { Success } else { Failure }
            }
            "ActionINodeControllableEndTurn" | "ActionINodeControllableForceEndTurn" | "ActionEndTurn" => {
                board.ended = true;
                Success
            }
            "CanExecute" | "EquipmentIsUsable" => {
                let tag = self.tag(node);
                if tag == Some(1) {
                    // The move skill: while the turn's movement range isn't spent.
                    return if b.fighters[who].moves_left() >= 1.0 && b.fighters[who].move_range > 0.0 { Success } else { Failure };
                }
                match self.skill_with_tag(b, who, tag) {
                    Some(s) if name == "EquipmentIsUsable" || b.skill_blocked(self.db, who, s).is_none() => Success,
                    _ => Failure,
                }
            }
            "GetTargetsInRangeSkill" | "GetTargetsInRangeSkillAllTiles" | "GetTargetsInRangeSkillCone" | "HasTargetInRange" => {
                let Some(s) = self.skill_with_tag(b, who, self.tag(node)) else { return Failure };
                board.actions = b.targets(self.db, who, s).into_iter().filter(|&t| b.fighters[t].side != b.fighters[who].side).map(|t| TargetAction { skill: Some(s), target: t, dest: None }).collect();
                if board.actions.is_empty() { Failure } else { Success }
            }
            "GetTargetsInRangeWithMoveSkill" | "GetTargetsInRangeWithMoveSkillCone" => {
                let Some(s) = self.skill_with_tag(b, who, self.tag(node)) else { return Failure };
                let reach = b.reachable(who, b.fighters[who].moves_left());
                let mut actions = Vec::new();
                for (&cell, &(walk, _)) in &reach {
                    for t in b.targets_from(self.db, who, s, cell) {
                        if b.fighters[t].side != b.fighters[who].side {
                            actions.push(TargetAction { skill: Some(s), target: t, dest: Some((cell, walk)) });
                        }
                    }
                }
                board.actions = actions;
                if board.actions.is_empty() { Failure } else { Success }
            }
            "GetTargetsWithMoveNone" | "GetTargetsCtanWithMoveNone" => {
                // Walk as far as this turn allows towards the nearest node beside each enemy.
                let me = b.fighters[who].side;
                let far = b.reachable(who, 40.0);
                let range = b.fighters[who].moves_left();
                let mut actions = Vec::new();
                for t in 0..b.fighters.len() {
                    let f = &b.fighters[t];
                    if f.side == me || !f.alive() || f.kind != Kind::Unit {
                        continue;
                    }
                    let goal = far.iter().filter(|(c, _)| distance(**c, f.cell) < 1.5).min_by(|a, c| a.1.0.total_cmp(&c.1.0)).map(|(c, _)| *c).or_else(|| far.keys().copied().min_by(|a, c| distance(*a, f.cell).total_cmp(&distance(*c, f.cell))));
                    let Some(goal) = goal else { continue };
                    let path = Battle::path(&far, goal);
                    let stop = path.iter().rev().find(|c| far.get(c).is_some_and(|d| d.0 <= range + 1e-3)).copied();
                    if let Some(stop) = stop {
                        actions.push(TargetAction { skill: None, target: t, dest: Some((stop, far[&goal].0)) });
                    }
                }
                board.actions = actions;
                if board.actions.is_empty() { Failure } else { Success }
            }
            n if n.starts_with("Select") || n.starts_with("Choose") || n == "FindMostArmorUnitTarget" => {
                if board.actions.is_empty() {
                    return Failure;
                }
                let me = b.fighters[who].cell;
                let pick = |key: &dyn Fn(&TargetAction) -> f32, lowest: bool| -> TargetAction {
                    let mut best = board.actions[0].clone();
                    for a in &board.actions {
                        let (k, kb) = (key(a), key(&best));
                        if (lowest && k <= kb) || (!lowest && k >= kb) {
                            best = a.clone();
                        }
                    }
                    best
                };
                let fdist = |a: &TargetAction| distance(me, b.fighters[a.target].cell);
                let chosen = match n {
                    "SelectClosestTargetWithMove" | "SelectClosestTargetMaxMoveRange" => pick(&|a| a.dest.map(|d| d.1).unwrap_or(0.0), true),
                    "SelectClosestTargetAndClosestMove" => {
                        let target = pick(&fdist, true).target;
                        let same: Vec<TargetAction> = board.actions.iter().filter(|a| a.target == target).cloned().collect();
                        same.into_iter().min_by(|x, y| x.dest.map(|d| distance(d.0, me)).unwrap_or(0.0).total_cmp(&y.dest.map(|d| distance(d.0, me)).unwrap_or(0.0))).unwrap()
                    }
                    "SelectRandomTarget" => board.actions[b.rng.range(0, board.actions.len() as i32) as usize].clone(),
                    "SelectLowestLifeTarget" => pick(&|a| b.fighters[a.target].hp as f32, true),
                    "SelectHighestLifeTarget" | "SelectHighestLifeTargetMaxMoveRange" => pick(&|a| b.fighters[a.target].hp as f32, false),
                    "SelectMostArmorTarget" | "FindMostArmorUnitTarget" => pick(&|a| (b.fighters[a.target].armor_physical + b.fighters[a.target].armor_energetic) as f32, false),
                    "SelectFurthestTarget" | "SelectFurthestTargetMaxMoveRange" => pick(&fdist, false),
                    _ => pick(&fdist, true),
                };
                board.selected = Some(chosen);
                Success
            }
            "ActionSkillMove" => {
                let Some((cell, _)) = board.selected.as_ref().and_then(|a| a.dest) else { return Failure };
                if cell == b.fighters[who].cell {
                    return Failure;
                }
                if b.move_to(who, cell) { Success } else { Failure }
            }
            "ActionSkillAttack" | "ActionSkillArcRifle" | "ActionSkillTeslaCarbine" | "ActionSkillCognisFlammer" | "ActionSkillSolarAtomiser" | "ActionSkillVolkiteBlasterXenos" | "ActionSkillFireAttack" | "ActionSkillLordAgrolekhMelee" | "ActionSkillLordSzaregonMelee" => {
                let tag_skill = self.skill_with_tag(b, who, self.tag(node));
                let Some(a) = board.selected.clone() else { return Failure };
                let Some(s) = tag_skill.or(a.skill) else { return Failure };
                if b.use_skill(self.db, who, s, a.target) { Success } else { Failure }
            }
            _ => {
                self.unknown.insert(name.to_string());
                Failure
            }
        }
    }

    fn tag(&self, node: &Value) -> Option<i64> {
        let name = node.get("TAGskillTag").and_then(Value::as_str)?;
        self.tags.get(name).copied()
    }

    /// The fighter's skill carrying `tag` (SkillBehaviourTag.tags).
    fn skill_with_tag(&self, b: &Battle, who: usize, tag: Option<i64>) -> Option<usize> {
        let tag = tag?;
        b.fighters[who].skills.iter().position(|s| {
            self.db.get(s.skill).is_some_and(|skill| {
                skill.get("tag").map(|t| skill.deref(t)).and_then(|t| t.get("tags")).map(|l| items(skill.deref(l)).iter().any(|v| as_int(v) == tag)).unwrap_or(false)
            })
        })
    }
}

fn shared_name(node: &Value, key: &str) -> Option<String> {
    node.get(key)?.get("Name")?.as_str().map(str::to_string)
}

/// A tree's root task from its Behavior Designer JSON.
fn parse_tree(tree: &Record) -> Option<Value> {
    let json = tree.get("mBehaviorSource")?.get("mTaskData")?.get("JSONSerialization")?.as_str()?;
    let doc: Value = serde_json::from_str(json).ok()?;
    doc.get("RootTask").cloned()
}

/// The side's alive fighting units.
pub fn alive_units(b: &Battle, side: Side) -> Vec<usize> {
    (0..b.fighters.len()).filter(|&i| b.fighters[i].side == side && b.fighters[i].alive() && b.fighters[i].kind == Kind::Unit).collect()
}
