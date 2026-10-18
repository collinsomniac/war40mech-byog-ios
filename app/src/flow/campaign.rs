//! The campaign in play: the game's data, the save, and the tomb or battle in progress. Saved after
//! every step that matters (the game autosaves the same moments: mission start, room changes, battle
//! results), to `<saves>/campaign.json` next to the content pack.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

use crate::data::Db;
use crate::rules::battle::Battle;
use crate::rules::explore::Expedition;
use crate::rules::save::Save;

#[derive(Resource, Clone)]
pub struct GameDb(pub Arc<Db>);

#[derive(Resource, Default, Serialize, Deserialize)]
pub struct Campaign {
    pub save: Option<Save>,
    pub expedition: Option<Expedition>,
    #[serde(skip)]
    pub battle: Option<Battle>,
    /// Where the expedition's fight happens (tomb room index), while a battle runs.
    #[serde(skip)]
    pub battle_room: Option<usize>,
}

pub fn saves_dir(pack: &std::path::Path) -> PathBuf {
    std::env::var_os("COGITATOR_SAVES").map(PathBuf::from).unwrap_or_else(|| pack.parent().unwrap_or(pack).join("saves"))
}

impl Campaign {
    pub fn path(pack: &std::path::Path) -> PathBuf {
        saves_dir(pack).join("campaign.json")
    }

    pub fn exists(pack: &std::path::Path) -> bool {
        Campaign::path(pack).exists()
    }

    pub fn load(pack: &std::path::Path) -> Option<Campaign> {
        serde_json::from_slice(&std::fs::read(Campaign::path(pack)).ok()?).ok()
    }

    pub fn store(&self, pack: &std::path::Path) {
        let path = Campaign::path(pack);
        let result = std::fs::create_dir_all(path.parent().unwrap())
            .map_err(|e| e.to_string())
            .and_then(|_| serde_json::to_vec(self).map_err(|e| e.to_string()))
            .and_then(|json| std::fs::write(&path, json).map_err(|e| e.to_string()));
        if let Err(e) = result {
            warn!("saving the campaign to {}: {e}", path.display());
        }
    }
}
