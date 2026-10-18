//! The Ark Mechanicus between missions: which missions are open (SaveDataArkMechanicusStory
//! .IsMissionAvailable), equipping tech-priests from owned blueprints, and deploying a team.

use serde_json::Value;

use super::save::{Deployment, Save, Unit};
use crate::data::{Db, Record, Ref, as_int, items};

/// Missions the campaign offers now, from MissionLibrary.missions.
pub fn available_missions<'a>(db: &'a Db, save: &Save) -> Vec<&'a Record> {
    let Some(lib) = db.single("MissionLibrary") else { return Vec::new() };
    let Some(config) = db.follow(lib, "missions") else { return Vec::new() };
    let done = &save.story.completed_missions;
    let countdown = save.story.overall_necron_countdown as f32;
    let szaregon = db.single("StoryLibrary").and_then(|s| db.follow(s, "lordSzaregonMission")).map(|m| m.id);
    db.follow_all(config, "missions")
        .into_iter()
        .filter(|m| !m.flag("isDlcHeretek") && !m.flag("isTutorial"))
        .filter(|m| !done.contains(&m.id))
        .filter(|m| !(save.settings.awakening && countdown >= 100.0 && Some(m.id) != szaregon))
        .filter(|m| countdown >= m.float("necronAwakeningRequirement"))
        .filter(|m| db.follow_all(m, "requiredMissions").iter().all(|r| done.contains(&r.id)))
        .filter(|m| {
            // A sector's boss opens after three of its missions.
            !m.flag("isSectorBoss") || m.int("tileset") == 6 || done.iter().filter_map(|d| db.get(*d)).filter(|d| d.int("tileset") == m.int("tileset")).count() >= 3
        })
        .collect()
}

/// The equipment slots a piece of equipment can go in (EquipmentBehaviourModelCustomize.availableSlots).
pub fn slots(equipment: &Record) -> Vec<i32> {
    let Some(custom) = equipment.get("modelCustomize").map(|c| equipment.deref(c)) else { return Vec::new() };
    custom.get("availableSlots").map(|s| items(equipment.deref(s)).iter().map(|v| as_int(v) as i32).collect()).unwrap_or_default()
}

/// Blueprints the player owns, with the equipment each makes.
pub fn owned_blueprints<'a>(db: &'a Db, save: &Save) -> Vec<(&'a Record, &'a Record)> {
    save.blueprints.iter().filter_map(|b| db.get(*b)).filter_map(|b| Some((b, db.follow(b, "equipment")?))).collect()
}

/// Craft a blueprint's equipment for a tech-priest (paying its blackstone cost) into the first slot it
/// fits. Returns why not, when it can't.
pub fn equip(db: &Db, save: &mut Save, techpriest: usize, blueprint: Ref) -> Result<(), String> {
    let bp = db.get(blueprint).ok_or("unknown blueprint")?;
    let equipment = db.follow(bp, "equipment").ok_or("blueprint without equipment")?;
    let cost = bp.int("cost") as i32;
    if save.blackstone < cost {
        return Err(format!("needs {cost} blackstone"));
    }
    let tp = save.techpriests.get_mut(techpriest).ok_or("no such tech-priest")?;
    let options = slots(equipment);
    let slot = options.iter().find(|s| !tp.equipment.contains_key(s)).or(options.first()).copied().ok_or("no slot for it")?;
    tp.equipment.insert(slot, equipment.id);
    save.blackstone -= cost;
    Ok(())
}

/// Troops the player has unlocked, as (blueprint, troop data).
pub fn troops<'a>(db: &'a Db, save: &Save) -> Vec<(&'a Record, &'a Record)> {
    save.troops.iter().filter_map(|t| db.get(*t)).filter_map(|t| Some((t, db.follow(t, "troop")?))).collect()
}

/// Deployment capacity for troops (GameplayConfig.baseMissionDeploymentCapacity).
pub fn capacity(db: &Db) -> i32 {
    db.single("GameplayConfig").map(|g| g.int("baseMissionDeploymentCapacity") as i32).unwrap_or(6)
}

/// UIArkMechWindowDeployment: the chosen tech-priests and troops go on `mission`.
pub fn deploy(db: &Db, save: &mut Save, mission: Ref, techpriests: &[usize], troop_data: &[Ref]) -> Result<(), String> {
    if techpriests.is_empty() {
        return Err("choose at least one tech-priest".into());
    }
    let mut units: Vec<Unit> = techpriests.iter().filter_map(|&i| save.techpriests.get(i).cloned()).collect();
    for t in troop_data {
        let data = db.get(*t).ok_or("unknown troop")?;
        units.push(Unit::create(db, data, &save.settings));
    }
    for u in &mut units {
        u.hp = u.hp.max(1);
    }
    save.deployment = Some(Deployment { units, canticles: save.canticles.clone(), mission });
    Ok(())
}

/// Text for a mission's card: name and description terms.
pub fn mission_terms(m: &Record) -> (String, String) {
    (m.str("missionName").to_string(), m.str("missionDescription").to_string())
}

pub fn troop_cost(data: &Record) -> i32 {
    data.get("missionDeploymentCost").map(as_int).filter(|&c| c > 0).or_else(|| data.get("deploymentCost").map(as_int)).unwrap_or(1) as i32
}

pub fn value_str(v: Option<&Value>) -> String {
    v.and_then(Value::as_str).unwrap_or("").to_string()
}
