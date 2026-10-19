//! Content pack sync: fetch the manifest from the user's PC over the tailnet and download changed files.

use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};

#[derive(Deserialize)]
pub struct Manifest {
    pub files: Vec<ManifestFile>,
}

#[derive(Deserialize)]
pub struct ManifestFile {
    pub path: String,
    pub size: u64,
    pub sha256: String,
}

/// Where the pack lives on the device. On iOS this is the app's Documents folder (visible in the Files app).
pub fn pack_dir() -> PathBuf {
    let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("."));
    home.join("Documents").join("pack")
}

fn sha256_hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

/// Files the game can't start without; a failure here stops the sync.
const CORE: &[&str] = &["data/", "loc/", "fonts/", "ui/"];

/// Pack folders fetched when first needed rather than at launch (each battle room is tens of MB and a
/// campaign visits a few at a time).
pub const ON_DEMAND: &[&str] = &["models/battle/"];

pub fn on_demand(path: &str) -> bool {
    ON_DEMAND.iter().any(|p| path.starts_with(p))
}

/// Checksums of files already synced (path -> sha256), so launches don't re-hash gigabytes.
type Index = std::collections::BTreeMap<String, String>;

fn index_path(dir: &Path) -> PathBuf {
    dir.join(".synced.json")
}

fn load_index(dir: &Path) -> Index {
    fs::read(index_path(dir)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_index(dir: &Path, index: &Index) {
    if let Ok(json) = serde_json::to_vec(index) {
        let _ = fs::write(index_path(dir), json);
    }
}

fn up_to_date(dir: &Path, index: &mut Index, file: &ManifestFile) -> bool {
    let local = dir.join(&file.path);
    let Ok(meta) = fs::metadata(&local) else { return false };
    if meta.len() != file.size {
        return false;
    }
    if index.get(&file.path) == Some(&file.sha256) {
        return true;
    }
    let ok = fs::read(&local).is_ok_and(|bytes| sha256_hex(&bytes) == file.sha256);
    if ok {
        index.insert(file.path.clone(), file.sha256.clone());
    }
    ok
}

fn download(server: &str, dir: &Path, file: &ManifestFile) -> Result<(), String> {
    let bytes = get(&format!("{server}/{}", file.path))?;
    if sha256_hex(&bytes) != file.sha256 {
        return Err(format!("{}: checksum mismatch", file.path));
    }
    let local = dir.join(&file.path);
    fs::create_dir_all(local.parent().unwrap()).map_err(|e| e.to_string())?;
    let partial = local.with_extension("partial");
    fs::write(&partial, &bytes).map_err(|e| e.to_string())?;
    fs::rename(&partial, &local).map_err(|e| e.to_string())
}

/// Fetch one on-demand file named in the last synced manifest, if it isn't here yet.
pub fn fetch(server: &str, dir: &Path, path: &str) -> Result<(), String> {
    let bytes = fs::read(dir.join("manifest.json")).map_err(|e| format!("manifest: {e}"))?;
    let manifest: Manifest = serde_json::from_slice(&bytes).map_err(|e| format!("manifest: {e}"))?;
    let file = manifest.files.iter().find(|f| f.path == path).ok_or_else(|| format!("{path} is not in the pack"))?;
    let mut index = load_index(dir);
    if up_to_date(dir, &mut index, file) {
        return Ok(());
    }
    download(server, dir, file)?;
    index.insert(file.path.clone(), file.sha256.clone());
    save_index(dir, &index);
    Ok(())
}

fn get(url: &str) -> Result<Vec<u8>, String> {
    let mut response = ureq::get(url).call().map_err(|e| format!("{url}: {e}"))?;
    let mut bytes = Vec::new();
    response
        .body_mut()
        .as_reader()
        .read_to_end(&mut bytes)
        .map_err(|e| format!("{url}: {e}"))?;
    Ok(bytes)
}

/// Bring `dir` in line with the server's manifest. `report` receives human-readable progress lines.
pub fn sync(server: &str, dir: &Path, report: &dyn Fn(String)) -> Result<Manifest, String> {
    report(format!("Contacting {server} ..."));
    let manifest_bytes = get(&format!("{server}/manifest.json"))?;
    let manifest: Manifest = serde_json::from_slice(&manifest_bytes).map_err(|e| format!("manifest: {e}"))?;

    let mut index = load_index(dir);
    let stale: Vec<&ManifestFile> = manifest.files.iter().filter(|f| !on_demand(&f.path) && !up_to_date(dir, &mut index, f)).collect();
    save_index(dir, &index);
    let total: u64 = stale.iter().map(|f| f.size).sum();
    let mut failed: Vec<String> = Vec::new();
    let mut done = 0u64;
    for (i, file) in stale.iter().enumerate() {
        report(format!(
            "Downloading {}/{}: {} ({:.1} / {:.1} MB)",
            i + 1,
            stale.len(),
            file.path,
            done as f64 / 1e6,
            total as f64 / 1e6
        ));
        // One bad file shouldn't strand the player: note it and carry on; only core files are fatal.
        match download(server, dir, file) {
            Ok(()) => {
                index.insert(file.path.clone(), file.sha256.clone());
            }
            Err(e) => failed.push(e),
        }
        if i % 20 == 0 {
            save_index(dir, &index);
        }
        done += file.size;
    }
    save_index(dir, &index);
    if let Some(core) = failed.iter().find(|e| CORE.iter().any(|c| e.starts_with(c) || e.contains(&format!("/{c}")))) {
        return Err(format!("{} file(s) failed, including {core}", failed.len()));
    }
    if !failed.is_empty() {
        report(format!("{} file(s) couldn't be downloaded (first: {}); continuing.", failed.len(), failed[0]));
    }
    // Kept locally so a later launch without the PC can still play from this pack.
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    fs::write(dir.join("manifest.json"), &manifest_bytes).map_err(|e| e.to_string())?;
    Ok(manifest)
}
