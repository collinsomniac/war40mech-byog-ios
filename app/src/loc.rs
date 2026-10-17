//! Localization: the game's I2 terms, exported per language into pack/loc/<code>.json.

use bevy::prelude::*;
use std::collections::HashMap;
use std::path::Path;

#[derive(Resource, Default)]
pub struct Loc {
    pub language: String,
    table: HashMap<String, String>,
}

impl Loc {
    pub fn load(pack: &Path, language: &str) -> Result<Loc, String> {
        let file = pack.join("loc").join(format!("{language}.json"));
        let bytes = std::fs::read(&file).map_err(|e| format!("{}: {e}", file.display()))?;
        let table = serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", file.display()))?;
        Ok(Loc { language: language.to_string(), table })
    }

    pub fn get(&self, term: &str) -> Option<&str> {
        self.table.get(term).map(String::as_str)
    }

    pub fn len(&self) -> usize {
        self.table.len()
    }

    pub fn is_empty(&self) -> bool {
        self.table.is_empty()
    }
}

/// TMP rich text reduced to plain text: tags dropped, `<br>` kept as a newline.
pub fn plain(text: &str) -> String {
    let text = text.replace("<br>", "\n");
    let mut out = String::with_capacity(text.len());
    let mut in_tag = false;
    for c in text.chars() {
        match c {
            '<' => in_tag = true,
            '>' if in_tag => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    out
}
