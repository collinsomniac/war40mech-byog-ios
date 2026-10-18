//! The game's data layer: every ScriptableObject the converter exported to `pack/data/<Class>.json`,
//! with Unity object references resolved across files.
//!
//! A record keeps Unity's own serialized fields (`fields`) and, for Odin-serialized classes, the Odin
//! graph (`odin`). Both are plain JSON; the typed views in `rules` read what they need from them.

use bevy::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;

/// A Unity object: (hash of its file's name, path id within that file). Stable across launches, so
/// saves can store it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, Deserialize)]
pub struct Ref {
    pub file: u32,
    pub path_id: i64,
}

/// FNV-1a: a file name's id.
pub fn file_id(name: &str) -> u32 {
    name.bytes().fold(0x811c_9dc5u32, |h, b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

#[derive(Deserialize)]
struct RawRecord {
    file: String,
    path_id: i64,
    fields: Value,
    #[serde(default)]
    odin: Option<Value>,
    #[serde(default)]
    odin_unity_refs: Vec<Value>,
}

pub struct Record {
    pub class: String,
    pub id: Ref,
    pub fields: Value,
    pub odin: Value,
    odin_unity_refs: Vec<Value>,
    /// Odin `$id` → node, for `$ref_internal` links inside this record's graph.
    internal: HashMap<i64, Value>,
}

impl Record {
    pub fn name(&self) -> &str {
        self.fields.get("m_Name").and_then(Value::as_str).unwrap_or("")
    }

    /// A Unity field, or failing that an Odin member, by name.
    pub fn get(&self, key: &str) -> Option<&Value> {
        self.fields.get(key).or_else(|| self.odin.get(key))
    }

    pub fn str(&self, key: &str) -> &str {
        self.get(key).and_then(Value::as_str).unwrap_or("")
    }

    pub fn int(&self, key: &str) -> i64 {
        self.get(key).map(as_int).unwrap_or(0)
    }

    pub fn float(&self, key: &str) -> f32 {
        self.get(key).and_then(Value::as_f64).unwrap_or(0.0) as f32
    }

    pub fn flag(&self, key: &str) -> bool {
        self.get(key).map(as_bool).unwrap_or(false)
    }

    /// Follow an Odin `$ref_internal` to the node it names; any other value is returned as is.
    pub fn deref<'a>(&'a self, value: &'a Value) -> &'a Value {
        match value.get("$ref_internal").and_then(Value::as_i64) {
            Some(id) => self.internal.get(&id).unwrap_or(&Value::Null),
            None => value,
        }
    }
}

pub fn as_int(v: &Value) -> i64 {
    v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).or_else(|| v.as_bool().map(i64::from)).unwrap_or(0)
}

pub fn as_bool(v: &Value) -> bool {
    v.as_bool().unwrap_or_else(|| as_int(v) != 0)
}

/// The items of an Odin list (`{"$items": [[...]]}`), a Unity array, or nothing.
pub fn items(v: &Value) -> &[Value] {
    if let Some(list) = v.as_array() {
        return list;
    }
    match v.get("$items").and_then(Value::as_array) {
        Some(outer) if outer.len() == 1 && outer[0].is_array() => outer[0].as_array().unwrap(),
        Some(outer) => outer,
        None => &[],
    }
}

/// The short class name of an Odin node: `"Mechanicus.X.Foo, Assembly-CSharp"` → `"Foo"`.
pub fn odin_type(v: &Value) -> &str {
    let full = v.get("$type").and_then(Value::as_str).unwrap_or("");
    let name = full.split(',').next().unwrap_or("");
    name.rsplit(['.', '+']).next().unwrap_or(name)
}

#[derive(Resource, Default)]
pub struct Db {
    /// Per file, its externals (PPtr m_FileID n > 0 means externals[n - 1]) as file ids.
    externals: HashMap<u32, Vec<u32>>,
    records: Vec<Record>,
    /// Enum tables from the game assembly (`_enums.json`): type name → (member → value).
    enums: Option<HashMap<String, std::collections::BTreeMap<String, i64>>>,
    by_ref: HashMap<Ref, usize>,
    by_class: HashMap<String, Vec<usize>>,
}

impl Db {
    pub fn load(pack: &Path) -> Result<Db, String> {
        let dir = pack.join("data");
        let read = |path: &Path| std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()));
        let externals: HashMap<String, Vec<String>> =
            serde_json::from_slice(&read(&dir.join("_files.json"))?).map_err(|e| format!("_files.json: {e}"))?;
        let mut db = Db::default();
        db.enums = std::fs::read(dir.join("_enums.json")).ok().and_then(|b| serde_json::from_slice(&b).ok());
        for (file, ext) in &externals {
            db.externals.insert(file_id(file), ext.iter().map(|e| file_id(e)).collect());
        }
        let mut entries: Vec<_> = std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten().collect();
        entries.sort_by_key(|e| e.file_name());
        for entry in entries {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if name.starts_with('_') || !name.ends_with(".json") {
                continue;
            }
            let class = name.trim_end_matches(".json").to_string();
            let raw: Vec<RawRecord> = serde_json::from_slice(&read(&path)?).map_err(|e| format!("{name}: {e}"))?;
            for r in raw {
                let id = Ref { file: file_id(&r.file), path_id: r.path_id };
                let odin = r.odin.unwrap_or(Value::Null);
                let mut internal = HashMap::new();
                collect_ids(&odin, &mut internal);
                let index = db.records.len();
                db.by_ref.insert(id, index);
                db.by_class.entry(class.clone()).or_default().push(index);
                db.records.push(Record { class: class.clone(), id, fields: r.fields, odin, odin_unity_refs: r.odin_unity_refs, internal });
            }
        }
        Ok(db)
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn enums(&self) -> Option<&HashMap<String, std::collections::BTreeMap<String, i64>>> {
        self.enums.as_ref()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }

    pub fn get(&self, id: Ref) -> Option<&Record> {
        self.by_ref.get(&id).map(|&i| &self.records[i])
    }

    pub fn all(&self, class: &str) -> impl Iterator<Item = &Record> {
        self.by_class.get(class).into_iter().flatten().map(|&i| &self.records[i])
    }

    pub fn named(&self, class: &str, name: &str) -> Option<&Record> {
        self.all(class).find(|r| r.name() == name)
    }

    /// The single instance of a library/config class.
    pub fn single(&self, class: &str) -> Option<&Record> {
        self.all(class).next()
    }

    /// Resolve a Unity PPtr (`{"m_FileID", "m_PathID"}`) written in `from`'s file.
    pub fn pptr(&self, from: &Record, pptr: &Value) -> Option<Ref> {
        let file_id = pptr.get("m_FileID").map(as_int)?;
        let path_id = pptr.get("m_PathID").map(as_int)?;
        if path_id == 0 {
            return None;
        }
        let file = if file_id == 0 {
            from.id.file
        } else {
            *self.externals.get(&from.id.file)?.get(file_id as usize - 1)?
        };
        Some(Ref { file, path_id })
    }

    /// Resolve any reference value: a PPtr, or an Odin `{"$ref_unity": n}` into the record's Unity refs.
    pub fn resolve(&self, from: &Record, value: &Value) -> Option<&Record> {
        let pptr = match value.get("$ref_unity").and_then(Value::as_u64) {
            Some(n) => from.odin_unity_refs.get(n as usize)?,
            None => value,
        };
        self.get(self.pptr(from, pptr)?)
    }

    /// `from[key]` resolved to the record it references.
    pub fn follow(&self, from: &Record, key: &str) -> Option<&Record> {
        self.resolve(from, from.get(key)?)
    }

    /// `from[key]` as a list of referenced records (unresolvable entries skipped).
    pub fn follow_all(&self, from: &Record, key: &str) -> Vec<&Record> {
        from.get(key).map(|v| items(v).iter().filter_map(|x| self.resolve(from, x)).collect()).unwrap_or_default()
    }
}

fn collect_ids(v: &Value, out: &mut HashMap<i64, Value>) {
    match v {
        Value::Object(map) => {
            if let (Some(id), Some(_)) = (map.get("$id").and_then(Value::as_i64), map.get("$type")) {
                out.insert(id, v.clone());
            }
            map.values().for_each(|x| collect_ids(x, out));
        }
        Value::Array(list) => list.iter().for_each(|x| collect_ids(x, out)),
        _ => {}
    }
}
