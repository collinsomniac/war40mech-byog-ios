//! The campaign save: what the Ark knows between missions. New Game builds one from the game's own
//! defaults (MechanicusLibrary's new-game lists, the default canticles, the New Game window's settings).

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

use crate::data::{Db, Record, Ref, items};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Difficulty {
    Casual,
    #[default]
    Normal,
    Hard,
    Impossible,
}

/// The New Game window's settings (GameNewSaveState.Data's defaults).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Settings {
    pub difficulty: Difficulty,
    pub skip_tutorial: bool,
    pub ironman: bool,
    pub permadeath: bool,
    pub awakening: bool,
    pub awakening_room: i32,
    pub awakening_turn: i32,
    pub tp_life: i32,
    pub extra_necron: i32,
    pub necron_life: i32,
    pub extra_fight: i32,
    pub extra_movement: i32,
    pub narrative_event: bool,
    pub glyph_event: bool,
    pub aoe_weapon: bool,
    pub blackstone_start: i32,
    pub blackstone_modifier: i32,
    pub canticle: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            difficulty: Difficulty::Normal,
            skip_tutorial: false,
            ironman: false,
            permadeath: false,
            awakening: true,
            awakening_room: 2,
            awakening_turn: 1,
            tp_life: 0,
            extra_necron: 0,
            necron_life: 0,
            extra_fight: 0,
            extra_movement: 1,
            narrative_event: true,
            glyph_event: true,
            aoe_weapon: true,
            blackstone_start: 300,
            blackstone_modifier: 0,
            canticle: 3,
        }
    }
}

impl Settings {
    /// GameplayConfig.GetExtraFight.
    pub fn extra_fights(&self) -> i32 {
        match self.difficulty {
            Difficulty::Hard | Difficulty::Impossible => 1 + self.extra_fight,
            _ => self.extra_fight,
        }
    }

    /// GameplayConfig.GetNecronLifeModifier, in percent.
    pub fn necron_life_modifier(&self) -> i32 {
        self.necron_life + match self.difficulty {
            Difficulty::Casual => -25,
            Difficulty::Normal => 0,
            Difficulty::Hard => 25,
            Difficulty::Impossible => 50,
        }
    }

    /// GameplayConfig.GetExtraNecronModifier, in percent.
    pub fn extra_necron_modifier(&self) -> i32 {
        self.extra_necron + match self.difficulty {
            Difficulty::Casual => -25,
            Difficulty::Normal => 0,
            Difficulty::Hard => 25,
            Difficulty::Impossible => 50,
        }
    }

    pub fn blackstone_at_start(&self) -> i32 {
        self.blackstone_start + if self.difficulty == Difficulty::Casual { 200 } else { 0 }
    }
}

/// A tech-priest or troop the player owns: its data record plus what the campaign changed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Unit {
    pub base: Ref,
    pub class: String,
    pub name: String,
    pub hp: i32,
    pub hp_max: i32,
    pub level: i32,
    pub xp: i32,
    /// Slot index (Mechanicus.Data.Customizations.SLOT) → equipment.
    pub equipment: BTreeMap<i32, Ref>,
    pub skills: Vec<Ref>,
}

impl Unit {
    /// EntityData.Create: a fresh unit from its data record.
    pub fn create(db: &Db, base: &Record, settings: &Settings) -> Unit {
        let life = base.get("life").cloned().unwrap_or(Value::Null);
        let mut hp_max = life.get("hpMax").map(crate::data::as_int).unwrap_or(1) as f32;
        let techpriest = base.class == "TechpriestDataBase";
        if techpriest {
            hp_max = (hp_max + hp_max * settings.tp_life as f32 / 100.0).max(1.0);
        }
        let mut equipment = BTreeMap::new();
        if let Some(slots) = base.get("slotEquipments") {
            for pair in items(slots) {
                let kv = items(pair);
                if let (Some(slot), Some(eq)) = (kv.first(), kv.get(1))
                    && let Some(r) = db.resolve(base, eq)
                {
                    equipment.insert(crate::data::as_int(slot) as i32, r.id);
                }
            }
        }
        for (i, eq) in db.follow_all(base, "equipments").into_iter().enumerate() {
            equipment.entry(100 + i as i32).or_insert(eq.id);
        }
        let name = if techpriest {
            format!("{} {}", base.str("firstname"), base.str("lastname")).trim().to_string()
        } else {
            base.str("unitName").to_string()
        };
        Unit {
            base: base.id,
            class: base.class.clone(),
            name,
            hp: hp_max as i32,
            hp_max: hp_max as i32,
            level: base.int("level").max(1) as i32,
            xp: base.int("xp") as i32,
            equipment,
            skills: db.follow_all(base, "skills").iter().map(|s| s.id).collect(),
        }
    }

    pub fn is_techpriest(&self) -> bool {
        self.class == "TechpriestDataBase"
    }
}

/// Who goes into a tomb, with which canticles, on which mission.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Deployment {
    pub units: Vec<Unit>,
    pub canticles: Vec<Ref>,
    pub mission: Ref,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Story {
    pub intro_done: bool,
    pub tutorial_done: bool,
    /// The overall Necron awakening (0..100) that scales missions as the campaign goes on.
    pub overall_necron_countdown: i32,
    /// One-shot tutorial popups already shown, by StoryLibrary field name.
    pub seen: BTreeSet<String>,
    pub completed_missions: Vec<Ref>,
    /// The last mission's result, shown once on the Ark: (success, blackstone recovered).
    #[serde(default)]
    pub last_result: Option<(bool, i32)>,
}

impl Story {
    /// SaveDataArkMechanicusStory.GetClampedOverallNecronCountdown.
    pub fn clamped_countdown(&self) -> f32 {
        self.overall_necron_countdown.clamp(0, 100) as f32
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Save {
    pub settings: Settings,
    /// Blackstone ("data" in the game's code).
    pub blackstone: i32,
    pub canticles: Vec<Ref>,
    pub techpriests: Vec<Unit>,
    pub troops: Vec<Ref>,
    pub blueprints: Vec<Ref>,
    pub ship_skills: Vec<Ref>,
    pub story: Story,
    pub deployment: Option<Deployment>,
    pub techpriest_slots: i32,
    pub troop_slots: i32,
}

impl Save {
    /// GameNewSaveState.NewGame.
    pub fn new_game(db: &Db, settings: Settings) -> Result<Save, String> {
        let lib = db.single("MechanicusLibrary").ok_or("MechanicusLibrary missing from the pack")?;
        let techpriests = db.follow_all(lib, "newGameTechpriests").into_iter().map(|t| Unit::create(db, t, &settings)).collect();
        // GameConfig.DEFAULT_CANTICLES: Litany of Life, level 1.
        let canticles = db
            .all("Canticle")
            .filter(|c| c.name().contains("LitanyOfLife") || c.name().contains("Litany of Life"))
            .min_by_key(|c| c.int("level"))
            .map(|c| vec![c.id])
            .unwrap_or_default();
        let mut save = Save {
            blackstone: settings.blackstone_at_start(),
            settings,
            canticles,
            techpriests,
            troops: db.follow_all(lib, "newGameTroopBlueprint").iter().map(|r| r.id).collect(),
            blueprints: db.follow_all(lib, "newGameBlueprints").iter().map(|r| r.id).collect(),
            ship_skills: db.follow_all(lib, "newGameShipSkill").iter().map(|r| r.id).collect(),
            story: Story::default(),
            deployment: None,
            techpriest_slots: 4,
            troop_slots: 2,
        };
        if save.settings.skip_tutorial {
            save.story.intro_done = true;
            save.story.tutorial_done = true;
        }
        Ok(save)
    }

    /// TutoLauncher.LaunchDeploy: the intro ends by sending StoryLibrary's tutorial team into its tomb.
    pub fn deploy_tutorial(&mut self, db: &Db) -> Result<(), String> {
        let story = db.single("StoryLibrary").ok_or("StoryLibrary missing from the pack")?;
        let dungeon = db.follow(story, "tutorialDungeon").ok_or("StoryLibrary.tutorialDungeon unresolved")?;
        let mission = db.follow(dungeon, "mission").ok_or("tutorial DungeonTemplate.mission unresolved")?;
        let units = db.follow_all(story, "tutorialEntities").into_iter().map(|e| Unit::create(db, e, &self.settings)).collect();
        self.story.intro_done = true;
        self.deployment = Some(Deployment { units, canticles: Vec::new(), mission: mission.id });
        Ok(())
    }

    pub fn path(dir: &std::path::Path) -> std::path::PathBuf {
        dir.join("save.json")
    }

    pub fn write(&self, dir: &std::path::Path) -> Result<(), String> {
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        std::fs::write(Save::path(dir), json).map_err(|e| e.to_string())
    }

    pub fn read(dir: &std::path::Path) -> Option<Save> {
        serde_json::from_slice(&std::fs::read(Save::path(dir)).ok()?).ok()
    }
}
